use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use openmi_checkpoints::CheckpointStore;
use openmi_core::{
    AgentSession, ApprovalDecision, ApprovalHandler, ApprovalRequest, PermissionMode,
};

/// Stdin-backed interactive approval handler used by headless `run`.
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

/// Construct an `AgentSession` wired with a shared `CheckpointStore` before-write hook.
pub fn build_session(
    cwd: &Path,
    model: &str,
    permission_mode: PermissionMode,
    checkpoints: Arc<Mutex<CheckpointStore>>,
) -> Result<AgentSession> {
    let mut session = AgentSession::new(cwd, model)?;
    session.set_permission_mode(permission_mode);
    session.set_approval_handler(Arc::new(StdinApprovalHandler));

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
