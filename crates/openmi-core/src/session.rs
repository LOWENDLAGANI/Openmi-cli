use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::background::BackgroundTaskManager;
use crate::events::{AgentEvent, ControlState, TodoItem, TodoStatus};
use crate::hooks::{
    run_post_tool_hooks, run_pre_tool_hooks, run_stop_hooks, HookOutcome, HooksConfig,
};
use crate::llm::{stream_step, ProxyConfig};
use crate::permissions::{
    evaluate_permission, ApprovalDecision, ApprovalHandler, ApprovalRequest, AskUserHandler,
    AskUserRequest, DenyAllApprovalHandler, NonInteractiveAskUserHandler, PermissionMode,
    PermissionOutcome, PermissionRules,
};
use crate::registry::ToolRegistry;
use crate::tools::execute_tool;
use openmi_platform::{atomic_write, PathGuard, PathGuardError};

pub const FREE_DEFAULT_MAX_STEPS: u32 = 40;
pub const PAID_DEFAULT_MAX_STEPS: u32 = 70;
pub const DEFAULT_MAX_STEPS: u32 = FREE_DEFAULT_MAX_STEPS;

const EXPLORE_SUBAGENT_TOOLS: &[&str] = &["read_file", "list_dir", "grep", "glob", "web_fetch"];
const WORKER_SUBAGENT_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep",
    "glob",
    "web_fetch",
    "edit_file",
    "write_file",
    "delete_file",
    "move_file",
    "run_command",
    "read_task_output",
];

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("turn cancelled")]
    Cancelled,

    #[error("authentication required or expired (HTTP 401/403)")]
    Unauthorized,

    #[error("insufficient credits (HTTP 402)")]
    OutOfCredits,

    #[error("loop cap exceeded after {0} steps")]
    LoopCapExceeded(u32),

    #[error("path guard rejected access: {0}")]
    PathGuard(#[from] PathGuardError),

    #[error("before-write hook failed: {0}")]
    BeforeWriteHook(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("tool error: {0}")]
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Base64-encoded image attachment sent alongside a user prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub mime_type: String,
    pub data_base64: String,
    #[serde(default)]
    pub label: String,
}

/// Standard RFC 4648 base64 encoder (no external dependency required).
pub fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ImageAttachment>,
}

impl Message {
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            attachments: Vec::new(),
        }
    }
}

/// Callback invoked right before any workspace file write (used for checkpoints).
pub type BeforeWriteHook = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

const PLAN_MODE_SYSTEM_PROMPT: &str = "\
PLAN MODE ACTIVE (read-only): Explore the codebase using read_file, list_dir, grep, and glob. \
Do NOT attempt to edit files or run commands. Produce a clear, concrete step-by-step implementation \
plan listing exact file paths and changes, then ask the user to approve or run `/plan off` to begin implementing.";

/// Result of compacting conversation history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactReport {
    pub messages_before: usize,
    pub messages_after: usize,
    pub chars_before: usize,
    pub chars_after: usize,
    pub summary: String,
}

/// UI-agnostic agent session state and turn runner.
pub struct AgentSession {
    model: String,
    permission_mode: PermissionMode,
    permission_rules: PermissionRules,
    hooks: HooksConfig,
    plan_mode: bool,
    pre_plan_mode: Option<PermissionMode>,
    path_guard: PathGuard,
    history: Vec<Message>,
    system_prompt: Option<String>,
    approval_handler: Arc<dyn ApprovalHandler>,
    ask_user_handler: Arc<dyn AskUserHandler>,
    before_write_hook: Option<BeforeWriteHook>,
    proxy_config: Option<ProxyConfig>,
    max_steps: u32,
    always_approved_tools: HashSet<String>,
    tool_registry: ToolRegistry,
    todos: Vec<TodoItem>,
    memory_file: Option<PathBuf>,
    allowed_tools: Option<Vec<String>>,
    subagent_depth: u8,
    bg_tasks: BackgroundTaskManager,
}

impl AgentSession {
    pub fn new(cwd: impl AsRef<Path>, model: impl Into<String>) -> Result<Self, CoreError> {
        let path_guard = PathGuard::new(cwd)?;
        Ok(Self {
            model: model.into(),
            permission_mode: PermissionMode::Ask,
            permission_rules: PermissionRules::default(),
            hooks: HooksConfig::default(),
            plan_mode: false,
            pre_plan_mode: None,
            path_guard,
            history: Vec::new(),
            system_prompt: None,
            approval_handler: Arc::new(DenyAllApprovalHandler),
            ask_user_handler: Arc::new(NonInteractiveAskUserHandler),
            before_write_hook: None,
            proxy_config: None,
            max_steps: DEFAULT_MAX_STEPS,
            always_approved_tools: HashSet::new(),
            tool_registry: ToolRegistry::new(),
            todos: Vec::new(),
            memory_file: None,
            allowed_tools: None,
            subagent_depth: 0,
            bg_tasks: BackgroundTaskManager::new(),
        })
    }

    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tool_registry
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
        if self.plan_mode && mode != PermissionMode::ReadOnly {
            self.plan_mode = false;
            self.pre_plan_mode = None;
        }
        self.permission_mode = mode;
    }

    pub fn plan_mode(&self) -> bool {
        self.plan_mode
    }

    /// Enable or disable Plan Mode. Entering Plan Mode saves the current permission mode
    /// and switches to `ReadOnly`; leaving restores the saved mode.
    pub fn set_plan_mode(&mut self, enabled: bool) -> PermissionMode {
        if enabled && !self.plan_mode {
            self.pre_plan_mode = Some(self.permission_mode);
            self.permission_mode = PermissionMode::ReadOnly;
            self.plan_mode = true;
        } else if !enabled && self.plan_mode {
            self.plan_mode = false;
            if let Some(prev) = self.pre_plan_mode.take() {
                self.permission_mode = prev;
            }
        }
        self.permission_mode
    }

    pub fn set_approval_handler(&mut self, handler: Arc<dyn ApprovalHandler>) {
        self.approval_handler = handler;
    }

    pub fn set_ask_user_handler(&mut self, handler: Arc<dyn AskUserHandler>) {
        self.ask_user_handler = handler;
    }

    pub fn set_hooks(&mut self, hooks: HooksConfig) {
        self.hooks = hooks;
    }

    pub fn hooks(&self) -> &HooksConfig {
        &self.hooks
    }

    pub fn set_permission_rules(&mut self, rules: PermissionRules) {
        self.permission_rules = rules;
    }

    pub fn permission_rules(&self) -> &PermissionRules {
        &self.permission_rules
    }

    pub fn todos(&self) -> &[TodoItem] {
        &self.todos
    }

    pub fn set_todos(&mut self, todos: Vec<TodoItem>) {
        self.todos = todos;
    }

    pub fn set_memory_file(&mut self, path: Option<PathBuf>) {
        self.memory_file = path;
    }

    pub fn memory_file(&self) -> Option<&Path> {
        self.memory_file.as_deref()
    }

    pub fn set_allowed_tools(&mut self, tools: Option<Vec<String>>) {
        self.allowed_tools = tools;
    }

    pub fn set_before_write_hook(&mut self, hook: BeforeWriteHook) {
        self.before_write_hook = Some(hook);
    }

    pub fn set_proxy_config(&mut self, config: Option<ProxyConfig>) {
        self.proxy_config = config;
    }

    pub fn set_max_steps(&mut self, max_steps: u32) {
        self.max_steps = max_steps.max(1);
    }

    pub fn set_system_prompt(&mut self, prompt: Option<String>) {
        self.system_prompt = prompt;
    }

    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.always_approved_tools.clear();
        self.todos.clear();
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }

    pub fn set_history(&mut self, messages: Vec<Message>) {
        self.history = messages;
    }

    pub fn estimated_context_chars(&self) -> usize {
        self.history.iter().map(|m| m.content.len()).sum()
    }

    /// Compact conversation history into a concise handoff summary to free context space.
    pub async fn compact_history(
        &mut self,
        focus: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<CompactReport, CoreError> {
        let messages_before = self.history.len();
        let chars_before = self.estimated_context_chars();
        if messages_before <= 2 {
            return Ok(CompactReport {
                messages_before,
                messages_after: messages_before,
                chars_before,
                chars_after: chars_before,
                summary: "Conversation is already short; nothing to compact.".to_string(),
            });
        }

        let summary = if let Some(proxy) = self.proxy_config.clone() {
            let mut transcript = String::new();
            for m in &self.history {
                let tag = match m.role {
                    Role::System => "SYSTEM",
                    Role::User => "USER",
                    Role::Assistant => "ASSISTANT",
                    Role::Tool => "TOOL",
                };
                let snippet: String = m.content.chars().take(1500).collect();
                transcript.push_str(&format!("[{tag}]\n{snippet}\n\n"));
            }
            let focus_clause = focus
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|f| format!("\nPrioritize preserving details about: {f}\n"))
                .unwrap_or_default();

            let req_messages = vec![
                Message::text(
                    Role::System,
                    "You are a concise technical summarizer. Do not call any tools. Produce a structured handoff summary in markdown with: Goal, Key Files & Edits, Decisions, and Next Steps.",
                ),
                Message::text(
                    Role::User,
                    format!(
                        "Summarize this coding session so we can continue seamlessly with minimal tokens.{focus_clause}\n---\n{transcript}"
                    ),
                ),
            ];
            let (sink_tx, _sink_rx) = mpsc::channel(64);
            let step =
                stream_step(&proxy, &self.model, &req_messages, &[], None, &sink_tx, cancel)
                    .await?;
            let text = step.text.trim().to_string();
            if text.is_empty() {
                build_local_compact_digest(&self.history, focus)
            } else {
                text
            }
        } else {
            build_local_compact_digest(&self.history, focus)
        };

        self.history = vec![
            Message::text(
                Role::User,
                format!("[Compacted conversation context]\n\n{summary}"),
            ),
            Message::text(
                Role::Assistant,
                "Understood. I have the compacted context and am ready to continue.",
            ),
        ];

        let messages_after = self.history.len();
        let chars_after = self.estimated_context_chars();
        Ok(CompactReport {
            messages_before,
            messages_after,
            chars_before,
            chars_after,
            summary,
        })
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

    fn build_request_messages(&self) -> Vec<Message> {
        let mut msgs = Vec::with_capacity(self.history.len() + 3);
        if let Some(sys) = &self.system_prompt {
            msgs.push(Message::text(Role::System, sys.clone()));
        }
        if self.plan_mode {
            msgs.push(Message::text(Role::System, PLAN_MODE_SYSTEM_PROMPT));
        }
        if !self.todos.is_empty() {
            let mut list = String::from("Current structured task list (`todo_write`):\n");
            for item in &self.todos {
                let box_str = match item.status {
                    TodoStatus::Pending => "[ ]",
                    TodoStatus::InProgress => "[~]",
                    TodoStatus::Completed => "[x]",
                };
                list.push_str(&format!("- {box_str} ({}) {}\n", item.id, item.content));
            }
            msgs.push(Message::text(Role::System, list));
        }
        msgs.extend(self.history.iter().cloned());
        msgs
    }

    /// Run a single turn, streaming `AgentEvent`s to `event_tx` and honoring `cancel`.
    ///
    /// When `proxy_config` is configured, streams from the managed SSE endpoint and executes
    /// requested tools up to `max_steps`. When unconfigured (e.g. `smoke-test`), runs a local
    /// deterministic baseline step.
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        self.run_turn_with_attachments(prompt, Vec::new(), event_tx, cancel)
            .await
    }

    /// Run a single turn with optional multimodal image attachments.
    pub async fn run_turn_with_attachments(
        &mut self,
        prompt: &str,
        attachments: Vec<ImageAttachment>,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        self.history.push(Message {
            role: Role::User,
            content: prompt.to_string(),
            attachments,
        });

        let _ = event_tx
            .send(AgentEvent::ControlStateChanged {
                state: ControlState::Running,
            })
            .await;

        let result = if let Some(proxy) = self.proxy_config.clone() {
            self.run_proxy_loop(&proxy, &event_tx, &cancel).await
        } else {
            self.run_offline_baseline(prompt, &event_tx, &cancel).await
        };

        let _ = event_tx
            .send(AgentEvent::ControlStateChanged {
                state: ControlState::Idle,
            })
            .await;

        result
    }

    async fn run_offline_baseline(
        &mut self,
        prompt: &str,
        event_tx: &mpsc::Sender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), CoreError> {
        let _ = event_tx.send(AgentEvent::StepStarted { step: 1 }).await;

        if cancel.is_cancelled() {
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

        self.history.push(Message::text(Role::Assistant, reply));

        let _ = event_tx
            .send(AgentEvent::TurnCompleted {
                finish_reason: "stop".to_string(),
            })
            .await;
        Ok(())
    }

    async fn run_proxy_loop(
        &mut self,
        proxy: &ProxyConfig,
        event_tx: &mpsc::Sender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), CoreError> {
        for step in 1..=self.max_steps {
            if cancel.is_cancelled() {
                return Err(CoreError::Cancelled);
            }

            let _ = event_tx.send(AgentEvent::StepStarted { step }).await;

            let req_messages = self.build_request_messages();
            let extra_tools = if self.allowed_tools.is_some() {
                Vec::new()
            } else {
                self.tool_registry.specs()
            };
            let resp = stream_step(
                proxy,
                &self.model,
                &req_messages,
                &extra_tools,
                self.allowed_tools.as_deref(),
                event_tx,
                cancel,
            )
            .await?;

            if !resp.text.is_empty() {
                self.history
                    .push(Message::text(Role::Assistant, resp.text.clone()));
            }

            if resp.tool_calls.is_empty() {
                run_stop_hooks(
                    &self.hooks.stop,
                    &resp.finish_reason,
                    self.path_guard.root(),
                    cancel,
                )
                .await;
                let _ = event_tx
                    .send(AgentEvent::TurnCompleted {
                        finish_reason: resp.finish_reason,
                    })
                    .await;
                return Ok(());
            }

            for call in resp.tool_calls {
                if cancel.is_cancelled() {
                    return Err(CoreError::Cancelled);
                }

                // Enforce sub-agent / skill tool allowlist if configured.
                if let Some(allowed) = &self.allowed_tools {
                    if !allowed.iter().any(|t| t == &call.tool_name) {
                        let denied_msg = format!(
                            "tool `{}` is not permitted in this isolated context",
                            call.tool_name
                        );
                        let _ = event_tx
                            .send(AgentEvent::ToolCompleted {
                                call_id: call.call_id.clone(),
                                tool_name: call.tool_name.clone(),
                                ok: false,
                                output: denied_msg.clone(),
                            })
                            .await;
                        self.history.push(Message::text(
                            Role::Tool,
                            format!("tool `{}` denied: {denied_msg}", call.tool_name),
                        ));
                        continue;
                    }
                }

                let outcome = self.evaluate_tool_permission(&call.tool_name, &call.arguments_json);
                let require_approval = match outcome {
                    PermissionOutcome::Allow => false,
                    PermissionOutcome::Ask => {
                        // Never skip approval via `always_approved_tools` for sensitive paths or explicit `ask` rules.
                        let is_explicit_ask = self.permission_rules.ask.iter().any(|r| {
                            crate::permissions::matches_permission_rule(
                                r,
                                &call.tool_name,
                                &call.arguments_json,
                            )
                        });
                        let touches_sensitive = serde_json::from_str::<serde_json::Value>(
                            &call.arguments_json,
                        )
                        .ok()
                        .map(|v| {
                            ["path", "from", "to"].iter().any(|k| {
                                v.get(*k)
                                    .and_then(|s| s.as_str())
                                    .is_some_and(crate::permissions::is_sensitive_path)
                            })
                        })
                        .unwrap_or(false);

                        if is_explicit_ask || touches_sensitive {
                            true
                        } else {
                            !self.always_approved_tools.contains(&call.tool_name)
                        }
                    }
                    PermissionOutcome::Deny(denied_msg) => {
                        let _ = event_tx
                            .send(AgentEvent::ToolCompleted {
                                call_id: call.call_id.clone(),
                                tool_name: call.tool_name.clone(),
                                ok: false,
                                output: denied_msg.clone(),
                            })
                            .await;
                        self.history.push(Message::text(
                            Role::Tool,
                            format!("tool `{}` denied: {denied_msg}", call.tool_name),
                        ));
                        continue;
                    }
                };

                if require_approval {
                    let summary = format!("{} {}", call.tool_name, call.arguments_json);
                    let _ = event_tx
                        .send(AgentEvent::ControlStateChanged {
                            state: ControlState::WaitingForApproval,
                        })
                        .await;
                    let _ = event_tx
                        .send(AgentEvent::ApprovalRequired {
                            call_id: call.call_id.clone(),
                            tool_name: call.tool_name.clone(),
                            summary: summary.clone(),
                        })
                        .await;

                    let decision = tokio::select! {
                        _ = cancel.cancelled() => return Err(CoreError::Cancelled),
                        d = self.approval_handler.request_approval(ApprovalRequest {
                            call_id: call.call_id.clone(),
                            tool_name: call.tool_name.clone(),
                            summary,
                        }) => d,
                    };

                    let _ = event_tx
                        .send(AgentEvent::ControlStateChanged {
                            state: ControlState::Running,
                        })
                        .await;

                    match decision {
                        ApprovalDecision::Approve => {}
                        ApprovalDecision::ApproveAlways => {
                            self.always_approved_tools.insert(call.tool_name.clone());
                        }
                        ApprovalDecision::Deny => {
                            let msg = format!("user denied tool `{}`", call.tool_name);
                            let _ = event_tx
                                .send(AgentEvent::ToolCompleted {
                                    call_id: call.call_id.clone(),
                                    tool_name: call.tool_name.clone(),
                                    ok: false,
                                    output: msg.clone(),
                                })
                                .await;
                            self.history.push(Message::text(Role::Tool, msg));
                            continue;
                        }
                    }
                }

                // Run PreToolUse lifecycle hooks (non-zero exit blocks execution with stderr feedback).
                if !self.hooks.pre_tool_use.is_empty() {
                    if let HookOutcome::Block(block_msg) = run_pre_tool_hooks(
                        &self.hooks.pre_tool_use,
                        &call.tool_name,
                        &call.arguments_json,
                        self.path_guard.root(),
                        cancel,
                    )
                    .await
                    {
                        let _ = event_tx
                            .send(AgentEvent::ToolCompleted {
                                call_id: call.call_id.clone(),
                                tool_name: call.tool_name.clone(),
                                ok: false,
                                output: block_msg.clone(),
                            })
                            .await;
                        self.history
                            .push(Message::text(Role::Tool, format!("error: {block_msg}")));
                        continue;
                    }
                }

                let _ = event_tx
                    .send(AgentEvent::ToolStarted {
                        call_id: call.call_id.clone(),
                        tool_name: call.tool_name.clone(),
                    })
                    .await;

                let is_bg_command = call.tool_name == "run_command"
                    && serde_json::from_str::<serde_json::Value>(&call.arguments_json)
                        .ok()
                        .and_then(|v| v.get("run_in_background").and_then(|b| b.as_bool()))
                        .unwrap_or(false);

                let exec = if call.tool_name == "todo_write" {
                    self.exec_todo_write(&call.arguments_json, event_tx).await
                } else if call.tool_name == "memory_write" {
                    self.exec_memory_write(&call.arguments_json)
                } else if call.tool_name == "ask_user" {
                    self.exec_ask_user(&call.call_id, &call.arguments_json, event_tx, cancel)
                        .await
                } else if call.tool_name == "spawn_agent" {
                    self.exec_spawn_agent(&call.arguments_json, proxy, cancel)
                        .await
                } else if is_bg_command {
                    self.exec_run_background(&call.arguments_json)
                } else if call.tool_name == "read_task_output" {
                    self.exec_read_task_output(&call.arguments_json)
                } else if let Some(registered) = self.tool_registry.get(&call.tool_name) {
                    let handler = Arc::clone(&registered.handler);
                    handler.call(&call.arguments_json, cancel).await
                } else {
                    execute_tool(
                        &call.tool_name,
                        &call.arguments_json,
                        &self.path_guard,
                        self.before_write_hook.as_ref(),
                        cancel,
                    )
                    .await
                };

                match exec {
                    Ok(mut output) => {
                        if !self.hooks.post_tool_use.is_empty() {
                            if let Some(note) = run_post_tool_hooks(
                                &self.hooks.post_tool_use,
                                &call.tool_name,
                                &call.arguments_json,
                                &output,
                                self.path_guard.root(),
                                cancel,
                            )
                            .await
                            {
                                output.push('\n');
                                output.push_str(&note);
                            }
                        }
                        let _ = event_tx
                            .send(AgentEvent::ToolCompleted {
                                call_id: call.call_id.clone(),
                                tool_name: call.tool_name.clone(),
                                ok: true,
                                output: output.clone(),
                            })
                            .await;
                        self.history.push(Message::text(Role::Tool, output));
                    }
                    Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                    Err(err) => {
                        let msg = err.to_string();
                        let _ = event_tx
                            .send(AgentEvent::ToolCompleted {
                                call_id: call.call_id.clone(),
                                tool_name: call.tool_name.clone(),
                                ok: false,
                                output: msg.clone(),
                            })
                            .await;
                        self.history
                            .push(Message::text(Role::Tool, format!("error: {msg}")));
                    }
                }
            }
        }

        let _ = event_tx
            .send(AgentEvent::Error {
                code: "loop_cap".to_string(),
                message: format!("exceeded maximum step limit ({})", self.max_steps),
            })
            .await;
        Err(CoreError::LoopCapExceeded(self.max_steps))
    }

    fn exec_run_background(&mut self, arguments_json: &str) -> Result<String, CoreError> {
        let args: serde_json::Value = serde_json::from_str(arguments_json)
            .map_err(|e| CoreError::Tool(format!("run_command invalid arguments: {e}")))?;
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::Tool("run_command requires `command`".to_string()))?;
        let task_id = self.bg_tasks.spawn(command, self.path_guard.root())?;
        Ok(serde_json::json!({
            "task_id": task_id,
            "status": "running",
            "message": "Background command started. Call read_task_output with this task_id to inspect logs."
        })
        .to_string())
    }

    fn exec_read_task_output(&self, arguments_json: &str) -> Result<String, CoreError> {
        let args: serde_json::Value = serde_json::from_str(arguments_json)
            .map_err(|e| CoreError::Tool(format!("read_task_output invalid arguments: {e}")))?;
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::Tool("read_task_output requires `task_id`".to_string()))?;
        self.bg_tasks.read_output(task_id)
    }

    async fn exec_ask_user(
        &self,
        call_id: &str,
        arguments_json: &str,
        event_tx: &mpsc::Sender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<String, CoreError> {
        let args: serde_json::Value = serde_json::from_str(arguments_json)
            .map_err(|e| CoreError::Tool(format!("ask_user invalid arguments: {e}")))?;
        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Tool("ask_user requires non-empty `question`".to_string()))?
            .to_string();
        let options: Vec<String> = args
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let allow_custom = args
            .get("allow_custom")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let _ = event_tx
            .send(AgentEvent::ControlStateChanged {
                state: ControlState::WaitingForUserInput,
            })
            .await;
        let _ = event_tx
            .send(AgentEvent::UserInputRequired {
                call_id: call_id.to_string(),
                question: question.clone(),
                options: options.clone(),
                allow_custom,
            })
            .await;

        let answer = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            ans = self.ask_user_handler.ask_user(AskUserRequest {
                call_id: call_id.to_string(),
                question,
                options,
                allow_custom,
            }) => ans,
        };

        let _ = event_tx
            .send(AgentEvent::ControlStateChanged {
                state: ControlState::Running,
            })
            .await;

        match answer {
            Some(choice) if !choice.trim().is_empty() => Ok(format!("user selected: {}", choice.trim())),
            _ => Ok("user dismissed prompt or non-interactive environment (no answer provided)".to_string()),
        }
    }

    fn evaluate_tool_permission(&self, tool_name: &str, args_json: &str) -> PermissionOutcome {
        // Explore sub-agents are strictly read-only and never require approval.
        if tool_name == "spawn_agent" {
            let subagent_type = serde_json::from_str::<serde_json::Value>(args_json)
                .ok()
                .and_then(|v| {
                    v.get("subagent_type")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_ascii_lowercase())
                })
                .unwrap_or_else(|| "explore".to_string());
            if subagent_type == "explore" {
                return PermissionOutcome::Allow;
            }
        }

        if let Some(mcp_res) = self
            .tool_registry
            .needs_approval(tool_name, self.permission_mode)
        {
            // Still apply explicit deny rules to MCP tools.
            for rule in &self.permission_rules.deny {
                if crate::permissions::matches_permission_rule(rule, tool_name, args_json) {
                    return PermissionOutcome::Deny(format!(
                        "tool `{tool_name}` blocked by permission deny rule `{rule}`"
                    ));
                }
            }
            return match mcp_res {
                Ok(true) => PermissionOutcome::Ask,
                Ok(false) => PermissionOutcome::Allow,
                Err(msg) => PermissionOutcome::Deny(msg),
            };
        }

        evaluate_permission(
            tool_name,
            args_json,
            self.permission_mode,
            &self.permission_rules,
        )
    }

    async fn exec_todo_write(
        &mut self,
        arguments_json: &str,
        event_tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<String, CoreError> {
        #[derive(Deserialize)]
        struct TodoWriteArgs {
            todos: Vec<TodoItem>,
        }
        let parsed: TodoWriteArgs = serde_json::from_str(arguments_json)
            .map_err(|e| CoreError::Tool(format!("todo_write invalid arguments: {e}")))?;
        self.todos = parsed.todos;
        let _ = event_tx
            .send(AgentEvent::TodosUpdated {
                items: self.todos.clone(),
            })
            .await;
        let completed = self
            .todos
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count();
        let in_progress = self
            .todos
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();
        Ok(format!(
            "updated todo list ({} total, {} in_progress, {} completed)",
            self.todos.len(),
            in_progress,
            completed
        ))
    }

    fn exec_memory_write(&self, arguments_json: &str) -> Result<String, CoreError> {
        let Some(mem_path) = &self.memory_file else {
            return Err(CoreError::Tool(
                "project memory file is not configured for this session".to_string(),
            ));
        };
        let args: serde_json::Value = serde_json::from_str(arguments_json)
            .map_err(|e| CoreError::Tool(format!("memory_write invalid arguments: {e}")))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::Tool("memory_write requires `content`".to_string()))?;
        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("append");

        let next_text = if mode.eq_ignore_ascii_case("replace") {
            content.to_string()
        } else {
            let existing = std::fs::read_to_string(mem_path).unwrap_or_default();
            if existing.trim().is_empty() {
                content.to_string()
            } else {
                format!("{}\n\n{}", existing.trim_end(), content)
            }
        };

        if let Some(parent) = mem_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Tool(format!("memory_write create dir failed: {e}")))?;
        }
        atomic_write(mem_path, next_text.as_bytes())
            .map_err(|e| CoreError::Tool(format!("memory_write failed: {e}")))?;
        Ok(format!(
            "saved project memory ({} bytes, mode={mode})",
            next_text.len()
        ))
    }

    async fn exec_spawn_agent(
        &self,
        arguments_json: &str,
        proxy: &ProxyConfig,
        cancel: &CancellationToken,
    ) -> Result<String, CoreError> {
        if self.subagent_depth >= 1 {
            return Err(CoreError::Tool(
                "nested spawn_agent calls are not permitted".to_string(),
            ));
        }

        let args: serde_json::Value = serde_json::from_str(arguments_json)
            .map_err(|e| CoreError::Tool(format!("spawn_agent invalid arguments: {e}")))?;
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Tool("spawn_agent requires non-empty `prompt`".to_string()))?;
        let subagent_type = args
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("explore")
            .to_ascii_lowercase();
        let max_turns = args
            .get("max_turns")
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).clamp(1, self.max_steps.min(30)))
            .unwrap_or_else(|| 12u32.min(self.max_steps));

        let mut child = AgentSession::new(self.path_guard.root(), self.model.clone())?;
        child.subagent_depth = self.subagent_depth + 1;
        child.set_proxy_config(Some(proxy.clone()));
        child.set_max_steps(max_turns);
        child.set_permission_rules(self.permission_rules.clone());
        child.set_approval_handler(Arc::clone(&self.approval_handler));
        if let Some(hook) = &self.before_write_hook {
            child.set_before_write_hook(Arc::clone(hook));
        }

        match subagent_type.as_str() {
            "explore" => {
                child.set_permission_mode(PermissionMode::ReadOnly);
                child.set_allowed_tools(Some(
                    EXPLORE_SUBAGENT_TOOLS
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                ));
                child.set_system_prompt(Some(
                    "You are a focused read-only codebase exploration sub-agent. \
                     Use read_file, list_dir, grep, and glob to answer the prompt, then return \
                     a concise, structured summary with exact file paths and findings."
                        .to_string(),
                ));
            }
            "worker" => {
                child.set_permission_mode(self.permission_mode);
                child.set_allowed_tools(Some(
                    WORKER_SUBAGENT_TOOLS
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                ));
                child.set_system_prompt(self.system_prompt.clone());
            }
            other => {
                return Err(CoreError::Tool(format!(
                    "unknown subagent_type `{other}` (expected `explore` or `worker`)"
                )));
            }
        }

        let (sub_tx, mut sub_rx) = mpsc::channel(64);
        let drain_task = tokio::spawn(async move { while sub_rx.recv().await.is_some() {} });
        let run_res = Box::pin(child.run_turn(prompt, sub_tx, cancel.clone())).await;
        let _ = drain_task.await;

        match run_res {
            Ok(()) => {
                let summary = child
                    .history()
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::Assistant && !m.content.trim().is_empty())
                    .map(|m| m.content.trim().to_string())
                    .unwrap_or_else(|| "(sub-agent completed with no text output)".to_string());
                Ok(format!("[subagent:{subagent_type} summary]\n{summary}"))
            }
            Err(CoreError::Cancelled) => Err(CoreError::Cancelled),
            Err(CoreError::LoopCapExceeded(n)) => {
                let partial = child
                    .history()
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::Assistant && !m.content.trim().is_empty())
                    .map(|m| m.content.trim().to_string())
                    .unwrap_or_default();
                Ok(format!(
                    "[subagent:{subagent_type} reached max_turns={n}]\n{partial}"
                ))
            }
            Err(err) => Err(err),
        }
    }
}

fn build_local_compact_digest(history: &[Message], focus: Option<&str>) -> String {
    let mut lines = Vec::new();
    if let Some(f) = focus.map(str::trim).filter(|s| !s.is_empty()) {
        lines.push(format!("Focus: {f}"));
    }
    lines.push(format!(
        "Condensed {} previous messages:",
        history.len()
    ));
    for msg in history {
        if matches!(msg.role, Role::User | Role::Assistant) {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                _ => continue,
            };
            let snippet: String = msg
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(180)
                .collect();
            if !snippet.is_empty() {
                lines.push(format!("- {role}: {snippet}"));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_mock_sse_server(
        responder: impl Fn(usize) -> (u16, String) + Send + Sync + 'static,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let responder = Arc::new(responder);

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let c = Arc::clone(&counter);
                let r = Arc::clone(&responder);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16384];
                    let _ = socket.read(&mut buf).await;
                    let idx = c.fetch_add(1, Ordering::SeqCst);
                    let (status, body) = r(idx);
                    let content_type = if status == 200 {
                        "text/event-stream"
                    } else {
                        "application/json"
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        format!("http://{addr}/functions/v1/chat")
    }

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

    #[tokio::test]
    async fn proxy_sse_streams_tool_call_and_respects_402_and_loop_cap() {
        let tmp = tempfile::tempdir().unwrap();

        // 1. Tool call write_file + follow-up text delta
        let endpoint = spawn_mock_sse_server(|step| {
            if step == 0 {
                (
                    200,
                    concat!(
                        "data: {\"type\":\"tool_call\",\"call_id\":\"c1\",\"tool_name\":\"write_file\",\"arguments\":{\"path\":\"out.txt\",\"content\":\"hi\"}}\n\n",
                        "data: {\"type\":\"done\",\"finish_reason\":\"tool_calls\"}\n\n"
                    )
                    .to_string(),
                )
            } else {
                (
                    200,
                    concat!(
                        "data: {\"type\":\"text_delta\",\"text\":\"Done!\"}\n\n",
                        "data: {\"type\":\"done\",\"finish_reason\":\"stop\"}\n\n"
                    )
                    .to_string(),
                )
            }
        })
        .await;

        let mut session = AgentSession::new(tmp.path(), "openmi-default").unwrap();
        session.set_permission_mode(PermissionMode::Trusted);
        session.set_proxy_config(Some(ProxyConfig {
            endpoint,
            bearer_token: "tok".to_string(),
        }));

        let (tx, mut rx) = mpsc::channel(32);
        session
            .run_turn("create out.txt", tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out.txt")).unwrap(),
            "hi"
        );

        let mut saw_tool_ok = false;
        while let Some(ev) = rx.recv().await {
            if let AgentEvent::ToolCompleted { ok: true, .. } = ev {
                saw_tool_ok = true;
            }
        }
        assert!(saw_tool_ok);

        // 2. HTTP 402 -> OutOfCredits
        let ep_402 = spawn_mock_sse_server(|_| (402, r#"{"error":"out_of_credits"}"#.into())).await;
        session.set_proxy_config(Some(ProxyConfig {
            endpoint: ep_402,
            bearer_token: "tok".to_string(),
        }));
        let (tx, _rx) = mpsc::channel(8);
        let err = session
            .run_turn("bill me", tx, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::OutOfCredits));

        // 3. Loop cap exceeded
        let ep_loop = spawn_mock_sse_server(|_| {
            (
                200,
                "data: {\"type\":\"tool_call\",\"call_id\":\"c\",\"tool_name\":\"list_dir\",\"arguments\":{\"path\":\".\"}}\n\n".into(),
            )
        })
        .await;
        session.set_max_steps(2);
        session.set_proxy_config(Some(ProxyConfig {
            endpoint: ep_loop,
            bearer_token: "tok".to_string(),
        }));
        let (tx, _rx) = mpsc::channel(16);
        let err = session
            .run_turn("loop forever", tx, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::LoopCapExceeded(2)));
    }

    #[tokio::test]
    async fn cancelling_running_command_kills_child_process() {
        let tmp = tempfile::tempdir().unwrap();
        let sleep_cmd = if cfg!(windows) {
            "ping -n 10 127.0.0.1 >nul"
        } else {
            "sleep 30"
        };
        let payload = format!(
            "data: {{\"type\":\"tool_call\",\"call_id\":\"c1\",\"tool_name\":\"run_command\",\"arguments\":{{\"command\":\"{sleep_cmd}\"}}}}\n\n"
        );
        let endpoint = spawn_mock_sse_server(move |_| (200, payload.clone())).await;

        let mut session = AgentSession::new(tmp.path(), "openmi-default").unwrap();
        session.set_permission_mode(PermissionMode::Trusted);
        session.set_proxy_config(Some(ProxyConfig {
            endpoint,
            bearer_token: "tok".to_string(),
        }));

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let (tx, mut rx) = mpsc::channel(32);

        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if matches!(ev, AgentEvent::ToolStarted { .. }) {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    cancel_clone.cancel();
                    break;
                }
            }
        });

        let err = session.run_turn("run sleep", tx, cancel).await.unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
    }

    #[tokio::test]
    async fn todo_write_emits_event_and_memory_write_persists_externally() {
        let tmp = tempfile::tempdir().unwrap();
        let mem_path = tmp.path().join("external_mem/memory.md");

        let endpoint = spawn_mock_sse_server(|step| match step {
            0 => (
                200,
                concat!(
                    "data: {\"type\":\"tool_call\",\"call_id\":\"t1\",\"tool_name\":\"todo_write\",\"arguments\":{\"todos\":[{\"id\":\"1\",\"content\":\"Audit code\",\"status\":\"in_progress\"},{\"id\":\"2\",\"content\":\"Run tests\",\"status\":\"pending\"}]}}\n\n",
                    "data: {\"type\":\"tool_call\",\"call_id\":\"m1\",\"tool_name\":\"memory_write\",\"arguments\":{\"content\":\"- Always run cargo test\",\"mode\":\"append\"}}\n\n"
                )
                .to_string(),
            ),
            _ => (
                200,
                "data: {\"type\":\"text_delta\",\"text\":\"All set.\"}\n\ndata: {\"type\":\"done\",\"finish_reason\":\"stop\"}\n\n"
                    .to_string(),
            ),
        })
        .await;

        let mut session = AgentSession::new(tmp.path(), "openmi-default").unwrap();
        session.set_permission_mode(PermissionMode::Trusted);
        session.set_memory_file(Some(mem_path.clone()));
        session.set_proxy_config(Some(ProxyConfig {
            endpoint,
            bearer_token: "tok".to_string(),
        }));

        let (tx, mut rx) = mpsc::channel(32);
        let saw_todos = Arc::new(AtomicBool::new(false));
        let saw_clone = Arc::clone(&saw_todos);
        let collector = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if let AgentEvent::TodosUpdated { items } = ev {
                    if items.len() == 2 && items[0].status == TodoStatus::InProgress {
                        saw_clone.store(true, Ordering::SeqCst);
                    }
                }
            }
        });

        session
            .run_turn("track tasks and remember test command", tx, CancellationToken::new())
            .await
            .unwrap();
        let _ = collector.await;

        assert!(saw_todos.load(Ordering::SeqCst));
        assert_eq!(session.todos().len(), 2);
        assert_eq!(
            std::fs::read_to_string(&mem_path).unwrap(),
            "- Always run cargo test"
        );
    }

    #[tokio::test]
    async fn spawn_agent_explore_runs_in_isolated_history() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "secret-finding-123").unwrap();

        let endpoint = spawn_mock_sse_server(|req_idx| match req_idx {
            // Parent step 1: invoke explore sub-agent
            0 => (
                200,
                "data: {\"type\":\"tool_call\",\"call_id\":\"s1\",\"tool_name\":\"spawn_agent\",\"arguments\":{\"prompt\":\"read notes.txt\",\"subagent_type\":\"explore\",\"max_turns\":4}}\n\n"
                    .to_string(),
            ),
            // Child step 1: read_file notes.txt
            1 => (
                200,
                "data: {\"type\":\"tool_call\",\"call_id\":\"c1\",\"tool_name\":\"read_file\",\"arguments\":{\"path\":\"notes.txt\"}}\n\n"
                    .to_string(),
            ),
            // Child step 2: concise summary
            2 => (
                200,
                "data: {\"type\":\"text_delta\",\"text\":\"Found secret-finding-123 in notes.txt\"}\n\ndata: {\"type\":\"done\",\"finish_reason\":\"stop\"}\n\n"
                    .to_string(),
            ),
            // Parent step 2: final answer
            _ => (
                200,
                "data: {\"type\":\"text_delta\",\"text\":\"Done exploring.\"}\n\ndata: {\"type\":\"done\",\"finish_reason\":\"stop\"}\n\n"
                    .to_string(),
            ),
        })
        .await;

        let mut session = AgentSession::new(tmp.path(), "openmi-default").unwrap();
        // Explore sub-agent works even in ReadOnly mode!
        session.set_permission_mode(PermissionMode::ReadOnly);
        session.set_proxy_config(Some(ProxyConfig {
            endpoint,
            bearer_token: "tok".to_string(),
        }));

        let (tx, _rx) = mpsc::channel(32);
        session
            .run_turn("explore workspace", tx, CancellationToken::new())
            .await
            .unwrap();

        // Parent history has only User, Tool(summary), Assistant — NOT the child's verbose read_file turn.
        assert_eq!(session.history().len(), 3);
        assert!(session.history()[1]
            .content
            .contains("[subagent:explore summary]"));
        assert!(session.history()[1]
            .content
            .contains("Found secret-finding-123 in notes.txt"));
    }
}
