pub mod background;
pub mod catalog;
pub mod events;
pub mod hooks;
pub mod llm;
pub mod mcp;
pub mod permissions;
pub mod registry;
pub mod session;
pub mod tools;
pub mod web_fetch;

pub use background::BackgroundTaskManager;
pub use catalog::{
    fetch_credit_status, fetch_models_with_fallback, format_token_count, CreditStatus, ModelInfo,
};
pub use events::{AgentEvent, ControlState, TodoItem, TodoStatus};
pub use hooks::{HookOutcome, HookRule, HooksConfig};
pub use llm::ProxyConfig;
pub use mcp::{
    load_merged_mcp_config, load_merged_mcp_config_with_trust, McpConfigFile, McpError, McpManager,
    McpServerConfig, McpServerStatus, McpStdioClient, McpToolInfo,
};
pub use permissions::{
    evaluate_permission, is_sensitive_path, ApprovalDecision, ApprovalHandler, ApprovalRequest,
    AskUserHandler, AskUserRequest, NonInteractiveAskUserHandler, PermissionMode,
    PermissionOutcome, PermissionRules,
};
pub use registry::{format_mcp_tool_name, DynamicTool, DynamicToolSpec, ToolRegistry};
pub use session::{
    encode_base64, AgentSession, BeforeWriteHook, CompactReport, CoreError, ImageAttachment,
    Message, Role,
};
