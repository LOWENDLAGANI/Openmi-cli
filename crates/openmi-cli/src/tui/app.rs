use std::fs;
use std::path::{Path, PathBuf};

use openmi_core::{
    AgentEvent, ApprovalDecision, ControlState, CreditStatus, Message, ModelInfo, PermissionMode,
    Role,
};

use crate::tui::approval::PendingApproval;
use crate::tui::input::InputState;
use crate::tui::mention::{active_mention_query, collect_workspace_files, fuzzy_match_paths};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptKind {
    User,
    Assistant,
    System,
    Error,
    Diff,
}

#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub kind: TranscriptKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Requested,
    WaitingApproval,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ToolTimelineItem {
    pub call_id: String,
    pub tool_name: String,
    pub status: ToolStatus,
    pub summary: String,
}

/// Typed confirmation modal state before enabling `PermissionMode::FullAccess`.
#[derive(Debug, Default)]
pub struct FullAccessModalState {
    pub typed: String,
    pub error: Option<String>,
}

/// Interactive `/model` selector modal state.
#[derive(Debug, Clone)]
pub struct ModelSelectorState {
    pub models: Vec<ModelInfo>,
    pub selected: usize,
}

impl ModelSelectorState {
    pub fn new(models: Vec<ModelInfo>, current_model: &str) -> Self {
        let selected = models
            .iter()
            .position(|m| m.id == current_model)
            .unwrap_or(0);
        Self { models, selected }
    }

    pub fn move_up(&mut self) {
        if self.models.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.models.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.models.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.models.len();
    }

    pub fn current(&self) -> Option<&ModelInfo> {
        self.models.get(self.selected)
    }
}

/// Complete state of the interactive TUI (UI-only; core stays UI-agnostic).
pub struct TuiApp {
    pub model: String,
    pub mode: PermissionMode,
    pub credits: Option<i64>,
    pub credit_status: Option<CreditStatus>,
    pub cwd: PathBuf,
    pub branch: String,
    pub control_state: ControlState,
    pub transcript: Vec<TranscriptEntry>,
    pub tools: Vec<ToolTimelineItem>,
    pub input: InputState,
    pub scroll_from_bottom: u16,
    pub pending_approval: Option<PendingApproval>,
    pub full_access_modal: Option<FullAccessModalState>,
    pub model_selector: Option<ModelSelectorState>,
    pub workspace_files: Vec<String>,
    pub mention_suggestions: Vec<String>,
    pub should_quit: bool,
    pub status_hint: String,
    pub tick: u64,
    /// Tracks whether the user already pressed Esc/Ctrl-C once while idle (press again to quit).
    pub armed_quit: bool,
}

impl TuiApp {
    pub fn new(model: String, mode: PermissionMode, cwd: PathBuf) -> Self {
        let branch = detect_git_branch(&cwd);
        let workspace_files = collect_workspace_files(&cwd, 2000);
        let mut app = Self {
            model,
            mode,
            credits: None,
            credit_status: None,
            cwd,
            branch,
            control_state: ControlState::Idle,
            transcript: Vec::new(),
            tools: Vec::new(),
            input: InputState::new(),
            scroll_from_bottom: 0,
            pending_approval: None,
            full_access_modal: None,
            model_selector: None,
            workspace_files,
            mention_suggestions: Vec::new(),
            should_quit: false,
            status_hint: "Enter: send | /model: pick model | /help: commands | Esc: cancel/quit"
                .to_string(),
            tick: 0,
            armed_quit: false,
        };
        app.push_system(
            "Welcome to Openmi CLI. Type a prompt, `/model` to pick a model, `/help` for commands, or Esc twice to quit.",
        );
        app
    }

    pub fn apply_credit_status(&mut self, status: CreditStatus) {
        self.credits = Some(status.balance);
        self.credit_status = Some(status);
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.control_state, ControlState::Idle)
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        self.transcript.push(TranscriptEntry {
            kind: TranscriptKind::User,
            text: text.into(),
        });
        self.scroll_from_bottom = 0;
    }

    pub fn push_system(&mut self, text: impl Into<String>) {
        self.transcript.push(TranscriptEntry {
            kind: TranscriptKind::System,
            text: text.into(),
        });
        self.scroll_from_bottom = 0;
    }

    pub fn push_diff(&mut self, text: impl Into<String>) {
        self.transcript.push(TranscriptEntry {
            kind: TranscriptKind::Diff,
            text: text.into(),
        });
        self.scroll_from_bottom = 0;
    }

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.transcript.push(TranscriptEntry {
            kind: TranscriptKind::Error,
            text: text.into(),
        });
        self.scroll_from_bottom = 0;
    }

    /// Populate transcript from restored `AgentSession` messages (`--resume`).
    pub fn hydrate_from_history(&mut self, messages: &[Message]) {
        for msg in messages {
            match msg.role {
                Role::User => self.push_user(msg.content.clone()),
                Role::Assistant => self.transcript.push(TranscriptEntry {
                    kind: TranscriptKind::Assistant,
                    text: msg.content.clone(),
                }),
                Role::System | Role::Tool => {}
            }
        }
    }

    /// Recompute `@path` fuzzy completion suggestions based on the current cursor position.
    pub fn refresh_mention_suggestions(&mut self) {
        if let Some((_, query)) = active_mention_query(self.input.text(), self.input.cursor_byte())
        {
            self.mention_suggestions = fuzzy_match_paths(&self.workspace_files, query, 5);
        } else {
            self.mention_suggestions.clear();
        }
    }

    /// Accept the top `@path` fuzzy match on `Tab`.
    pub fn apply_top_mention_completion(&mut self) -> bool {
        let Some(top) = self.mention_suggestions.first().cloned() else {
            return false;
        };
        let cursor = self.input.cursor_byte();
        if let Some((start, _)) = active_mention_query(self.input.text(), cursor) {
            self.input.replace_range(start, cursor, &format!("@{top} "));
            self.mention_suggestions.clear();
            return true;
        }
        false
    }

    pub fn scroll_up(&mut self, lines: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lines);
    }

    pub fn apply_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::ControlStateChanged { state } => {
                self.control_state = state;
                if matches!(state, ControlState::Idle) {
                    self.armed_quit = false;
                }
            }
            AgentEvent::StepStarted { step } => {
                self.status_hint = format!("Running step {step}...");
            }
            AgentEvent::TextDelta { text } => {
                if let Some(last) = self.transcript.last_mut() {
                    if last.kind == TranscriptKind::Assistant {
                        last.text.push_str(&text);
                        return;
                    }
                }
                self.transcript.push(TranscriptEntry {
                    kind: TranscriptKind::Assistant,
                    text,
                });
            }
            AgentEvent::ToolCallRequested {
                call_id,
                tool_name,
                arguments_json,
            } => {
                self.tools.push(ToolTimelineItem {
                    call_id,
                    tool_name,
                    status: ToolStatus::Requested,
                    summary: arguments_json,
                });
            }
            AgentEvent::ApprovalRequired {
                call_id,
                tool_name,
                summary,
            } => {
                self.update_or_push_tool(call_id, tool_name, ToolStatus::WaitingApproval, summary);
            }
            AgentEvent::ToolStarted { call_id, tool_name } => {
                self.update_or_push_tool(call_id, tool_name, ToolStatus::Running, "running".into());
            }
            AgentEvent::ToolCompleted {
                call_id,
                tool_name,
                ok,
                output,
            } => {
                let first_line = output.lines().next().unwrap_or("").to_string();
                let status = if ok {
                    ToolStatus::Succeeded
                } else {
                    ToolStatus::Failed
                };
                self.update_or_push_tool(call_id, tool_name, status, first_line);
            }
            AgentEvent::TurnCompleted { finish_reason } => {
                self.status_hint = format!("Ready ({finish_reason})");
            }
            AgentEvent::Error { code, message } => {
                self.push_error(format!("[{code}] {message}"));
                self.status_hint = format!("Error: {code}");
            }
        }
    }

    fn update_or_push_tool(
        &mut self,
        call_id: String,
        tool_name: String,
        status: ToolStatus,
        summary: String,
    ) {
        if let Some(item) = self.tools.iter_mut().rev().find(|t| t.call_id == call_id) {
            item.status = status;
            item.summary = summary;
        } else {
            self.tools.push(ToolTimelineItem {
                call_id,
                tool_name,
                status,
                summary,
            });
        }
    }

    /// Resolve a pending approval popup with `decision`.
    pub fn resolve_approval(&mut self, decision: ApprovalDecision) -> bool {
        if let Some(pending) = self.pending_approval.take() {
            let label = match decision {
                ApprovalDecision::Approve => "approved",
                ApprovalDecision::ApproveAlways => "approved (always this session)",
                ApprovalDecision::Deny => "denied",
            };
            self.status_hint = format!("Tool `{}` {label}", pending.request.tool_name);
            let _ = pending.responder.send(decision);
            return true;
        }
        false
    }

    /// Handle Esc or Ctrl-C: closes modals, cancels active turn, or quits on 2nd press.
    /// Returns `true` if the active turn should be cancelled now.
    pub fn handle_interrupt(&mut self) -> bool {
        if self.model_selector.is_some() {
            self.model_selector = None;
            return false;
        }

        if self.full_access_modal.is_some() {
            self.full_access_modal = None;
            self.push_system("Cancelled FullAccess mode change.");
            return false;
        }

        if self.pending_approval.is_some() {
            self.resolve_approval(ApprovalDecision::Deny);
            return true;
        }

        if self.is_busy() {
            if self.armed_quit {
                self.should_quit = true;
                return true;
            }
            self.armed_quit = true;
            self.status_hint = "Cancelling turn... press Esc/Ctrl-C again to quit".to_string();
            return true;
        }

        if self.armed_quit {
            self.should_quit = true;
        } else {
            self.armed_quit = true;
            self.status_hint = "Press Esc or Ctrl-C again to quit".to_string();
        }
        false
    }
}

/// Lightweight `.git/HEAD` reader that avoids spawning `git` on every draw.
pub fn detect_git_branch(start_dir: &Path) -> String {
    let mut cur = Some(start_dir);
    while let Some(dir) = cur {
        let head = dir.join(".git").join("HEAD");
        if let Ok(content) = fs::read_to_string(&head) {
            let trimmed = content.trim();
            if let Some(rest) = trimmed.strip_prefix("ref: refs/heads/") {
                return rest.to_string();
            }
            return trimmed.chars().take(7).collect();
        }
        cur = dir.parent();
    }
    "-".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_cancels_first_then_quits_on_second_press() {
        let mut app = TuiApp::new(
            "openmi-default".into(),
            PermissionMode::Ask,
            PathBuf::from("."),
        );
        app.control_state = ControlState::Running;

        assert!(app.handle_interrupt());
        assert!(!app.should_quit);

        assert!(app.handle_interrupt());
        assert!(app.should_quit);
    }

    #[test]
    fn agent_events_accumulate_transcript_and_tools() {
        let mut app = TuiApp::new(
            "openmi-default".into(),
            PermissionMode::Ask,
            PathBuf::from("."),
        );
        app.apply_agent_event(AgentEvent::TextDelta {
            text: "Hello ".into(),
        });
        app.apply_agent_event(AgentEvent::TextDelta {
            text: "world".into(),
        });
        app.apply_agent_event(AgentEvent::ToolStarted {
            call_id: "c1".into(),
            tool_name: "read_file".into(),
        });
        app.apply_agent_event(AgentEvent::ToolCompleted {
            call_id: "c1".into(),
            tool_name: "read_file".into(),
            ok: true,
            output: "ok".into(),
        });

        assert_eq!(app.transcript.last().unwrap().text, "Hello world");
        assert_eq!(app.tools.len(), 1);
        assert_eq!(app.tools[0].status, ToolStatus::Succeeded);
    }

    #[test]
    fn at_mention_tab_completion_replaces_active_token() {
        let mut app = TuiApp::new(
            "openmi-default".into(),
            PermissionMode::Ask,
            PathBuf::from("."),
        );
        app.workspace_files = vec!["Cargo.toml".into(), "src/tui/slash.rs".into()];
        for ch in "check @slash".chars() {
            app.input.insert_char(ch);
        }
        app.refresh_mention_suggestions();
        assert_eq!(app.mention_suggestions, vec!["src/tui/slash.rs"]);
        assert!(app.apply_top_mention_completion());
        assert_eq!(app.input.text(), "check @src/tui/slash.rs ");
    }
}
