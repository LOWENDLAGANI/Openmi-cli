use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use openmi_checkpoints::CheckpointStore;
use openmi_core::{
    AgentSession, ApprovalDecision, ApprovalHandler, ApprovalRequest, PermissionMode, ProxyConfig,
};

/// Auto-approve handler used when `--yes` is supplied in headless mode.
pub struct AutoApproveHandler;

#[async_trait]
impl ApprovalHandler for AutoApproveHandler {
    async fn request_approval(&self, _req: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Approve
    }
}

/// Non-interactive Ask-mode handler that denies and explains why on stderr.
pub struct NonInteractiveDenyHandler;

#[async_trait]
impl ApprovalHandler for NonInteractiveDenyHandler {
    async fn request_approval(&self, req: ApprovalRequest) -> ApprovalDecision {
        let mut stderr = io::stderr();
        let _ = writeln!(
            stderr,
            "Non-interactive terminal in Ask mode: denying tool `{}` (re-run with `--yes` to auto-approve).",
            req.tool_name
        );
        ApprovalDecision::Deny
    }
}

/// Stdin-backed interactive approval handler used when stdin is a TTY.
pub struct StdinApprovalHandler;

#[async_trait]
impl ApprovalHandler for StdinApprovalHandler {
    async fn request_approval(&self, req: ApprovalRequest) -> ApprovalDecision {
        let prompt = format!(
            "Approve tool `{}` ({})? [y/N/a]: ",
            req.tool_name, req.summary
        );
        let result = tokio::task::spawn_blocking(move || {
            let mut stderr = io::stderr();
            let _ = write!(stderr, "{prompt}");
            let _ = stderr.flush();
            let mut line = String::new();
            if io::stdin().lock().read_line(&mut line).is_err() {
                return ApprovalDecision::Deny;
            }
            match line.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => ApprovalDecision::Approve,
                "a" | "always" => ApprovalDecision::ApproveAlways,
                _ => ApprovalDecision::Deny,
            }
        })
        .await;

        result.unwrap_or(ApprovalDecision::Deny)
    }
}

/// Select the appropriate headless approval handler based on `--yes` and TTY state.
pub fn select_headless_approval_handler(auto_yes: bool) -> Arc<dyn ApprovalHandler> {
    if auto_yes {
        Arc::new(AutoApproveHandler)
    } else if io::stdin().is_terminal() {
        Arc::new(StdinApprovalHandler)
    } else {
        Arc::new(NonInteractiveDenyHandler)
    }
}

/// Read project instructions from `OPENMI.md`, falling back to `CLAUDE.md` or `AGENTS.md`
/// so repositories configured for Claude Code or Codex work out of the box.
pub fn load_project_instructions(workspace_root: &Path) -> Option<String> {
    for filename in ["OPENMI.md", "CLAUDE.md", "AGENTS.md"] {
        let candidate = workspace_root.join(filename);
        if let Some(content) = std::fs::read_to_string(candidate)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return Some(content);
        }
    }
    None
}

/// Generate a friendly starter `OPENMI.md` tailored to the detected project stack.
pub fn generate_starter_openmi_md(workspace_root: &Path) -> String {
    let project_name = workspace_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Project");

    let mut stack_hints: Vec<&str> = Vec::new();
    let mut commands: Vec<&str> = Vec::new();

    if workspace_root.join("Cargo.toml").exists() {
        stack_hints.push("Rust (Cargo)");
        commands.push("- Build & test: `cargo test`");
    }
    if workspace_root.join("package.json").exists() {
        stack_hints.push("Node.js / TypeScript");
        commands.push("- Install deps: `npm install` (or `pnpm install`)");
        commands.push("- Run tests: `npm test`");
    }
    if workspace_root.join("pyproject.toml").exists()
        || workspace_root.join("requirements.txt").exists()
    {
        stack_hints.push("Python");
        commands.push("- Run tests: `pytest`");
    }
    if workspace_root.join("go.mod").exists() {
        stack_hints.push("Go");
        commands.push("- Run tests: `go test ./...`");
    }

    let stack_line = if stack_hints.is_empty() {
        "Describe your tech stack and project structure here.".to_string()
    } else {
        format!("Detected stack: {}", stack_hints.join(", "))
    };

    let commands_block = if commands.is_empty() {
        "- Add your build, lint, and test commands here.".to_string()
    } else {
        commands.join("\n")
    };

    format!(
        "# {project_name} — Openmi Project Guide\n\n\
         Openmi automatically reads this file (as well as `CLAUDE.md` / `AGENTS.md`) at the start of every chat.\n\n\
         ## Overview\n\
         {stack_line}\n\n\
         ## Common Commands\n\
         {commands_block}\n\n\
         ## Preferences\n\
         - Keep explanations clear and concise.\n\
         - Run relevant tests after making code changes.\n"
    )
}

/// Construct an `AgentSession` wired with a shared `CheckpointStore` before-write hook,
/// optional proxy configuration, project instructions (`OPENMI.md` / `CLAUDE.md` / `AGENTS.md`),
/// persistent external project `memory.md`, merged permission rules, and approval handler.
pub fn build_session_with_options(
    cwd: &Path,
    model: &str,
    permission_mode: PermissionMode,
    checkpoints: Arc<Mutex<CheckpointStore>>,
    approval_handler: Arc<dyn ApprovalHandler>,
    proxy_config: Option<ProxyConfig>,
) -> Result<AgentSession> {
    let mut session = AgentSession::new(cwd, model)?;
    session.set_permission_mode(permission_mode);
    session.set_approval_handler(approval_handler);
    session.set_proxy_config(proxy_config);

    let merged_settings = crate::settings::UserSettings::load_merged(session.workspace_root());
    session.set_permission_rules(merged_settings.permissions);
    session.set_hooks(merged_settings.hooks);

    let memory_path = openmi_platform::AppPaths::resolve()
        .ok()
        .map(|paths| paths.project_memory_file(session.workspace_root()));
    let project_memory = memory_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    session.set_memory_file(memory_path);

    let mut prompt_sections = Vec::new();
    if let Some(instructions) = load_project_instructions(session.workspace_root()) {
        prompt_sections.push(instructions);
    }
    if let Some(mem) = project_memory {
        prompt_sections.push(format!("## Persistent Project Memory\n{mem}"));
    }
    if !prompt_sections.is_empty() {
        session.set_system_prompt(Some(prompt_sections.join("\n\n")));
    }

    let cp = Arc::clone(&checkpoints);
    session.set_before_write_hook(Arc::new(move |path| {
        let mut guard = cp
            .lock()
            .map_err(|_| "checkpoint store mutex poisoned".to_string())?;
        guard
            .snapshot_before_write(path)
            .map_err(|e| format!("checkpoint snapshot failed: {e}"))
    }));

    Ok(session)
}

/// Convenience helper preserved for `smoke-test` and simple offline sessions.
pub fn build_session(
    cwd: &Path,
    model: &str,
    permission_mode: PermissionMode,
    checkpoints: Arc<Mutex<CheckpointStore>>,
) -> Result<AgentSession> {
    build_session_with_options(
        cwd,
        model,
        permission_mode,
        checkpoints,
        select_headless_approval_handler(false),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_openmi_md_into_session_system_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("OPENMI.md"),
            "# Project Rules\nAlways run cargo test.\n",
        )
        .unwrap();

        let cp = Arc::new(Mutex::new(CheckpointStore::new()));
        let session = build_session(tmp.path(), "openmi-default", PermissionMode::Ask, cp).unwrap();
        assert_eq!(
            session.system_prompt(),
            Some("# Project Rules\nAlways run cargo test.")
        );
    }

    #[test]
    fn falls_back_to_claude_md_and_agents_md() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "Agent instructions").unwrap();
        assert_eq!(
            load_project_instructions(tmp.path()).as_deref(),
            Some("Agent instructions")
        );

        std::fs::write(tmp.path().join("CLAUDE.md"), "Claude instructions").unwrap();
        assert_eq!(
            load_project_instructions(tmp.path()).as_deref(),
            Some("Claude instructions")
        );

        std::fs::write(tmp.path().join("OPENMI.md"), "Openmi instructions").unwrap();
        assert_eq!(
            load_project_instructions(tmp.path()).as_deref(),
            Some("Openmi instructions")
        );
    }
}
