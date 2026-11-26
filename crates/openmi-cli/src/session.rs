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

/// Read `OPENMI.md` from `workspace_root` if present (used as project system context).
pub fn load_project_instructions(workspace_root: &Path) -> Option<String> {
    let candidate = workspace_root.join("OPENMI.md");
    std::fs::read_to_string(candidate)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Construct an `AgentSession` wired with a shared `CheckpointStore` before-write hook,
/// optional proxy configuration, project instructions (`OPENMI.md`), and approval handler.
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

    if let Some(instructions) = load_project_instructions(session.workspace_root()) {
        session.set_system_prompt(Some(instructions));
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
