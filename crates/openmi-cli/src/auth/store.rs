use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use openmi_platform::{atomic_write_private, AppPaths};
use serde::{Deserialize, Serialize};

use crate::auth::AuthError;

/// Persisted authentication credentials for `openmi`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Unix epoch seconds when `access_token` expires (`0` means unknown/manual).
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub api_base_url: Option<String>,
}

impl TokenSet {
    /// Returns true if the token is expired or within `skew_secs` of expiring.
    pub fn is_expired_within(&self, skew_secs: u64) -> bool {
        if self.expires_at == 0 {
            return false;
        }
        let now = now_unix_secs();
        now.saturating_add(skew_secs) >= self.expires_at
    }
}

pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Abstract credential store (supports OS keyring or 0600 file fallback).
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<TokenSet>, AuthError>;
    fn save(&self, tokens: &TokenSet) -> Result<(), AuthError>;
    fn clear(&self) -> Result<(), AuthError>;
}

/// 0600-protected file token store (works reliably on Linux, macOS, Windows, and Termux).
#[derive(Debug, Clone)]
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self) -> Result<Option<TokenSet>, AuthError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let raw = fs::read(&self.path)?;
        let tokens: TokenSet = serde_json::from_slice(&raw)?;
        Ok(Some(tokens))
    }

    fn save(&self, tokens: &TokenSet) -> Result<(), AuthError> {
        let bytes = serde_json::to_vec_pretty(tokens)?;
        atomic_write_private(&self.path, &bytes)?;
        Ok(())
    }

    fn clear(&self) -> Result<(), AuthError> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

/// Resolve the default token store using `OPENMI_CREDENTIALS_FILE` override or `AppPaths`.
pub fn default_token_store() -> Result<FileTokenStore, AuthError> {
    if let Ok(custom) = std::env::var("OPENMI_CREDENTIALS_FILE") {
        if !custom.trim().is_empty() {
            return Ok(FileTokenStore::new(PathBuf::from(custom)));
        }
    }
    let paths = AppPaths::resolve()?;
    Ok(FileTokenStore::new(paths.credentials_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trip_and_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(tmp.path().join("creds.json"));
        assert!(store.load().unwrap().is_none());

        let tokens = TokenSet {
            access_token: "acc-123".to_string(),
            refresh_token: "ref-456".to_string(),
            expires_at: now_unix_secs() + 3600,
            user_id: Some("u-1".to_string()),
            email: Some("dev@openmi.space".to_string()),
            api_base_url: Some("https://umjlmqeghfrfpduhkits.supabase.co".to_string()),
        };

        store.save(&tokens).unwrap();
        let loaded = store.load().unwrap().expect("tokens saved");
        assert_eq!(loaded, tokens);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "token file must have 0600 permissions");
        }

        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn detects_expired_tokens_with_skew() {
        let now = now_unix_secs();
        let expired = TokenSet {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: now.saturating_sub(10),
            user_id: None,
            email: None,
            api_base_url: None,
        };
        assert!(expired.is_expired_within(60));

        let fresh = TokenSet {
            expires_at: now + 600,
            ..expired
        };
        assert!(!fresh.is_expired_within(60));
    }
}
