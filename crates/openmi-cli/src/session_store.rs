use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use openmi_core::Message;
use openmi_platform::{atomic_write, AppPaths};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::store::now_unix_secs;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSession {
    pub id: String,
    pub model: String,
    pub cwd: PathBuf,
    pub updated_at: u64,
    pub messages: Vec<Message>,
}

/// Manages persisted conversation sessions under `~/.local/share/openmi/sessions`.
#[derive(Debug, Clone)]
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_store() -> Result<Self> {
        let paths = AppPaths::resolve()?;
        Ok(Self::new(paths.sessions_dir()))
    }

    pub fn new_session_id() -> String {
        Uuid::new_v4().to_string()
    }

    pub fn save(&self, session: &SavedSession) -> Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        let path = self.session_path(&session.id);
        let bytes = serde_json::to_vec_pretty(session)?;
        atomic_write(&path, &bytes)?;
        Ok(path)
    }

    pub fn load_by_id(&self, id: &str) -> Result<SavedSession> {
        let path = self.session_path(id);
        let raw = fs::read(&path).map_err(|_| anyhow!("session `{id}` not found"))?;
        Ok(serde_json::from_slice(&raw)?)
    }

    /// Load the most recently updated session (preferring sessions matching `cwd`, else any).
    pub fn load_latest(&self, cwd: Option<&Path>) -> Result<Option<SavedSession>> {
        if !self.dir.exists() {
            return Ok(None);
        }
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = fs::read(&path) {
                if let Ok(parsed) = serde_json::from_slice::<SavedSession>(&raw) {
                    sessions.push(parsed);
                }
            }
        }
        sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

        if let Some(target_cwd) = cwd {
            if let Some(matched) = sessions.iter().find(|s| s.cwd == target_cwd) {
                return Ok(Some(matched.clone()));
            }
        }
        Ok(sessions.into_iter().next())
    }

    /// List recent non-empty sessions (current folder's sessions first, newest first).
    pub fn list_recent(&self, cwd: Option<&Path>, limit: usize) -> Result<Vec<SavedSession>> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = fs::read(&path) {
                if let Ok(parsed) = serde_json::from_slice::<SavedSession>(&raw) {
                    if !parsed.messages.is_empty() {
                        sessions.push(parsed);
                    }
                }
            }
        }
        sessions.sort_by(|a, b| {
            let a_match = cwd.map(|c| a.cwd == c).unwrap_or(false);
            let b_match = cwd.map(|c| b.cwd == c).unwrap_or(false);
            b_match
                .cmp(&a_match)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        sessions.truncate(limit);
        Ok(sessions)
    }

    pub fn make_snapshot(
        id: String,
        model: String,
        cwd: PathBuf,
        messages: Vec<Message>,
    ) -> SavedSession {
        SavedSession {
            id,
            model,
            cwd,
            updated_at: now_unix_secs(),
            messages,
        }
    }

    fn session_path(&self, id: &str) -> PathBuf {
        let safe_id: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        self.dir.join(format!("{safe_id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmi_core::Role;

    #[test]
    fn saves_and_resumes_session_by_id_and_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path().join("sessions"));
        let ws = PathBuf::from("/tmp/project-a");

        let s1 = SavedSession {
            id: "sess-1".into(),
            model: "openmi-default".into(),
            cwd: ws.clone(),
            updated_at: 100,
            messages: vec![Message {
                role: Role::User,
                content: "first".into(),
            }],
        };
        let s2 = SavedSession {
            id: "sess-2".into(),
            model: "gpt-4.1".into(),
            cwd: ws.clone(),
            updated_at: 200,
            messages: vec![Message {
                role: Role::User,
                content: "second".into(),
            }],
        };

        store.save(&s1).unwrap();
        store.save(&s2).unwrap();

        let by_id = store.load_by_id("sess-1").unwrap();
        assert_eq!(by_id.messages[0].content, "first");

        let latest = store.load_latest(Some(&ws)).unwrap().unwrap();
        assert_eq!(latest.id, "sess-2");
        assert_eq!(latest.model, "gpt-4.1");
    }
}
