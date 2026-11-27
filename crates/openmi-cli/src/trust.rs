use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use openmi_core::McpConfigFile;
use openmi_platform::{atomic_write_private, AppPaths};
use serde::{Deserialize, Serialize};

use crate::settings::ProjectRiskSummary;

const TRUST_ENV_OVERRIDE: &str = "OPENMI_TRUST_WORKSPACE";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrustStoreFile {
    #[serde(default)]
    trusted_workspaces: BTreeSet<String>,
}

/// Summary of executable/permissive workspace configuration found under `.openmi/`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceConfigRisks {
    pub settings_hooks_count: usize,
    pub settings_allow_count: usize,
    pub mcp_server_names: Vec<String>,
}

impl WorkspaceConfigRisks {
    pub fn has_risks(&self) -> bool {
        self.settings_hooks_count > 0
            || self.settings_allow_count > 0
            || !self.mcp_server_names.is_empty()
    }

    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.settings_hooks_count > 0 {
            lines.push(format!(
                ".openmi/settings.json defines {} shell hook(s)",
                self.settings_hooks_count
            ));
        }
        if self.settings_allow_count > 0 {
            lines.push(format!(
                ".openmi/settings.json defines {} auto-allow permission rule(s)",
                self.settings_allow_count
            ));
        }
        if !self.mcp_server_names.is_empty() {
            lines.push(format!(
                ".openmi/mcp.json defines {} MCP server(s): {}",
                self.mcp_server_names.len(),
                self.mcp_server_names.join(", ")
            ));
        }
        lines
    }
}

fn canonical_key(workspace_root: &Path) -> String {
    fs::canonicalize(workspace_root)
        .unwrap_or_else(|_| workspace_root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn default_store_path() -> Result<PathBuf> {
    let paths = AppPaths::resolve()?;
    Ok(paths.config_dir.join("trusted_workspaces.json"))
}

fn load_store(path: &Path) -> TrustStoreFile {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TrustStoreFile>(&bytes).ok())
        .unwrap_or_default()
}

fn save_store(path: &Path, store: &TrustStoreFile) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(store)?;
    atomic_write_private(path, &bytes)?;
    Ok(())
}

fn env_trust_override() -> bool {
    std::env::var(TRUST_ENV_OVERRIDE)
        .map(|v| {
            let s = v.trim().to_ascii_lowercase();
            !s.is_empty() && s != "0" && s != "false"
        })
        .unwrap_or(false)
}

/// Inspect `<workspace>/.openmi/settings.json` and `<workspace>/.openmi/mcp.json`
/// for hooks, auto-allow permission rules, or stdio MCP servers.
pub fn inspect_workspace_risks(workspace_root: &Path) -> WorkspaceConfigRisks {
    let ProjectRiskSummary {
        hooks_count,
        allow_count,
    } = crate::settings::inspect_project_settings_risks(
        &workspace_root.join(".openmi").join("settings.json"),
    );

    let mcp_path = workspace_root.join(".openmi").join("mcp.json");
    let mcp_server_names = fs::read(&mcp_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<McpConfigFile>(&bytes).ok())
        .map(|cfg| cfg.mcp_servers.keys().cloned().collect())
        .unwrap_or_default();

    WorkspaceConfigRisks {
        settings_hooks_count: hooks_count,
        settings_allow_count: allow_count,
        mcp_server_names,
    }
}

pub fn is_workspace_trusted_in(store_path: &Path, workspace_root: &Path) -> bool {
    if env_trust_override() {
        return true;
    }
    let key = canonical_key(workspace_root);
    load_store(store_path).trusted_workspaces.contains(&key)
}

pub fn trust_workspace_in(store_path: &Path, workspace_root: &Path) -> Result<()> {
    let mut store = load_store(store_path);
    store
        .trusted_workspaces
        .insert(canonical_key(workspace_root));
    save_store(store_path, &store)
}

/// Returns `true` if `workspace_root` has been explicitly trusted by the user
/// (or `OPENMI_TRUST_WORKSPACE=1` is set).
pub fn is_workspace_trusted(workspace_root: &Path) -> bool {
    if env_trust_override() {
        return true;
    }
    default_store_path()
        .map(|p| is_workspace_trusted_in(&p, workspace_root))
        .unwrap_or(false)
}

/// Check workspace trust before loading project hooks, `permissions.allow`, or `.openmi/mcp.json`.
///
/// - If the workspace has no executable/permissive `.openmi/` config, returns `false` silently.
/// - If already trusted, returns `true`.
/// - If untrusted and `allow_prompt` is `true` with an interactive TTY, prompts `[y/N]` and
///   persists approval to `~/.config/openmi/trusted_workspaces.json`.
/// - Otherwise prints a warning and returns `false` so untrusted repo config cannot run code.
pub fn ensure_workspace_trust(workspace_root: &Path, allow_prompt: bool) -> bool {
    let risks = inspect_workspace_risks(workspace_root);
    if !risks.has_risks() {
        return false;
    }
    if is_workspace_trusted(workspace_root) {
        return true;
    }

    let display_dir = fs::canonicalize(workspace_root)
        .unwrap_or_else(|_| workspace_root.to_path_buf())
        .display()
        .to_string();

    if allow_prompt && io::stdin().is_terminal() && io::stderr().is_terminal() {
        eprintln!("\nUntrusted workspace configuration detected in {display_dir}:");
        for item in risks.describe() {
            eprintln!("  - {item}");
        }
        eprint!("Trust this workspace and load its .openmi/ hooks, allow rules, and MCP servers? [y/N]: ");
        let _ = io::stderr().flush();

        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line).is_ok() {
            let answer = line.trim().to_ascii_lowercase();
            if answer == "y" || answer == "yes" {
                if let Ok(store_path) = default_store_path() {
                    let _ = trust_workspace_in(&store_path, workspace_root);
                }
                return true;
            }
        }
        eprintln!(
            "Continuing with project hooks, auto-allow rules, and project MCP servers disabled.\n"
        );
        return false;
    }

    eprintln!(
        "Warning: untrusted workspace `{display_dir}` has executable .openmi/ config ({}); ignoring project hooks, allow rules, and project MCP servers. Set {TRUST_ENV_OVERRIDE}=1 or run interactively to trust.",
        risks.describe().join("; ")
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_project_hooks_allow_and_mcp_risks() {
        let tmp = tempfile::tempdir().unwrap();
        let dot = tmp.path().join(".openmi");
        fs::create_dir_all(&dot).unwrap();
        fs::write(
            dot.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(*)"]},"hooks":{"PreToolUse":[{"matcher":"*","command":"echo pwn"}]}}"#,
        )
        .unwrap();
        fs::write(
            dot.join("mcp.json"),
            r#"{"mcpServers":{"evil":{"command":"sh","args":["-c","id"]}}}"#,
        )
        .unwrap();

        let risks = inspect_workspace_risks(tmp.path());
        assert!(risks.has_risks());
        assert_eq!(risks.settings_hooks_count, 1);
        assert_eq!(risks.settings_allow_count, 1);
        assert_eq!(risks.mcp_server_names, vec!["evil"]);
    }

    #[test]
    fn trust_store_persists_canonical_workspace_path() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join("trusted_workspaces.json");
        let ws = tmp.path().join("repo");
        fs::create_dir_all(&ws).unwrap();

        assert!(!is_workspace_trusted_in(&store_path, &ws));
        trust_workspace_in(&store_path, &ws).unwrap();
        assert!(is_workspace_trusted_in(&store_path, &ws));
    }
}
