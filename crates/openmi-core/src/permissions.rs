use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Permission modes governing tool execution and write access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    ReadOnly,
    #[default]
    Ask,
    Trusted,
    FullAccess,
}

impl FromStr for PermissionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "read_only" | "readonly" | "read-only" => Ok(Self::ReadOnly),
            "ask" => Ok(Self::Ask),
            "trusted" => Ok(Self::Trusted),
            "full_access" | "fullaccess" | "full-access" => Ok(Self::FullAccess),
            other => Err(format!("unknown permission mode: {other}")),
        }
    }
}

/// Request presented to an `ApprovalHandler` when a tool call requires user consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub call_id: String,
    pub tool_name: String,
    pub summary: String,
}

/// Decision returned by an `ApprovalHandler`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    ApproveAlways,
    Deny,
}

/// UI-provided handler for interactive or non-interactive tool approvals.
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request_approval(&self, req: ApprovalRequest) -> ApprovalDecision;
}

/// Default non-interactive handler that denies all approval prompts.
pub struct DenyAllApprovalHandler;

#[async_trait]
impl ApprovalHandler for DenyAllApprovalHandler {
    async fn request_approval(&self, _req: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_permission_modes() {
        assert_eq!(
            "read-only".parse::<PermissionMode>().unwrap(),
            PermissionMode::ReadOnly
        );
        assert_eq!(
            "ask".parse::<PermissionMode>().unwrap(),
            PermissionMode::Ask
        );
        assert_eq!(
            "trusted".parse::<PermissionMode>().unwrap(),
            PermissionMode::Trusted
        );
        assert_eq!(
            "full-access".parse::<PermissionMode>().unwrap(),
            PermissionMode::FullAccess
        );
        assert!("invalid".parse::<PermissionMode>().is_err());
    }
}
