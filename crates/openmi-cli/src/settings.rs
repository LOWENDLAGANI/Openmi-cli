use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use openmi_core::{HooksConfig, PermissionMode, PermissionRules};
use openmi_platform::{atomic_write, AppPaths};
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Persistent user preferences saved to `~/.config/openmi/settings.json`
/// and optionally extended by project `.openmi/settings.json`.
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

    /// Granular glob-based permission rules (`allow`, `ask`, `deny`).
    #[serde(default)]
    pub permissions: PermissionRules,

    /// Shell lifecycle hooks (`PreToolUse`, `PostToolUse`, `Stop`).
    #[serde(default)]
    pub hooks: HooksConfig,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            auto_approve: true,
            read_only: false,
            show_tool_activity: true,
            permissions: PermissionRules::default(),
            hooks: HooksConfig::default(),
        }
    }
}

/// Partial project-level `.openmi/settings.json` overlay.
#[derive(Debug, Clone, Default, Deserialize)]
struct ProjectSettingsOverlay {
    #[serde(default)]
    read_only: Option<bool>,
    #[serde(default)]
    permissions: PermissionRules,
    #[serde(default)]
    hooks: HooksConfig,
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

    /// Load global settings and merge project-level `.openmi/settings.json` rules & hooks.
    pub fn load_merged(workspace_root: &Path) -> Self {
        let global = Self::load_default();
        Self::merge_project_file(global, &workspace_root.join(".openmi").join("settings.json"))
    }

    /// Merge a project `.openmi/settings.json` overlay onto `base` settings.
    pub fn merge_project_file(mut base: Self, project_settings_path: &Path) -> Self {
        if let Some(overlay) = fs::read(project_settings_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ProjectSettingsOverlay>(&bytes).ok())
        {
            if overlay.read_only == Some(true) {
                base.read_only = true;
            }
            base.permissions = base.permissions.merge(&overlay.permissions);
            base.hooks = base.hooks.merge(&overlay.hooks);
        }
        base
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
        assert!(defaults.permissions.is_empty());
        assert_eq!(
            defaults.effective_permission_mode(),
            PermissionMode::Trusted
        );

        let updated = UserSettings {
            auto_approve: false,
            read_only: false,
            show_tool_activity: false,
            permissions: PermissionRules {
                allow: vec!["Bash(cargo test *)".into()],
                ask: vec![],
                deny: vec!["Bash(rm -rf *)".into()],
            },
            hooks: HooksConfig::default(),
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
            permissions: PermissionRules::default(),
            hooks: HooksConfig::default(),
        };
        assert_eq!(s.effective_permission_mode(), PermissionMode::ReadOnly);
    }

    #[test]
    fn merges_project_settings_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let project_cfg = tmp.path().join(".openmi/settings.json");
        fs::create_dir_all(project_cfg.parent().unwrap()).unwrap();
        fs::write(
            &project_cfg,
            r#"{"permissions":{"allow":["Bash(git status *)"],"deny":["Edit(.env*)"]}}"#,
        )
        .unwrap();

        let base = UserSettings {
            permissions: PermissionRules {
                deny: vec!["Bash(rm -rf *)".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = UserSettings::merge_project_file(base, &project_cfg);
        assert!(merged
            .permissions
            .allow
            .contains(&"Bash(git status *)".to_string()));
        assert!(merged
            .permissions
            .deny
            .contains(&"Bash(rm -rf *)".to_string()));
        assert!(merged
            .permissions
            .deny
            .contains(&"Edit(.env*)".to_string()));
    }
}
