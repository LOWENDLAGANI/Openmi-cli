pub mod catalog;
pub mod events;
pub mod permissions;
pub mod session;

pub use catalog::{fetch_credit_status, fetch_models_with_fallback, CreditStatus, ModelInfo};
pub use events::{AgentEvent, ControlState};
pub use permissions::{ApprovalDecision, ApprovalHandler, ApprovalRequest, PermissionMode};
pub use session::{AgentSession, BeforeWriteHook, CoreError, Message, Role};
