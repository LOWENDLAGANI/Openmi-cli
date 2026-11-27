use std::fs;
use std::path::{Path, PathBuf};

use openmi_core::{
    AgentEvent, ApprovalDecision, ControlState, CreditStatus, Message, ModelInfo, PermissionMode,
    Role,
};
use similar::TextDiff;

use crate::session_store::SavedSession;
use crate::settings::UserSettings;
use crate::tui::approval::PendingApproval;
use crate::tui::input::InputState;
use crate::tui::mention::{active_mention_query, collect_workspace_files, fuzzy_match_paths};
use crate::tui::slash::{filter_slash_suggestions, SlashSuggestion};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptKind {
    User,
    Assistant,
    System,
    Error,
    Diff,
    ToolOk,
    ToolErr,
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
    pub target_hint: String,
    pub raw_args: Option<String>,
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

/// An entry inside the interactive `/folder` project picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderEntry {
    /// Confirm the currently viewed directory as the project folder.
    UseCurrent,
    /// Navigate up to the parent directory.
    Parent(PathBuf),
    /// Navigate into a child subdirectory.
    Child { name: String, path: PathBuf },
}

/// Interactive `/folder` picker modal state for choosing a project directory.
#[derive(Debug, Clone)]
pub struct FolderPickerState {
    pub current_dir: PathBuf,
    pub entries: Vec<FolderEntry>,
    pub selected: usize,
    pub error: Option<String>,
}

impl FolderPickerState {
    pub fn new(start_dir: PathBuf) -> Self {
        let canonical = fs::canonicalize(&start_dir).unwrap_or(start_dir);
        let mut state = Self {
            current_dir: canonical,
            entries: Vec::new(),
            selected: 0,
            error: None,
        };
        state.reload();
        state
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        self.entries.push(FolderEntry::UseCurrent);
        if let Some(parent) = self.current_dir.parent() {
            self.entries.push(FolderEntry::Parent(parent.to_path_buf()));
        }

        match fs::read_dir(&self.current_dir) {
            Ok(read_dir) => {
                self.error = None;
                let mut children: Vec<(String, PathBuf)> = read_dir
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let path = e.path();
                        if !path.is_dir() {
                            return None;
                        }
                        let name = e.file_name().to_string_lossy().to_string();
                        if name.starts_with('.')
                            || matches!(
                                name.as_str(),
                                "node_modules" | "target" | "__pycache__" | ".git"
                            )
                        {
                            return None;
                        }
                        Some((name, path))
                    })
                    .collect();
                children.sort_by_key(|a| a.0.to_ascii_lowercase());
                for (name, path) in children {
                    self.entries.push(FolderEntry::Child { name, path });
                }
            }
            Err(err) => {
                self.error = Some(format!("Could not read folder: {err}"));
            }
        }
        self.selected = 0;
    }

    pub fn move_up(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.entries.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.entries.len();
    }

    pub fn go_up(&mut self) {
        if let Some(parent) = self.current_dir.parent().map(Path::to_path_buf) {
            self.current_dir = parent;
            self.reload();
        }
    }

    /// Open a highlighted subfolder / parent, or return `Some(path)` if `UseCurrent` is selected.
    pub fn enter(&mut self) -> Option<PathBuf> {
        match self.entries.get(self.selected).cloned() {
            Some(FolderEntry::UseCurrent) => Some(self.current_dir.clone()),
            Some(FolderEntry::Parent(p)) => {
                self.current_dir = p;
                self.reload();
                None
            }
            Some(FolderEntry::Child { path, .. }) => {
                self.current_dir = path;
                self.reload();
                None
            }
            None => None,
        }
    }

    /// Immediately select the highlighted folder (or current folder) as the project folder.
    pub fn select_highlighted(&self) -> PathBuf {
        match self.entries.get(self.selected) {
            Some(FolderEntry::Child { path, .. }) => path.clone(),
            Some(FolderEntry::Parent(p)) => p.clone(),
            _ => self.current_dir.clone(),
        }
    }
}

/// Interactive `/resume` session picker modal state.
#[derive(Debug, Clone)]
pub struct ResumePickerState {
    pub sessions: Vec<SavedSession>,
    pub selected: usize,
}

impl ResumePickerState {
    pub fn new(sessions: Vec<SavedSession>) -> Self {
        Self {
            sessions,
            selected: 0,
        }
    }

    pub fn move_up(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.sessions.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.sessions.len();
    }

    pub fn current(&self) -> Option<&SavedSession> {
        self.sessions.get(self.selected)
    }
}

/// Interactive `/settings` modal state.
#[derive(Debug, Clone, Default)]
pub struct SettingsModalState {
    pub selected: usize,
}

impl SettingsModalState {
    pub const ITEM_COUNT: usize = 3;

    pub fn move_up(&mut self) {
        if self.selected == 0 {
            self.selected = Self::ITEM_COUNT - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        self.selected = (self.selected + 1) % Self::ITEM_COUNT;
    }
}

/// Complete state of the interactive TUI (UI-only; core stays UI-agnostic).
pub struct TuiApp {
    pub model: String,
    pub mode: PermissionMode,
    pub plan_mode: bool,
    pub settings: UserSettings,
    pub credits: Option<i64>,
    pub credit_status: Option<CreditStatus>,
    pub cwd: PathBuf,
    pub control_state: ControlState,
    pub transcript: Vec<TranscriptEntry>,
    pub tools: Vec<ToolTimelineItem>,
    pub input: InputState,
    pub scroll_from_bottom: u16,
    pub pending_approval: Option<PendingApproval>,
    pub approval_selected: usize,
    pub full_access_modal: Option<FullAccessModalState>,
    pub model_selector: Option<ModelSelectorState>,
    pub folder_picker: Option<FolderPickerState>,
    pub resume_picker: Option<ResumePickerState>,
    pub settings_modal: Option<SettingsModalState>,
    pub workspace_files: Vec<String>,
    pub mention_suggestions: Vec<String>,
    pub slash_suggestions: Vec<SlashSuggestion>,
    pub slash_selected: usize,
    pub should_quit: bool,
    pub status_hint: String,
    pub tick: u64,
    /// Tracks whether the user already pressed Esc/Ctrl-C once while idle (press again to quit).
    pub armed_quit: bool,
    /// Model ID -> user-facing display name (from the live catalog).
    pub model_names: std::collections::HashMap<String, String>,
}

impl TuiApp {
    pub fn new(model: String, mode: PermissionMode, cwd: PathBuf) -> Self {
        let canonical = fs::canonicalize(&cwd).unwrap_or(cwd);
        let workspace_files = collect_workspace_files(&canonical, 2000);
        Self {
            model,
            mode,
            plan_mode: false,
            settings: UserSettings::default(),
            credits: None,
            credit_status: None,
            cwd: canonical,
            control_state: ControlState::Idle,
            transcript: Vec::new(),
            tools: Vec::new(),
            input: InputState::new(),
            scroll_from_bottom: 0,
            pending_approval: None,
            approval_selected: 0,
            full_access_modal: None,
            model_selector: None,
            folder_picker: None,
            resume_picker: None,
            settings_modal: None,
            workspace_files,
            mention_suggestions: Vec::new(),
            slash_suggestions: Vec::new(),
            slash_selected: 0,
            should_quit: false,
            status_hint: String::new(),
            tick: 0,
            armed_quit: false,
            model_names: std::collections::HashMap::new(),
        }
    }

    /// Toggle the currently highlighted item in `/settings` and return the new `PermissionMode`
    /// if the toggle changed permissions.
    pub fn toggle_selected_setting(&mut self) -> Option<PermissionMode> {
        let idx = self.settings_modal.as_ref()?.selected;
        match idx {
            0 => {
                self.settings.auto_approve = !self.settings.auto_approve;
                if self.settings.auto_approve {
                    self.settings.read_only = false;
                }
                self.mode = self.settings.effective_permission_mode();
                Some(self.mode)
            }
            1 => {
                self.settings.read_only = !self.settings.read_only;
                self.mode = self.settings.effective_permission_mode();
                Some(self.mode)
            }
            2 => {
                self.settings.show_tool_activity = !self.settings.show_tool_activity;
                None
            }
            _ => None,
        }
    }

    /// Cycle safe permission modes on `Shift+Tab`: Ask -> Trusted -> ReadOnly -> Ask.
    pub fn cycle_permission_mode(&mut self) -> PermissionMode {
        let next = match self.mode {
            PermissionMode::Ask => PermissionMode::Trusted,
            PermissionMode::Trusted => PermissionMode::ReadOnly,
            PermissionMode::ReadOnly | PermissionMode::FullAccess => PermissionMode::Ask,
        };
        self.mode = next;
        next
    }

    pub fn approval_move_up(&mut self) {
        self.approval_selected = if self.approval_selected == 0 {
            2
        } else {
            self.approval_selected - 1
        };
    }

    pub fn approval_move_down(&mut self) {
        self.approval_selected = (self.approval_selected + 1) % 3;
    }

    pub fn approval_selected_decision(&self) -> ApprovalDecision {
        match self.approval_selected {
            0 => ApprovalDecision::Approve,
            1 => ApprovalDecision::ApproveAlways,
            _ => ApprovalDecision::Deny,
        }
    }

    /// Compute a concise unified diff preview for a pending `write_file` or `edit_file` approval popup.
    pub fn approval_preview_diff(&self) -> Option<String> {
        let pending = self.pending_approval.as_ref()?;
        let tool = pending.request.tool_name.as_str();
        if tool != "write_file" && tool != "edit_file" {
            return None;
        }
        let raw_args = self
            .tools
            .iter()
            .rev()
            .find(|t| t.call_id == pending.request.call_id)
            .and_then(|t| t.raw_args.as_deref())?;
        let val = serde_json::from_str::<serde_json::Value>(raw_args).ok()?;
        let rel_path = val.get("path").and_then(|v| v.as_str())?;

        let full_path = self.cwd.join(rel_path);
        let old_content = fs::read_to_string(&full_path).unwrap_or_default();
        let new_content = if tool == "write_file" {
            val.get("content").and_then(|v| v.as_str())?.to_string()
        } else {
            let old_str = val.get("old_string").and_then(|v| v.as_str())?;
            let new_str = val.get("new_string").and_then(|v| v.as_str())?;
            let replace_all = val
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if replace_all {
                old_content.replace(old_str, new_str)
            } else {
                old_content.replacen(old_str, new_str, 1)
            }
        };

        let diff = TextDiff::from_lines(old_content.as_str(), new_content.as_str());
        let mut out = String::new();
        let mut shown = 0usize;
        for change in diff.iter_all_changes() {
            let prefix = match change.tag() {
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
                similar::ChangeTag::Equal => continue,
            };
            if shown >= 12 {
                out.push_str("  … (more changes)\n");
                break;
            }
            let line = change.value().trim_end_matches(['\r', '\n']);
            out.push_str(&format!("{prefix}{line}\n"));
            shown += 1;
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Update the active project folder and refresh `@file` completion candidates.
    pub fn set_cwd(&mut self, new_cwd: PathBuf) {
        let canonical = fs::canonicalize(&new_cwd).unwrap_or(new_cwd);
        self.workspace_files = collect_workspace_files(&canonical, 2000);
        self.mention_suggestions.clear();
        self.cwd = canonical;
    }

    /// Friendly path with `~` shorthand for the user's home folder.
    pub fn friendly_cwd(&self) -> String {
        format_friendly_path(&self.cwd)
    }

    /// Short folder name for the compact top header (e.g. `my-app` or `~`).
    pub fn short_cwd(&self) -> String {
        let friendly = self.friendly_cwd();
        if friendly == "~" {
            return "~".to_string();
        }
        self.cwd
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or(friendly)
    }

    /// Remember display names from the model catalog so the UI can show them instead of IDs.
    pub fn apply_model_catalog(&mut self, models: &[ModelInfo]) {
        self.model_names = models
            .iter()
            .map(|m| (m.id.clone(), m.name().to_string()))
            .collect();
    }

    /// User-facing name for `id` (falls back to the raw ID).
    pub fn display_name_for<'a>(&'a self, id: &'a str) -> &'a str {
        self.model_names.get(id).map(String::as_str).unwrap_or(id)
    }

    /// User-facing name of the currently selected model.
    pub fn model_display(&self) -> &str {
        self.display_name_for(&self.model)
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

    /// Recompute live `/` slash command suggestions from the current input.
    pub fn refresh_slash_suggestions(&mut self) {
        self.slash_suggestions = filter_slash_suggestions(self.input.text());
        if self.slash_suggestions.is_empty() || self.slash_selected >= self.slash_suggestions.len()
        {
            self.slash_selected = 0;
        }
    }

    pub fn slash_move_up(&mut self) {
        if self.slash_suggestions.is_empty() {
            return;
        }
        if self.slash_selected == 0 {
            self.slash_selected = self.slash_suggestions.len() - 1;
        } else {
            self.slash_selected -= 1;
        }
    }

    pub fn slash_move_down(&mut self) {
        if self.slash_suggestions.is_empty() {
            return;
        }
        self.slash_selected = (self.slash_selected + 1) % self.slash_suggestions.len();
    }

    /// Replace the input box with the highlighted slash command on `Tab`.
    pub fn apply_selected_slash_completion(&mut self) -> bool {
        let Some(chosen) = self.slash_suggestions.get(self.slash_selected).copied() else {
            return false;
        };
        let len = self.input.text().len();
        self.input.replace_range(0, len, chosen.command);
        self.refresh_slash_suggestions();
        true
    }

    /// Consume and return the highlighted slash command when pressing `Enter` in the palette.
    pub fn take_selected_slash_command(&mut self) -> Option<String> {
        let chosen = self.slash_suggestions.get(self.slash_selected).copied()?;
        self.slash_suggestions.clear();
        self.slash_selected = 0;
        let _ = self.input.take_submission();
        Some(chosen.command.to_string())
    }

    pub fn scroll_up(&mut self, lines: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lines);
    }

    /// Human-readable description of what Openmi is currently doing, when busy.
    pub fn active_activity_label(&self) -> Option<String> {
        if !self.is_busy() {
            return None;
        }
        if let Some(item) = self.tools.iter().rev().find(|t| {
            matches!(
                t.status,
                ToolStatus::WaitingApproval | ToolStatus::Running | ToolStatus::Requested
            )
        }) {
            return Some(match item.status {
                ToolStatus::WaitingApproval => "Waiting for your permission...".to_string(),
                ToolStatus::Running | ToolStatus::Requested => {
                    friendly_tool_progress(&item.tool_name)
                }
                _ => "Thinking...".to_string(),
            });
        }
        Some("Thinking...".to_string())
    }

    pub fn apply_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::ControlStateChanged { state } => {
                self.control_state = state;
                if matches!(state, ControlState::Idle) {
                    self.armed_quit = false;
                    self.status_hint.clear();
                }
            }
            AgentEvent::StepStarted { .. } => {
                self.status_hint = "Thinking...".to_string();
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
                let target_hint = extract_tool_target(&tool_name, &arguments_json);
                self.tools.push(ToolTimelineItem {
                    call_id,
                    tool_name,
                    status: ToolStatus::Requested,
                    summary: arguments_json.clone(),
                    target_hint,
                    raw_args: Some(arguments_json),
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
                let target_hint = self
                    .tools
                    .iter_mut()
                    .rev()
                    .find(|t| t.call_id == call_id)
                    .map(|t| {
                        t.raw_args = None;
                        t.target_hint.clone()
                    })
                    .unwrap_or_default();
                let inline_line = format_inline_tool_summary(&tool_name, &target_hint, ok);
                self.update_or_push_tool(call_id, tool_name, status, first_line);
                if self.settings.show_tool_activity {
                    self.transcript.push(TranscriptEntry {
                        kind: if ok {
                            TranscriptKind::ToolOk
                        } else {
                            TranscriptKind::ToolErr
                        },
                        text: inline_line,
                    });
                    self.scroll_from_bottom = 0;
                }
            }
            AgentEvent::TurnCompleted { .. } => {
                self.status_hint.clear();
            }
            AgentEvent::Error { message, .. } => {
                self.push_error(message);
                self.status_hint.clear();
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
                target_hint: String::new(),
                raw_args: None,
            });
        }
    }

    /// Resolve a pending approval popup with `decision`.
    pub fn resolve_approval(&mut self, decision: ApprovalDecision) -> bool {
        if let Some(pending) = self.pending_approval.take() {
            self.approval_selected = 0;
            self.status_hint = match decision {
                ApprovalDecision::Approve => "Allowed".to_string(),
                ApprovalDecision::ApproveAlways => "Allowed for this session".to_string(),
                ApprovalDecision::Deny => "Cancelled".to_string(),
            };
            let _ = pending.responder.send(decision);
            return true;
        }
        false
    }

    /// Handle Esc or Ctrl-C: closes modals, cancels active turn, or quits on 2nd press.
    /// Returns `true` if the active turn should be cancelled now.
    pub fn handle_interrupt(&mut self) -> bool {
        if self.settings_modal.is_some() {
            self.settings_modal = None;
            return false;
        }

        if self.resume_picker.is_some() {
            self.resume_picker = None;
            return false;
        }

        if self.folder_picker.is_some() {
            self.folder_picker = None;
            return false;
        }

        if self.model_selector.is_some() {
            self.model_selector = None;
            return false;
        }

        if self.full_access_modal.is_some() {
            self.full_access_modal = None;
            self.push_system("Cancelled Full Access mode change.");
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
            self.status_hint = "Stopping... press Esc again to quit".to_string();
            return true;
        }

        if self.armed_quit {
            self.should_quit = true;
        } else {
            self.armed_quit = true;
            self.status_hint = "Press Esc again to quit".to_string();
        }
        false
    }
}

/// Format a path with `~` replacing the user's home directory so it's short and easy to read.
pub fn format_friendly_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home_path = PathBuf::from(home);
        if path == home_path {
            return "~".to_string();
        }
        if let Ok(rel) = path.strip_prefix(&home_path) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

/// Resolve a user-typed folder path (supports `~`, `~/sub`, relative, or absolute paths).
pub fn resolve_folder_input(raw: &str, base: &Path) -> PathBuf {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home_path = PathBuf::from(home);
        if trimmed == "~" {
            return home_path;
        }
        if let Some(rest) = trimmed
            .strip_prefix("~/")
            .or_else(|| trimmed.strip_prefix("~\\"))
        {
            return home_path.join(rest);
        }
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    }
}

/// Convert internal tool names into friendly progress phrases for terminal beginners.
pub fn friendly_tool_progress(tool_name: &str) -> String {
    match tool_name {
        "read_file" => "Reading a file...".to_string(),
        "list_dir" => "Looking at your files...".to_string(),
        "grep" => "Searching code...".to_string(),
        "glob" => "Finding files...".to_string(),
        "edit_file" | "write_file" => "Editing a file...".to_string(),
        "run_command" => "Running a command...".to_string(),
        other => format!("Using {other}..."),
    }
}

/// Convert internal tool names into friendly action titles for approval popups.
pub fn friendly_tool_title(tool_name: &str) -> String {
    match tool_name {
        "read_file" => "Read a file".to_string(),
        "list_dir" => "View a folder".to_string(),
        "grep" => "Search code".to_string(),
        "glob" => "Find files".to_string(),
        "edit_file" => "Edit a file".to_string(),
        "write_file" => "Write a file".to_string(),
        "run_command" => "Run a command".to_string(),
        other => format!("Use `{other}`"),
    }
}

/// Extract a concise target (file path, pattern, or command) from raw tool JSON arguments.
pub fn extract_tool_target(tool_name: &str, arguments_json: &str) -> String {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(arguments_json) else {
        return String::new();
    };
    let raw = match tool_name {
        "read_file" | "edit_file" | "write_file" | "list_dir" => val
            .get("path")
            .or_else(|| val.get("file_path"))
            .and_then(|v| v.as_str()),
        "grep" | "glob" => val.get("pattern").and_then(|v| v.as_str()),
        "run_command" => val
            .get("command")
            .or_else(|| val.get("cmd"))
            .and_then(|v| v.as_str()),
        _ => None,
    };
    let Some(s) = raw else {
        return String::new();
    };
    let single_line = s.lines().next().unwrap_or("").trim();
    let truncated: String = single_line.chars().take(54).collect();
    if single_line.chars().count() > 54 {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Format a clean, Claude Code-style 1-line inline summary for a completed tool call.
pub fn format_inline_tool_summary(tool_name: &str, target: &str, ok: bool) -> String {
    let verb = match tool_name {
        "read_file" => "Read",
        "list_dir" => "Listed",
        "grep" => "Searched",
        "glob" => "Found",
        "edit_file" | "write_file" => "Edited",
        "run_command" => "Ran",
        other => other,
    };
    let base = if target.is_empty() {
        match tool_name {
            "read_file" => "Read file".to_string(),
            "list_dir" => "Listed folder".to_string(),
            "grep" => "Searched workspace".to_string(),
            "glob" => "Found files".to_string(),
            "edit_file" | "write_file" => "Edited file".to_string(),
            "run_command" => "Ran command".to_string(),
            other => format!("Used {other}"),
        }
    } else {
        format!("{verb} {target}")
    };
    if ok {
        base
    } else {
        format!("{base} (failed)")
    }
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
        app.apply_agent_event(AgentEvent::ToolCallRequested {
            call_id: "c1".into(),
            tool_name: "read_file".into(),
            arguments_json: r#"{"path":"src/main.rs"}"#.into(),
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

        assert_eq!(app.transcript[0].text, "Hello world");
        assert_eq!(app.transcript[1].kind, TranscriptKind::ToolOk);
        assert_eq!(app.transcript[1].text, "Read src/main.rs");
        assert_eq!(app.tools.len(), 1);
        assert_eq!(app.tools[0].status, ToolStatus::Succeeded);
    }

    #[test]
    fn slash_palette_filters_and_completes() {
        let mut app = TuiApp::new(
            "openmi-default".into(),
            PermissionMode::Ask,
            PathBuf::from("."),
        );
        for ch in "/fo".chars() {
            app.input.insert_char(ch);
        }
        app.refresh_slash_suggestions();
        assert_eq!(app.slash_suggestions.len(), 1);
        assert_eq!(app.slash_suggestions[0].command, "/folder");
        assert_eq!(
            app.take_selected_slash_command(),
            Some("/folder".to_string())
        );
        assert!(app.slash_suggestions.is_empty());
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

    #[test]
    fn folder_picker_navigates_and_filters_hidden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("my-project");
        fs::create_dir(&sub).unwrap();
        fs::create_dir(tmp.path().join(".hidden")).unwrap();
        fs::create_dir(tmp.path().join("node_modules")).unwrap();

        let mut picker = FolderPickerState::new(tmp.path().to_path_buf());
        let child_names: Vec<&str> = picker
            .entries
            .iter()
            .filter_map(|e| match e {
                FolderEntry::Child { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(child_names, vec!["my-project"]);

        // Select "my-project" and enter it.
        let child_idx = picker
            .entries
            .iter()
            .position(|e| matches!(e, FolderEntry::Child { name, .. } if name == "my-project"))
            .unwrap();
        picker.selected = child_idx;
        assert!(picker.enter().is_none());
        assert_eq!(picker.current_dir, fs::canonicalize(&sub).unwrap());

        // Entry 0 is UseCurrent -> Enter confirms the folder.
        assert_eq!(picker.selected, 0);
        assert_eq!(picker.enter(), Some(fs::canonicalize(&sub).unwrap()));
    }

    #[test]
    fn cycle_mode_and_approval_diff_preview() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("hello.txt"), "old line\n").unwrap();

        let mut app = TuiApp::new(
            "openmi-default".into(),
            PermissionMode::Ask,
            tmp.path().to_path_buf(),
        );
        assert_eq!(app.cycle_permission_mode(), PermissionMode::Trusted);
        assert_eq!(app.cycle_permission_mode(), PermissionMode::ReadOnly);
        assert_eq!(app.cycle_permission_mode(), PermissionMode::Ask);

        app.apply_agent_event(AgentEvent::ToolCallRequested {
            call_id: "w1".into(),
            tool_name: "write_file".into(),
            arguments_json: r#"{"path":"hello.txt","content":"new line\n"}"#.into(),
        });
        let (tx, _rx) = tokio::sync::oneshot::channel();
        app.pending_approval = Some(PendingApproval {
            request: openmi_core::ApprovalRequest {
                call_id: "w1".into(),
                tool_name: "write_file".into(),
                summary: "write_file hello.txt".into(),
            },
            responder: tx,
        });
        let diff = app.approval_preview_diff().unwrap();
        assert!(diff.contains("-old line"));
        assert!(diff.contains("+new line"));
    }
}
