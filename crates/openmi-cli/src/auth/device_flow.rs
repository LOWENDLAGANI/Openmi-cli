use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::sleep;

use crate::auth::store::{now_unix_secs, TokenSet, TokenStore};

/// Production Openmi backend (Supabase project). Used when neither `--api-url`,
/// `OPENMI_API_URL`, nor a stored credential base URL is available.
pub const DEFAULT_API_BASE_URL: &str = "https://umjlmqeghfrfpduhkits.supabase.co";

/// Public (publishable) Supabase key for the production project. Required as the `apikey`
/// header by the Supabase API gateway for GoTrue routes (`/auth/v1/*`). It is safe to embed:
/// it grants no privileges beyond what RLS / Edge Function auth already allow.
pub const DEFAULT_SUPABASE_ANON_KEY: &str = "sb_publishable_5VfWWlMRio4X-dCfq8IZnw_oYPbh6db";

/// Env var that overrides the publishable/anon key (e.g. for staging projects).
pub const ANON_KEY_ENV: &str = "OPENMI_SUPABASE_ANON_KEY";

/// Choose the `apikey` header value for `base_url`.
///
/// An explicit env override always wins. The built-in key is only used for the default
/// production project, since sending it to another Supabase project would be rejected.
pub fn select_anon_key(base_url: &str, env_override: Option<String>) -> Option<String> {
    if let Some(key) = env_override.filter(|k| !k.trim().is_empty()) {
        return Some(key.trim().to_string());
    }
    if base_url.trim_end_matches('/') == DEFAULT_API_BASE_URL {
        return Some(DEFAULT_SUPABASE_ANON_KEY.to_string());
    }
    None
}

/// Resolve the `apikey` header value for `base_url` using `OPENMI_SUPABASE_ANON_KEY`.
pub fn resolve_anon_key(base_url: &str) -> Option<String> {
    select_anon_key(base_url, std::env::var(ANON_KEY_ENV).ok())
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("not logged in — run `openmi login` first")]
    NotLoggedIn,

    #[error("session revoked or invalid — run `openmi login` to re-authenticate")]
    Revoked,

    #[error("device login expired before approval — run `openmi login` to try again")]
    Expired,

    #[error("device login was denied")]
    Denied,

    #[error("token expired and no refresh token is available — run `openmi login`")]
    MissingRefreshToken,

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("unexpected auth server response ({status}): {body}")]
    Server { status: u16, body: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verify_url: String,
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_expires_in() -> u64 {
    600
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Clone, Deserialize)]
pub struct PollUser {
    pub id: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DevicePollResponse {
    pub status: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    pub interval: Option<u64>,
    pub user: Option<PollUser>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RefreshResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WhoAmI {
    pub id: String,
    pub email: Option<String>,
    pub plan: Option<String>,
}

/// Resolve the base API URL from explicit override, `OPENMI_API_URL`, stored token, or default.
pub fn resolve_api_base_url(explicit: Option<&str>, stored: Option<&TokenSet>) -> String {
    if let Some(url) = explicit.filter(|s| !s.trim().is_empty()) {
        return url.trim_end_matches('/').to_string();
    }
    if let Ok(env_url) = std::env::var("OPENMI_API_URL") {
        if !env_url.trim().is_empty() {
            return env_url.trim_end_matches('/').to_string();
        }
    }
    if let Some(url) = stored.and_then(|t| t.api_base_url.as_deref()) {
        if !url.trim().is_empty() {
            return url.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_API_BASE_URL.to_string()
}

/// Client for the `cli-auth` Supabase Edge Function and managed auth refresh/whoami endpoints.
#[derive(Debug, Clone)]
pub struct DeviceAuthClient {
    base_url: String,
    http: reqwest::Client,
    min_poll_delay: Duration,
    anon_key: Option<String>,
}

impl DeviceAuthClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, AuthError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let anon_key = resolve_anon_key(&base_url);
        Ok(Self {
            base_url,
            http,
            min_poll_delay: Duration::from_secs(1),
            anon_key,
        })
    }

    /// Override the Supabase `apikey` header value (`None` disables it).
    #[cfg(test)]
    pub fn with_anon_key(mut self, key: Option<String>) -> Self {
        self.anon_key = key.filter(|k| !k.trim().is_empty());
        self
    }

    /// Build a request, attaching the Supabase `apikey` header when configured.
    ///
    /// The gateway requires `apikey` on `/auth/v1/*`; it is harmless on `/functions/v1/*`.
    /// Publishable keys (`sb_publishable_…`) are not JWTs, so they are never sent as
    /// `Authorization: Bearer` — only the user's access token is.
    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let mut req = self.http.request(method, url);
        if let Some(key) = &self.anon_key {
            req = req.header("apikey", key);
        }
        req
    }

    #[cfg(test)]
    pub fn with_poll_delay(mut self, delay: Duration) -> Self {
        self.min_poll_delay = delay;
        self
    }

    /// `POST /functions/v1/cli-auth/start`
    pub async fn start_device_flow(&self) -> Result<DeviceStartResponse, AuthError> {
        let url = format!("{}/functions/v1/cli-auth/start", self.base_url);
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&serde_json::json!({}))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::Server {
                status: status.as_u16(),
                body,
            });
        }
        Ok(resp.json::<DeviceStartResponse>().await?)
    }

    /// `POST /functions/v1/cli-auth/poll` once.
    pub async fn poll_once(&self, device_code: &str) -> Result<DevicePollResponse, AuthError> {
        let url = format!("{}/functions/v1/cli-auth/poll", self.base_url);
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&serde_json::json!({ "device_code": device_code }))
            .send()
            .await?;

        let status = resp.status().as_u16();
        if status == 404 || status == 410 {
            return Err(AuthError::Expired);
        }
        if status == 429 {
            // Rate limit / slow_down may include JSON body with interval.
            let parsed = resp
                .json::<DevicePollResponse>()
                .await
                .unwrap_or(DevicePollResponse {
                    status: "slow_down".to_string(),
                    access_token: None,
                    refresh_token: None,
                    expires_at: None,
                    interval: Some(7),
                    user: None,
                });
            return Ok(parsed);
        }
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::Server { status, body });
        }
        Ok(resp.json::<DevicePollResponse>().await?)
    }

    /// Poll until approved, denied, or expired.
    pub async fn poll_until_approved(
        &self,
        start: &DeviceStartResponse,
    ) -> Result<TokenSet, AuthError> {
        let deadline = now_unix_secs().saturating_add(start.expires_in.max(1));
        let mut interval_secs = start.interval.max(1);

        loop {
            let wait = Duration::from_secs(interval_secs)
                .min(self.min_poll_delay.max(Duration::from_millis(10)));
            let delay = if self.min_poll_delay >= Duration::from_secs(1) {
                Duration::from_secs(interval_secs)
            } else {
                wait
            };
            sleep(delay).await;

            if now_unix_secs() > deadline {
                return Err(AuthError::Expired);
            }

            let poll = self.poll_once(&start.device_code).await?;
            match poll.status.as_str() {
                "pending" => continue,
                "slow_down" => {
                    interval_secs = poll.interval.unwrap_or(interval_secs + 2);
                    continue;
                }
                "denied" => return Err(AuthError::Denied),
                "expired" => return Err(AuthError::Expired),
                "approved" => {
                    let access_token = poll.access_token.ok_or_else(|| AuthError::Server {
                        status: 200,
                        body: "approved response missing access_token".to_string(),
                    })?;
                    return Ok(TokenSet {
                        access_token,
                        refresh_token: poll.refresh_token.unwrap_or_default(),
                        expires_at: poll.expires_at.unwrap_or(0),
                        user_id: poll.user.as_ref().and_then(|u| u.id.clone()),
                        email: poll.user.as_ref().and_then(|u| u.email.clone()),
                        api_base_url: Some(self.base_url.clone()),
                    });
                }
                other => {
                    return Err(AuthError::Server {
                        status: 200,
                        body: format!("unexpected poll status: {other}"),
                    });
                }
            }
        }
    }

    /// Exchange a refresh token for a fresh access token.
    pub async fn refresh_tokens(&self, current: &TokenSet) -> Result<TokenSet, AuthError> {
        if current.refresh_token.trim().is_empty() {
            return Err(AuthError::MissingRefreshToken);
        }
        let url = format!("{}/auth/v1/token?grant_type=refresh_token", self.base_url);
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&serde_json::json!({ "refresh_token": current.refresh_token }))
            .send()
            .await?;

        let status = resp.status().as_u16();
        if status == 400 || status == 401 || status == 403 {
            return Err(AuthError::Revoked);
        }
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::Server { status, body });
        }

        let refreshed: RefreshResponse = resp.json().await?;
        let expires_at = refreshed.expires_at.unwrap_or_else(|| {
            refreshed
                .expires_in
                .map(|secs| now_unix_secs().saturating_add(secs))
                .unwrap_or(0)
        });

        Ok(TokenSet {
            access_token: refreshed.access_token,
            refresh_token: refreshed
                .refresh_token
                .unwrap_or_else(|| current.refresh_token.clone()),
            expires_at,
            user_id: current.user_id.clone(),
            email: current.email.clone(),
            api_base_url: Some(self.base_url.clone()),
        })
    }

    /// Fetch the authenticated user profile using a bearer access token.
    pub async fn whoami(&self, access_token: &str) -> Result<WhoAmI, AuthError> {
        let url = format!("{}/auth/v1/user", self.base_url);
        let resp = self
            .request(reqwest::Method::GET, &url)
            .bearer_auth(access_token)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(AuthError::Revoked);
        }
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::Server { status, body });
        }
        Ok(resp.json::<WhoAmI>().await?)
    }

    /// Load credentials from `store`, refreshing automatically if expired or near expiry.
    pub async fn ensure_valid_tokens(&self, store: &dyn TokenStore) -> Result<TokenSet, AuthError> {
        let tokens = store.load()?.ok_or(AuthError::NotLoggedIn)?;
        if tokens.is_expired_within(60) {
            let refreshed = self.refresh_tokens(&tokens).await?;
            store.save(&refreshed)?;
            return Ok(refreshed);
        }
        Ok(tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::store::FileTokenStore;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_mock_auth_server(
        handler: impl Fn(String, String) -> (u16, String) + Send + Sync + 'static,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let h = Arc::clone(&handler);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let first_line = req.lines().next().unwrap_or("");
                    let path = first_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .to_string();
                    let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();

                    let (status, resp_body) = h(path, body);
                    let status_text = match status {
                        200 => "OK",
                        401 => "Unauthorized",
                        429 => "Too Many Requests",
                        _ => "Error",
                    };
                    let response = format!(
                        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                        resp_body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });

        format!("http://{addr}")
    }

    #[tokio::test]
    async fn headless_device_flow_polls_and_persists_0600_tokens() {
        let poll_hits = Arc::new(AtomicUsize::new(0));
        let hits = Arc::clone(&poll_hits);

        let base_url = spawn_mock_auth_server(move |path, _body| {
            if path == "/functions/v1/cli-auth/start" {
                (
                    200,
                    serde_json::json!({
                        "device_code": "dev-secret-1",
                        "user_code": "ABCD-EFGH",
                        "verify_url": "https://www.openmi.space/verify?code=ABCD-EFGH",
                        "expires_in": 600,
                        "interval": 1
                    })
                    .to_string(),
                )
            } else if path == "/functions/v1/cli-auth/poll" {
                let attempt = hits.fetch_add(1, Ordering::SeqCst);
                match attempt {
                    0 => (200, r#"{"status":"pending"}"#.to_string()),
                    1 => (429, r#"{"status":"slow_down","interval":1}"#.to_string()),
                    _ => (
                        200,
                        serde_json::json!({
                            "status": "approved",
                            "access_token": "jwt-fresh",
                            "refresh_token": "ref-fresh",
                            "expires_at": now_unix_secs() + 3600,
                            "user": { "id": "user-42", "email": "alice@openmi.dev" }
                        })
                        .to_string(),
                    ),
                }
            } else {
                (404, r#"{"error":"not_found"}"#.to_string())
            }
        })
        .await;

        let client = DeviceAuthClient::new(&base_url)
            .unwrap()
            .with_poll_delay(Duration::from_millis(10));
        let start = client.start_device_flow().await.unwrap();
        assert_eq!(start.user_code, "ABCD-EFGH");

        let tokens = client.poll_until_approved(&start).await.unwrap();
        assert_eq!(tokens.access_token, "jwt-fresh");
        assert_eq!(tokens.refresh_token, "ref-fresh");
        assert_eq!(tokens.email.as_deref(), Some("alice@openmi.dev"));
        assert!(poll_hits.load(Ordering::SeqCst) >= 3);

        let tmp = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(tmp.path().join("credentials.json"));
        store.save(&tokens).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[tokio::test]
    async fn expired_token_refreshes_automatically() {
        let base_url = spawn_mock_auth_server(|path, _body| {
            if path.starts_with("/auth/v1/token") {
                (
                    200,
                    serde_json::json!({
                        "access_token": "jwt-renewed",
                        "refresh_token": "ref-renewed",
                        "expires_in": 3600
                    })
                    .to_string(),
                )
            } else {
                (404, "{}".to_string())
            }
        })
        .await;

        let tmp = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(tmp.path().join("credentials.json"));
        store
            .save(&TokenSet {
                access_token: "jwt-old".to_string(),
                refresh_token: "ref-old".to_string(),
                expires_at: now_unix_secs().saturating_sub(120),
                user_id: Some("u-1".to_string()),
                email: Some("bob@openmi.dev".to_string()),
                api_base_url: Some(base_url.clone()),
            })
            .unwrap();

        let client = DeviceAuthClient::new(&base_url).unwrap();
        let valid = client.ensure_valid_tokens(&store).await.unwrap();
        assert_eq!(valid.access_token, "jwt-renewed");
        assert_eq!(valid.refresh_token, "ref-renewed");

        let persisted = store.load().unwrap().unwrap();
        assert_eq!(persisted.access_token, "jwt-renewed");
    }

    #[tokio::test]
    async fn revoked_token_returns_clear_error() {
        let base_url = spawn_mock_auth_server(|_path, _body| {
            (401, r#"{"error":"invalid_grant"}"#.to_string())
        })
        .await;

        let tmp = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(tmp.path().join("credentials.json"));
        store
            .save(&TokenSet {
                access_token: "jwt-revoked".to_string(),
                refresh_token: "ref-revoked".to_string(),
                expires_at: now_unix_secs().saturating_sub(10),
                user_id: None,
                email: None,
                api_base_url: Some(base_url.clone()),
            })
            .unwrap();

        let client = DeviceAuthClient::new(&base_url).unwrap();
        let err = client.ensure_valid_tokens(&store).await.unwrap_err();
        assert!(matches!(err, AuthError::Revoked));
        assert!(err.to_string().contains("openmi login"));
    }

    #[test]
    fn default_api_url_is_production_supabase() {
        assert_eq!(
            DEFAULT_API_BASE_URL,
            "https://umjlmqeghfrfpduhkits.supabase.co"
        );
        // Explicit override always wins and is normalized.
        assert_eq!(
            resolve_api_base_url(Some("https://example.test/"), None),
            "https://example.test"
        );
    }

    #[test]
    fn anon_key_only_defaults_for_production_project() {
        assert_eq!(
            select_anon_key(DEFAULT_API_BASE_URL, None).as_deref(),
            Some(DEFAULT_SUPABASE_ANON_KEY)
        );
        assert_eq!(
            select_anon_key(&format!("{DEFAULT_API_BASE_URL}/"), None).as_deref(),
            Some(DEFAULT_SUPABASE_ANON_KEY)
        );
        assert_eq!(select_anon_key("http://127.0.0.1:9999", None), None);
        assert_eq!(
            select_anon_key("http://127.0.0.1:9999", Some("custom".into())).as_deref(),
            Some("custom")
        );
        assert_eq!(
            select_anon_key(DEFAULT_API_BASE_URL, Some("  ".into())).as_deref(),
            Some(DEFAULT_SUPABASE_ANON_KEY)
        );
    }

    #[tokio::test]
    async fn whoami_sends_apikey_and_bearer_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
            let body = r#"{"id":"u-1","email":"a@b.c","aud":"authenticated"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let client = DeviceAuthClient::new(format!("http://{addr}"))
            .unwrap()
            .with_anon_key(Some("sb_publishable_test".into()));
        let me = client.whoami("user-jwt").await.unwrap();
        assert_eq!(me.id, "u-1");
        assert_eq!(me.plan, None);

        let raw = rx.await.unwrap().to_ascii_lowercase();
        assert!(raw.starts_with("get /auth/v1/user "));
        assert!(raw.contains("apikey: sb_publishable_test"));
        assert!(raw.contains("authorization: bearer user-jwt"));
    }
}
