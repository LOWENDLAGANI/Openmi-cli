use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::{AgentEvent, ControlState};
use crate::permissions::{ApprovalHandler, DenyAllApprovalHandler, PermissionMode};
use openmi_platform::{PathGuard, PathGuardError};

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("turn cancelled")]
    Cancelled,

    #[error("path guard rejected access: {0}")]
    PathGuard(#[from] PathGuardError),

    #[error("before-write hook failed: {0}")]
    BeforeWriteHook(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// Callback invoked right before any workspace file write (used for checkpoints).
pub type BeforeWriteHook = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// UI-agnostic agent session state and turn runner.
pub struct AgentSession {
    model: String,
    permission_mode: PermissionMode,
    path_guard: PathGuard,
    history: Vec<Message>,
    approval_handler: Arc<dyn ApprovalHandler>,
    before_write_hook: Option<BeforeWriteHook>,
}

impl AgentSession {
    pub fn new(cwd: impl AsRef<Path>, model: impl Into<String>) -> Result<Self, CoreError> {
        let path_guard = PathGuard::new(cwd)?;
        Ok(Self {
            model: model.into(),
            permission_mode: PermissionMode::Ask,
            path_guard,
            history: Vec::new(),
            approval_handler: Arc::new(DenyAllApprovalHandler),
            before_write_hook: None,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    pub fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_mode = mode;
    }

    pub fn set_approval_handler(&mut self, handler: Arc<dyn ApprovalHandler>) {
        self.approval_handler = handler;
    }

    pub fn set_before_write_hook(&mut self, hook: BeforeWriteHook) {
        self.before_write_hook = Some(hook);
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }

    pub fn workspace_root(&self) -> &Path {
        self.path_guard.root()
    }

    pub fn path_guard(&self) -> &PathGuard {
        &self.path_guard
    }

    pub fn approval_handler(&self) -> &Arc<dyn ApprovalHandler> {
        &self.approval_handler
    }

    /// Resolve a target file path through `PathGuard` and invoke the `before_write_hook` if set.
    pub fn prepare_write(&self, candidate: impl AsRef<Path>) -> Result<PathBuf, CoreError> {
        let resolved = self.path_guard.resolve_for_write(candidate)?;
        if let Some(hook) = &self.before_write_hook {
            hook(&resolved).map_err(CoreError::BeforeWriteHook)?;
        }
        Ok(resolved)
    }

    /// Run a single turn, streaming `AgentEvent`s to `event_tx` and honoring `cancel`.
    ///
    /// In M0 this provides the deterministic local baseline loop; M2 wires the managed Supabase
    /// SSE proxy transport into this turn loop without changing the UI-facing contract.
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        self.history.push(Message {
            role: Role::User,
            content: prompt.to_string(),
        });

        let _ = event_tx
            .send(AgentEvent::ControlStateChanged {
                state: ControlState::Running,
            })
            .await;
        let _ = event_tx.send(AgentEvent::StepStarted { step: 1 }).await;

        if cancel.is_cancelled() {
            let _ = event_tx
                .send(AgentEvent::ControlStateChanged {
                    state: ControlState::Idle,
                })
                .await;
            return Err(CoreError::Cancelled);
        }

        let reply = format!(
            "[openmi baseline | model={} | mode={:?}] Received: {}",
            self.model, self.permission_mode, prompt
        );

        let _ = event_tx
            .send(AgentEvent::TextDelta {
                text: reply.clone(),
            })
            .await;

        self.history.push(Message {
            role: Role::Assistant,
            content: reply,
        });

        let _ = event_tx
            .send(AgentEvent::TurnCompleted {
                finish_reason: "stop".to_string(),
            })
            .await;
        let _ = event_tx
            .send(AgentEvent::ControlStateChanged {
                state: ControlState::Idle,
            })
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn run_turn_emits_expected_event_sequence() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = AgentSession::new(tmp.path(), "default-model").unwrap();
        session.set_model("gpt-4.1");
        session.set_permission_mode(PermissionMode::Trusted);

        let (tx, mut rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();

        session.run_turn("hello world", tx, cancel).await.unwrap();

        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }

        assert!(matches!(
            events.first(),
            Some(AgentEvent::ControlStateChanged {
                state: ControlState::Running
            })
        ));
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnCompleted { .. })));
        assert_eq!(session.history().len(), 2);

        session.clear_history();
        assert!(session.history().is_empty());
    }

    #[tokio::test]
    async fn run_turn_honors_cancellation() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = AgentSession::new(tmp.path(), "default-model").unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = session.run_turn("cancel me", tx, cancel).await.unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
    }

    #[test]
    fn prepare_write_invokes_before_write_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = AgentSession::new(tmp.path(), "default-model").unwrap();
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = Arc::clone(&called);

        session.set_before_write_hook(Arc::new(move |_path| {
            called_clone.store(true, Ordering::SeqCst);
            Ok(())
        }));

        let resolved = session.prepare_write("notes.txt").unwrap();
        assert!(resolved.ends_with("notes.txt"));
        assert!(called.load(Ordering::SeqCst));
    }
}
