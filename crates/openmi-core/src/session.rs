use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::{AgentEvent, ControlState};
use crate::llm::{stream_step, ProxyConfig};
use crate::permissions::{
    ApprovalDecision, ApprovalHandler, ApprovalRequest, DenyAllApprovalHandler, PermissionMode,
};
use crate::registry::ToolRegistry;
use crate::tools::{execute_tool, needs_approval};
use openmi_platform::{PathGuard, PathGuardError};

pub const FREE_DEFAULT_MAX_STEPS: u32 = 40;
pub const PAID_DEFAULT_MAX_STEPS: u32 = 70;
pub const DEFAULT_MAX_STEPS: u32 = FREE_DEFAULT_MAX_STEPS;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
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
    plan_mode: bool,
    pre_plan_mode: Option<PermissionMode>,
    path_guard: PathGuard,
    history: Vec<Message>,
    system_prompt: Option<String>,
    approval_handler: Arc<dyn ApprovalHandler>,
    before_write_hook: Option<BeforeWriteHook>,
    proxy_config: Option<ProxyConfig>,
    max_steps: u32,
    always_approved_tools: HashSet<String>,
    tool_registry: ToolRegistry,
}

impl AgentSession {
    pub fn new(cwd: impl AsRef<Path>, model: impl Into<String>) -> Result<Self, CoreError> {
        let path_guard = PathGuard::new(cwd)?;
        Ok(Self {
            model: model.into(),
            permission_mode: PermissionMode::Ask,
            plan_mode: false,
            pre_plan_mode: None,
            path_guard,
            history: Vec::new(),
            system_prompt: None,
            approval_handler: Arc::new(DenyAllApprovalHandler),
            before_write_hook: None,
            proxy_config: None,
            max_steps: DEFAULT_MAX_STEPS,
            always_approved_tools: HashSet::new(),
            tool_registry: ToolRegistry::new(),
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
                Message {
                    role: Role::System,
                    content: "You are a concise technical summarizer. Do not call any tools. Produce a structured handoff summary in markdown with: Goal, Key Files & Edits, Decisions, and Next Steps.".to_string(),
                },
                Message {
                    role: Role::User,
                    content: format!(
                        "Summarize this coding session so we can continue seamlessly with minimal tokens.{focus_clause}\n---\n{transcript}"
                    ),
                },
            ];
            let (sink_tx, _sink_rx) = mpsc::channel(64);
            let step = stream_step(&proxy, &self.model, &req_messages, &[], &sink_tx, cancel).await?;
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
            Message {
                role: Role::User,
                content: format!("[Compacted conversation context]\n\n{summary}"),
            },
            Message {
                role: Role::Assistant,
                content: "Understood. I have the compacted context and am ready to continue."
                    .to_string(),
            },
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
        let mut msgs = Vec::with_capacity(self.history.len() + 2);
        if let Some(sys) = &self.system_prompt {
            msgs.push(Message {
                role: Role::System,
                content: sys.clone(),
            });
        }
        if self.plan_mode {
            msgs.push(Message {
                role: Role::System,
                content: PLAN_MODE_SYSTEM_PROMPT.to_string(),
            });
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
        self.history.push(Message {
            role: Role::User,
            content: prompt.to_string(),
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

        self.history.push(Message {
            role: Role::Assistant,
            content: reply,
        });

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
            let extra_tools = self.tool_registry.specs();
            let resp = stream_step(
                proxy,
                &self.model,
                &req_messages,
                &extra_tools,
                event_tx,
                cancel,
            )
            .await?;

            if !resp.text.is_empty() {
                self.history.push(Message {
                    role: Role::Assistant,
                    content: resp.text.clone(),
                });
            }

            if resp.tool_calls.is_empty() {
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

                let approval_check = self
                    .tool_registry
                    .needs_approval(&call.tool_name, self.permission_mode)
                    .unwrap_or_else(|| needs_approval(&call.tool_name, self.permission_mode));

                let require_approval = match approval_check {
                    Ok(v) => v && !self.always_approved_tools.contains(&call.tool_name),
                    Err(denied_msg) => {
                        let _ = event_tx
                            .send(AgentEvent::ToolCompleted {
                                call_id: call.call_id.clone(),
                                tool_name: call.tool_name.clone(),
                                ok: false,
                                output: denied_msg.clone(),
                            })
                            .await;
                        self.history.push(Message {
                            role: Role::Tool,
                            content: format!("tool `{}` denied: {denied_msg}", call.tool_name),
                        });
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
                            self.history.push(Message {
                                role: Role::Tool,
                                content: msg,
                            });
                            continue;
                        }
                    }
                }

                let _ = event_tx
                    .send(AgentEvent::ToolStarted {
                        call_id: call.call_id.clone(),
                        tool_name: call.tool_name.clone(),
                    })
                    .await;

                let exec = if let Some(registered) = self.tool_registry.get(&call.tool_name) {
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
                    Ok(output) => {
                        let _ = event_tx
                            .send(AgentEvent::ToolCompleted {
                                call_id: call.call_id.clone(),
                                tool_name: call.tool_name.clone(),
                                ok: true,
                                output: output.clone(),
                            })
                            .await;
                        self.history.push(Message {
                            role: Role::Tool,
                            content: output,
                        });
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
                        self.history.push(Message {
                            role: Role::Tool,
                            content: format!("error: {msg}"),
                        });
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
}
