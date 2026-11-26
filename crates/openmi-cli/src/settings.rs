use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use openmi_core::PermissionMode;
use openmi_platform::{atomic_write, AppPaths};
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Persistent user preferences saved to `~/.config/openmi/settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSettings {
    /// Auto-approve workspace file edits and commands (`PermissionMode::Trusted`).
    /// When `false`, Openmi asks before editing files or running commands (`PermissionMode::Ask`).
    #[serde(default = "default_true")]
    pub auto_approve: bool,

    /// Block all file edits and shell commands (`PermissionMode::ReadOnly`).
    #[serde(default)]
    pub read_only: bool,

    /// Show compact inline tool activity lines (`✓ Read ...`) in the chat stream.
    #[serde(default = "default_true")]
    pub show_tool_activity: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            auto_approve: true,
            read_only: false,
            show_tool_activity: true,
        }
    }
}

impl UserSettings {
    /// Compute the effective `PermissionMode` from the saved toggles.
    pub fn effective_permission_mode(&self) -> PermissionMode {
        if self.read_only {
            PermissionMode::ReadOnly
        } else if self.auto_approve {
            PermissionMode::Trusted
        } else {
            PermissionMode::Ask
        }
    }

    /// Resolve the default settings file path (`~/.config/openmi/settings.json`).
    pub fn default_path() -> Result<PathBuf> {
        let paths = AppPaths::resolve()?;
        Ok(paths.config_dir.join("settings.json"))
    }

    /// Load settings from `path`, returning defaults if the file does not exist or is unreadable.
    pub fn load_from(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .unwrap_or_default()
    }

    /// Load settings from the default config path (never fails; falls back to defaults).
    pub fn load_default() -> Self {
        Self::default_path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    /// Persist settings atomically to `path`.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        atomic_write(path, &bytes)?;
        Ok(())
    }

    /// Persist settings atomically to the default config path.
    pub fn save_default(&self) -> Result<()> {
        let path = Self::default_path()?;
        self.save_to(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_auto_approve_and_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");

        let defaults = UserSettings::load_from(&path);
        assert!(defaults.auto_approve);
        assert!(!defaults.read_only);
        assert!(defaults.show_tool_activity);
        assert_eq!(
            defaults.effective_permission_mode(),
            PermissionMode::Trusted
        );

        let updated = UserSettings {
            auto_approve: false,
            read_only: false,
            show_tool_activity: false,
        };
        updated.save_to(&path).unwrap();

        let loaded = UserSettings::load_from(&path);
        assert_eq!(loaded, updated);
        assert_eq!(loaded.effective_permission_mode(), PermissionMode::Ask);
    }

    #[test]
    fn read_only_takes_precedence_over_auto_approve() {
        let s = UserSettings {
            auto_approve: true,
            read_only: true,
            show_tool_activity: true,
        };
        assert_eq!(s.effective_permission_mode(), PermissionMode::ReadOnly);
    }
}
