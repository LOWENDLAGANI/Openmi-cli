use async_trait::async_trait;
use openmi_core::{
    ApprovalDecision, ApprovalHandler, ApprovalRequest, AskUserHandler, AskUserRequest,
};
use tokio::sync::{mpsc, oneshot};

/// An approval request awaiting a user decision in the TUI modal popup.
#[derive(Debug)]
pub struct PendingApproval {
    pub request: ApprovalRequest,
    pub responder: oneshot::Sender<ApprovalDecision>,
}

/// UI-agnostic `ApprovalHandler` bridging `AgentSession` to the ratatui event loop via oneshot.
#[derive(Clone)]
pub struct TuiApprovalHandler {
    tx: mpsc::Sender<PendingApproval>,
}

impl TuiApprovalHandler {
    pub fn new(tx: mpsc::Sender<PendingApproval>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl ApprovalHandler for TuiApprovalHandler {
    async fn request_approval(&self, req: ApprovalRequest) -> ApprovalDecision {
        let (resp_tx, resp_rx) = oneshot::channel();
        if self
            .tx
            .send(PendingApproval {
                request: req,
                responder: resp_tx,
            })
            .await
            .is_err()
        {
            return ApprovalDecision::Deny;
        }
        resp_rx.await.unwrap_or(ApprovalDecision::Deny)
    }
}

/// An interactive `ask_user` clarification prompt awaiting user selection in the TUI modal.
#[derive(Debug)]
pub struct PendingAskUser {
    pub request: AskUserRequest,
    pub responder: oneshot::Sender<Option<String>>,
}

/// `AskUserHandler` bridging `AgentSession::exec_ask_user` to the ratatui event loop via oneshot.
#[derive(Clone)]
pub struct TuiAskUserHandler {
    tx: mpsc::Sender<PendingAskUser>,
}

impl TuiAskUserHandler {
    pub fn new(tx: mpsc::Sender<PendingAskUser>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl AskUserHandler for TuiAskUserHandler {
    async fn ask_user(&self, req: AskUserRequest) -> Option<String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        if self
            .tx
            .send(PendingAskUser {
                request: req,
                responder: resp_tx,
            })
            .await
            .is_err()
        {
            return None;
        }
        resp_rx.await.ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tui_approval_handler_round_trips_decision() {
        let (tx, mut rx) = mpsc::channel(4);
        let handler = TuiApprovalHandler::new(tx);

        let task = tokio::spawn(async move {
            handler
                .request_approval(ApprovalRequest {
                    call_id: "c1".into(),
                    tool_name: "write_file".into(),
                    summary: "write_file a.txt".into(),
                })
                .await
        });

        let pending = rx.recv().await.expect("pending approval");
        assert_eq!(pending.request.tool_name, "write_file");
        pending
            .responder
            .send(ApprovalDecision::ApproveAlways)
            .unwrap();

        let decision = task.await.unwrap();
        assert_eq!(decision, ApprovalDecision::ApproveAlways);
    }
}
