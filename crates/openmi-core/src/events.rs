use serde::{Deserialize, Serialize};

/// High-level lifecycle state of the agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlState {
    Idle,
    Running,
    WaitingForApproval,
    Cancelling,
}

/// UI-agnostic event stream emitted by `AgentSession::run_turn`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    StepStarted {
        step: u32,
    },
    TextDelta {
        text: String,
    },
    ToolCallRequested {
        call_id: String,
        tool_name: String,
        arguments_json: String,
    },
    ApprovalRequired {
        call_id: String,
        tool_name: String,
        summary: String,
    },
    ToolStarted {
        call_id: String,
        tool_name: String,
    },
    ToolCompleted {
        call_id: String,
        tool_name: String,
        ok: bool,
        output: String,
    },
    ControlStateChanged {
        state: ControlState,
    },
    TurnCompleted {
        finish_reason: String,
    },
    Error {
        code: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_serializes_with_type_tag() {
        let ev = AgentEvent::TextDelta {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"text_delta\""));
        let decoded: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ev);
    }
}
