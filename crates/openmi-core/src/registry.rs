use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::permissions::PermissionMode;
use crate::session::CoreError;

/// Metadata for a dynamically registered tool (e.g. `mcp__<server>__<tool>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicToolSpec {
    pub name: String,
    pub server: String,
    pub remote_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
    /// MCP tools are never auto-approved in `Trusted` unless explicitly allowlisted in config.
    #[serde(default)]
    pub auto_approve_in_trusted: bool,
}

/// Async handler for a dynamically registered tool.
#[async_trait]
pub trait DynamicTool: Send + Sync {
    async fn call(
        &self,
        arguments_json: &str,
        cancel: &CancellationToken,
    ) -> Result<String, CoreError>;
}

#[derive(Clone)]
pub struct RegisteredTool {
    pub spec: DynamicToolSpec,
    pub handler: Arc<dyn DynamicTool>,
}

/// Registry of dynamically registered tools (such as MCP server tools) owned by `AgentSession`.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, spec: DynamicToolSpec, handler: Arc<dyn DynamicTool>) {
        let key = spec.name.clone();
        self.tools.insert(key, RegisteredTool { spec, handler });
    }

    pub fn unregister_server(&mut self, server: &str) {
        self.tools.retain(|_, entry| entry.spec.server != server);
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.get(name)
    }

    pub fn specs(&self) -> Vec<DynamicToolSpec> {
        self.tools.values().map(|e| e.spec.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Evaluate permission rules for a registered MCP/dynamic tool:
    /// - Classified at least `Ask` risk.
    /// - Blocked in `ReadOnly`.
    /// - Requires approval in `Ask`.
    /// - Requires approval in `Trusted` UNLESS explicitly allowlisted (`auto_approve_in_trusted`).
    /// - Allowed in `FullAccess`.
    pub fn needs_approval(&self, name: &str, mode: PermissionMode) -> Option<Result<bool, String>> {
        let entry = self.tools.get(name)?;
        let res = match mode {
            PermissionMode::ReadOnly => Err(format!(
                "MCP tool `{name}` is blocked in ReadOnly permission mode"
            )),
            PermissionMode::Ask => Ok(true),
            PermissionMode::Trusted => Ok(!entry.spec.auto_approve_in_trusted),
            PermissionMode::FullAccess => Ok(false),
        };
        Some(res)
    }
}

/// Format a canonical MCP tool name `mcp__<server>__<tool>`.
pub fn format_mcp_tool_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!("mcp__{}__{}", clean(server), clean(tool))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyTool;

    #[async_trait]
    impl DynamicTool for DummyTool {
        async fn call(
            &self,
            _arguments_json: &str,
            _cancel: &CancellationToken,
        ) -> Result<String, CoreError> {
            Ok("ok".into())
        }
    }

    #[test]
    fn enforces_mcp_risk_policy_in_trusted_and_readonly_modes() {
        let mut reg = ToolRegistry::new();
        let default_name = format_mcp_tool_name("fs", "write_file");
        let allow_name = format_mcp_tool_name("fs", "read_file");

        reg.register(
            DynamicToolSpec {
                name: default_name.clone(),
                server: "fs".into(),
                remote_name: "write_file".into(),
                description: "write".into(),
                input_schema: serde_json::json!({}),
                auto_approve_in_trusted: false,
            },
            Arc::new(DummyTool),
        );
        reg.register(
            DynamicToolSpec {
                name: allow_name.clone(),
                server: "fs".into(),
                remote_name: "read_file".into(),
                description: "read".into(),
                input_schema: serde_json::json!({}),
                auto_approve_in_trusted: true,
            },
            Arc::new(DummyTool),
        );

        assert_eq!(default_name, "mcp__fs__write_file");
        assert!(reg
            .needs_approval(&default_name, PermissionMode::ReadOnly)
            .unwrap()
            .is_err());
        assert!(reg
            .needs_approval(&default_name, PermissionMode::Ask)
            .unwrap()
            .unwrap());
        // Never auto-approved in Trusted unless allowlisted.
        assert!(reg
            .needs_approval(&default_name, PermissionMode::Trusted)
            .unwrap()
            .unwrap());
        // Allowlisted tool auto-approves in Trusted, still asks in Ask.
        assert!(!reg
            .needs_approval(&allow_name, PermissionMode::Trusted)
            .unwrap()
            .unwrap());
        assert!(reg
            .needs_approval(&allow_name, PermissionMode::Ask)
            .unwrap()
            .unwrap());
        assert!(!reg
            .needs_approval(&default_name, PermissionMode::FullAccess)
            .unwrap()
            .unwrap());

        reg.unregister_server("fs");
        assert!(reg.is_empty());
    }
}
