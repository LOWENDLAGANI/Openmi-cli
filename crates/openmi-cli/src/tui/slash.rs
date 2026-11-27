use std::path::Path;

use openmi_checkpoints::CheckpointStore;
use openmi_core::{PermissionMode, ReasoningEffort};

use crate::tui::diff::render_latest_checkpoint_diff;

/// Confirmation phrase ported from `FullAccessModal` wording.
pub const FULL_ACCESS_CONFIRM_PHRASE: &str = "I UNDERSTAND FULL ACCESS";

/// Warning copy shown inside the FullAccess confirmation modal.
pub const FULL_ACCESS_MODAL_WARNING: &str = concat!(
    "FullAccess mode allows the agent to run commands and modify files ",
    "without asking for confirmation. Only enable this in trusted repositories.\n\n",
    "Type exactly `I UNDERSTAND FULL ACCESS` and press Enter to confirm, or Esc to cancel."
);

/// One entry in the live `/` slash command autocomplete palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashSuggestion {
    pub command: String,
    pub description: String,
}

/// Ordered list of user-facing built-in slash commands shown when the user types `/`.
pub const SLASH_PALETTE: &[(&str, &str)] = &[
    ("/settings", "Configure auto-approve & preferences"),
    ("/folder", "Pick or change project folder"),
    ("/model", "Switch AI model"),
    ("/effort", "Set AI reasoning effort (off|low|medium|high)"),
    ("/resume", "Continue a previous chat"),
    ("/new", "Start a fresh chat"),
    ("/init", "Create a starter OPENMI.md guide"),
    (
        "/memory",
        "View or update persistent project memory (/memory [add <note>|clear])",
    ),
    ("/diff", "View recent file changes"),
    ("/undo", "Undo the last file edit"),
    ("/plan", "Toggle read-only planning mode (/plan on|off)"),
    ("/compact", "Summarize chat to save context & tokens"),
    ("/mode", "Change permission mode (or Shift+Tab)"),
    ("/credits", "Check remaining credits & daily token cap"),
    ("/upgrade", "Open the /pay page in your browser to upgrade"),
    ("/mcp", "Check MCP server status"),
    ("/help", "Show tips & commands"),
    ("/version", "Show Openmi version"),
    ("/quit", "Close Openmi"),
];

/// Return matching built-in slash commands when the input is an active single-token `/...` prefix.
#[allow(dead_code)]
pub fn filter_slash_suggestions(input: &str) -> Vec<SlashSuggestion> {
    filter_slash_suggestions_with_skills(input, &[])
}

/// Return matching built-in slash commands and custom skills when the input is an active `/...` prefix.
pub fn filter_slash_suggestions_with_skills(
    input: &str,
    skills: &[crate::skills::SkillDef],
) -> Vec<SlashSuggestion> {
    if !input.starts_with('/') || input.contains('\n') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let query = input.to_ascii_lowercase();
    let mut out: Vec<SlashSuggestion> = SLASH_PALETTE
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(&query))
        .map(|(cmd, desc)| SlashSuggestion {
            command: (*cmd).to_string(),
            description: (*desc).to_string(),
        })
        .collect();

    for skill in skills {
        let cmd = format!("/{}", skill.name);
        if cmd.starts_with(&query) && !out.iter().any(|s| s.command == cmd) {
            out.push(SlashSuggestion {
                command: cmd,
                description: format!("[skill] {}", skill.description),
            });
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Settings,
    Folder(Option<String>),
    Model(Option<String>),
    Effort(Option<String>),
    Resume,
    Init,
    Memory(Option<String>),
    Plan(Option<String>),
    Compact(Option<String>),
    Mode(Option<String>),
    Undo(Option<String>),
    Diff,
    Clear,
    Credits,
    Upgrade,
    Mcp(Option<String>),
    Help(Option<String>),
    Version,
    Quit,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashOutcome {
    Message(String),
    Diff(String),
    Error(String),
    OpenSettings,
    OpenFolderPicker,
    SetFolder(String),
    OpenModelSelector,
    SetModel(String),
    OpenEffortSelector,
    SetEffort(ReasoningEffort),
    OpenResumePicker,
    InitProjectGuide,
    TogglePlan(Option<bool>),
    Compact(Option<String>),
    SetMode(PermissionMode),
    RequireFullAccessConfirm,
    ClearHistory,
    RefreshCredits,
    OpenUpgrade,
    McpStatus,
    McpRestart(Option<String>),
    Quit,
}

/// Parse an input line starting with `/` into a `SlashCommand`.
pub fn parse_slash(input: &str) -> Option<SlashCommand> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let cmd = parts.next()?.to_ascii_lowercase();
    let arg = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let parsed = match cmd.as_str() {
        "/settings" | "/setting" => SlashCommand::Settings,
        "/folder" | "/project" | "/cd" => SlashCommand::Folder(arg),
        "/model" => SlashCommand::Model(arg),
        "/effort" => SlashCommand::Effort(arg),
        "/resume" => SlashCommand::Resume,
        "/init" => SlashCommand::Init,
        "/memory" => SlashCommand::Memory(arg),
        "/plan" => SlashCommand::Plan(arg),
        "/compact" => SlashCommand::Compact(arg),
        "/mode" => SlashCommand::Mode(arg),
        "/undo" => SlashCommand::Undo(arg),
        "/diff" => SlashCommand::Diff,
        "/clear" | "/new" => SlashCommand::Clear,
        "/credits" => SlashCommand::Credits,
        "/upgrade" | "/pay" => SlashCommand::Upgrade,
        "/mcp" => SlashCommand::Mcp(arg),
        "/help" => SlashCommand::Help(arg),
        "/version" => SlashCommand::Version,
        "/quit" | "/exit" => SlashCommand::Quit,
        other => SlashCommand::Unknown(other.to_string()),
    };
    Some(parsed)
}

/// Evaluate a parsed slash command against current model, permission mode, checkpoints, and cwd.
pub fn execute_slash(
    cmd: SlashCommand,
    _current_model: &str,
    current_mode: PermissionMode,
    checkpoints: &CheckpointStore,
    cwd: &Path,
) -> SlashOutcome {
    match cmd {
        SlashCommand::Settings => SlashOutcome::OpenSettings,
        SlashCommand::Folder(None) => SlashOutcome::OpenFolderPicker,
        SlashCommand::Folder(Some(path)) => SlashOutcome::SetFolder(path),
        SlashCommand::Model(None) => SlashOutcome::OpenModelSelector,
        SlashCommand::Model(Some(model)) => SlashOutcome::SetModel(model),
        SlashCommand::Effort(None) => SlashOutcome::OpenEffortSelector,
        SlashCommand::Effort(Some(raw)) => match ReasoningEffort::parse(&raw) {
            Some(effort) => SlashOutcome::SetEffort(effort),
            None => SlashOutcome::Error(format!(
                "Unknown effort level `{raw}`. Use: off, low, medium, or high."
            )),
        },
        SlashCommand::Resume => SlashOutcome::OpenResumePicker,
        SlashCommand::Init => SlashOutcome::InitProjectGuide,
        SlashCommand::Memory(arg) => execute_memory(arg.as_deref(), cwd),
        SlashCommand::Plan(arg) => match arg.as_deref().map(str::to_ascii_lowercase).as_deref() {
            None | Some("toggle") => SlashOutcome::TogglePlan(None),
            Some("on") | Some("enter") | Some("start") => SlashOutcome::TogglePlan(Some(true)),
            Some("off") | Some("exit") | Some("stop") => SlashOutcome::TogglePlan(Some(false)),
            Some(other) => SlashOutcome::Error(format!(
                "Unknown `/plan` argument `{other}`. Use `/plan`, `/plan on`, or `/plan off`."
            )),
        },
        SlashCommand::Compact(focus) => SlashOutcome::Compact(focus),
        SlashCommand::Mode(None) => {
            SlashOutcome::Message(format!("Current permission mode: {current_mode:?}"))
        }
        SlashCommand::Mode(Some(raw)) => match raw.parse::<PermissionMode>() {
            Ok(PermissionMode::FullAccess) if current_mode != PermissionMode::FullAccess => {
                SlashOutcome::RequireFullAccessConfirm
            }
            Ok(mode) => SlashOutcome::SetMode(mode),
            Err(err) => SlashOutcome::Error(err),
        },
        SlashCommand::Undo(target) => execute_undo(target.as_deref(), checkpoints, cwd),
        SlashCommand::Diff => SlashOutcome::Diff(render_latest_checkpoint_diff(checkpoints, cwd)),
        SlashCommand::Clear => SlashOutcome::ClearHistory,
        SlashCommand::Credits => SlashOutcome::RefreshCredits,
        SlashCommand::Upgrade => SlashOutcome::OpenUpgrade,
        SlashCommand::Mcp(arg) => match arg.as_deref() {
            None | Some("status") => SlashOutcome::McpStatus,
            Some("restart") => SlashOutcome::McpRestart(None),
            Some(rest) if rest.starts_with("restart ") => {
                let server = rest.trim_start_matches("restart ").trim();
                if server.is_empty() {
                    SlashOutcome::McpRestart(None)
                } else {
                    SlashOutcome::McpRestart(Some(server.to_string()))
                }
            }
            Some(other) => SlashOutcome::Error(format!(
                "Unknown `/mcp` subcommand `{other}`. Use `/mcp status` or `/mcp restart [server]`."
            )),
        },
        SlashCommand::Help(topic) => match topic.as_deref() {
            None => SlashOutcome::Message(help_text().to_string()),
            Some(t) => match help_topic_text(t) {
                Some(msg) => SlashOutcome::Message(msg.to_string()),
                None => SlashOutcome::Error(format!(
                    "Unknown help topic `{t}`. Try `/help` or `/help steps|commands|modes|keys|mcp|cli`."
                )),
            },
        },
        SlashCommand::Version => SlashOutcome::Message(version_text()),
        SlashCommand::Quit => SlashOutcome::Quit,
        SlashCommand::Unknown(name) => {
            SlashOutcome::Error(format!("Unknown slash command `{name}`. Try `/help`."))
        }
    }
}

fn execute_memory(arg: Option<&str>, cwd: &Path) -> SlashOutcome {
    let Ok(paths) = openmi_platform::AppPaths::resolve() else {
        return SlashOutcome::Error("Could not resolve Openmi config directory.".to_string());
    };
    let mem_path = paths.project_memory_file(cwd);

    match arg.map(str::trim) {
        None | Some("") | Some("view") => match std::fs::read_to_string(&mem_path) {
            Ok(content) if !content.trim().is_empty() => SlashOutcome::Message(format!(
                "Project memory ({}):\n\n{}",
                mem_path.display(),
                content.trim()
            )),
            _ => SlashOutcome::Message(format!(
                "No project memory saved yet ({}). Use `/memory add <note>` to save a note.",
                mem_path.display()
            )),
        },
        Some("clear") => {
            if mem_path.exists() {
                if let Err(e) = std::fs::remove_file(&mem_path) {
                    return SlashOutcome::Error(format!("Failed to clear memory: {e}"));
                }
            }
            SlashOutcome::Message("Cleared project memory.".to_string())
        }
        Some(rest) => {
            let note = rest.strip_prefix("add ").map(str::trim).unwrap_or(rest);
            if note.is_empty() {
                return SlashOutcome::Error(
                    "Usage: `/memory` | `/memory add <note>` | `/memory clear`".to_string(),
                );
            }
            let existing = std::fs::read_to_string(&mem_path).unwrap_or_default();
            let updated = if existing.trim().is_empty() {
                format!("- {note}\n")
            } else {
                format!("{}\n- {note}\n", existing.trim_end())
            };
            if let Some(parent) = mem_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return SlashOutcome::Error(format!("Failed to create memory dir: {e}"));
                }
            }
            if let Err(e) = openmi_platform::atomic_write(&mem_path, updated.as_bytes()) {
                return SlashOutcome::Error(format!("Failed to save memory: {e}"));
            }
            SlashOutcome::Message(format!(
                "Saved note to project memory ({}).",
                mem_path.display()
            ))
        }
    }
}

fn execute_undo(target: Option<&str>, checkpoints: &CheckpointStore, cwd: &Path) -> SlashOutcome {
    match target {
        None => match checkpoints.undo_run(None) {
            Ok(n) => SlashOutcome::Message(format!("Undid latest run ({n} file(s) restored).")),
            Err(err) => SlashOutcome::Error(err.to_string()),
        },
        Some(arg) => {
            // Check if arg matches a checkpoint run ID first.
            if checkpoints.checkpoints().iter().any(|cp| cp.id == arg) {
                return match checkpoints.undo_run(Some(arg)) {
                    Ok(n) => {
                        SlashOutcome::Message(format!("Undid run `{arg}` ({n} file(s) restored)."))
                    }
                    Err(err) => SlashOutcome::Error(err.to_string()),
                };
            }

            // Otherwise treat as a file path (relative to cwd or absolute).
            let candidate = Path::new(arg);
            let resolved = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                cwd.join(candidate)
            };
            let canonical_or_joined =
                std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());

            match checkpoints
                .undo_file(&canonical_or_joined)
                .or_else(|_| checkpoints.undo_file(&resolved))
            {
                Ok(()) => SlashOutcome::Message(format!("Restored `{arg}` from checkpoint.")),
                Err(err) => SlashOutcome::Error(err.to_string()),
            }
        }
    }
}

// Help topics are macros (not consts) so `help_text()` can `concat!` them without
// duplicating the copy — keeping the full guide and per-topic output in sync.
macro_rules! help_steps {
    () => {
        concat!(
            "=== STEP-BY-STEP TUTORIAL (HOW TO USE OPENMI) ===\n",
            "  Step 1. Sign in & check credits\n",
            "    - Run `openmi login` in your terminal if not signed in yet.\n",
            "    - Inside TUI, run `/credits` to see your plan, daily/wallet credits, and active pass.\n",
            "  Step 2. Pick a project folder and model\n",
            "    - Run `/folder` to browse to a project (or `/folder <path>`).\n",
            "    - Run `/model` to open the picker (Up/Down or j/k, Enter to select),\n",
            "      or `/model <id>` to switch directly.\n",
            "  Step 3. Choose a permission mode\n",
            "    - Default is `trusted` (auto-approve). Change it in `/settings`, with `/mode <mode>`,\n",
            "      or press Shift+Tab to cycle ask -> trusted -> read-only.\n",
            "  Step 4. Send a prompt (with @file mentions)\n",
            "    - Type your request and press Enter to send (Shift+Enter or Alt+Enter for newline).\n",
            "    - Type `@` + partial filename + Tab for .gitignore-aware fuzzy file completion.\n",
            "  Step 5. Review & approve tool calls\n",
            "    - In `ask` mode, `write_file`, `run_command` and MCP tools show a popup:\n",
            "      [y] allow once | [a] always this session | [n] cancel (or Up/Down + Enter).\n",
            "    - Tool activity is shown inline in the chat (toggle in `/settings`).\n",
            "  Step 6. Inspect diffs or undo changes\n",
            "    - Run `/diff` to view a colored unified diff of the latest run.\n",
            "    - Run `/undo`, `/undo <file>`, or `/undo <run_id>` to restore exact pre-write bytes.\n",
            "  Step 7. Customize & resume later\n",
            "    - Run `/init` to create a starter `OPENMI.md` (CLAUDE.md / AGENTS.md also work).\n",
            "    - Exit with `/quit` or Esc twice; continue with `/resume` or `openmi --resume`.\n"
        )
    };
}

macro_rules! help_commands {
    () => {
        concat!(
            "=== SLASH COMMANDS (INSIDE TUI) ===\n",
            "  /settings                       Toggle auto-approve, read-only, tool activity lines\n",
            "  /folder [path]                  Pick a project folder (aliases: /project, /cd)\n",
            "  /model [id]                     Open interactive model selector or switch directly\n",
            "  /effort [off|low|medium|high]   Set AI reasoning effort (default: off)\n",
            "  /resume                         Pick a previous chat to continue\n",
            "  /new (or /clear)                Start a fresh chat (clears history)\n",
            "  /init                           Create a starter OPENMI.md in the project folder\n",
            "  /mode [mode]                    Show or switch mode (read-only|ask|trusted|full-access)\n",
            "  /diff                           Show colored unified diff for the latest run\n",
            "  /undo [file|run]                Restore latest run, run ID, or file to pre-write bytes\n",
            "  /credits                        Refresh plan, daily + wallet credits, and pass status\n",
            "  /mcp [status|restart [server]]  Inspect or restart configured stdio MCP servers\n",
            "  /help [topic]                   Show full guide or topic (steps|commands|modes|keys|mcp|cli)\n",
            "  /version                        Show the running Openmi CLI version and platform\n",
            "  /quit (or /exit)                Exit Openmi TUI (session is auto-saved)\n"
        )
    };
}

macro_rules! help_modes {
    () => {
        concat!(
            "=== PERMISSION MODES & TOOLS ===\n",
            "  read-only    Allow `read_file` & `list_dir` only; block all writes and commands.\n",
            "  ask          Prompt [y/a/n] before writes/commands.\n",
            "  trusted      Auto-approve workspace tools within PathGuard boundaries (default)\n",
            "               (MCP tools still ask unless listed in the server's `allowlist`).\n",
            "  full-access  Unrestricted tool execution; requires typing `I UNDERSTAND FULL ACCESS`.\n",
            "  Built-in tools: `read_file`, `list_dir`, `write_file`, `run_command`.\n"
        )
    };
}

macro_rules! help_keys {
    () => {
        concat!(
            "=== KEYBOARD SHORTCUTS ===\n",
            "  Enter                    Send prompt or run /command\n",
            "  Shift+Enter / Alt+Enter  Insert newline in multiline prompt\n",
            "  Tab                      Accept / command or @path completion\n",
            "  Shift+Tab                Cycle mode: ask -> trusted -> read-only\n",
            "  Up / Down                Prompt history (or move in lists and pickers)\n",
            "  Ctrl/Alt+Up / Down       Scroll chat 3 lines\n",
            "  PageUp / PageDown        Scroll chat 5 lines\n",
            "  y / a / n                Approval popup: allow once / always / cancel\n",
            "  Esc / Ctrl-C             Close popup (denies a pending approval) or stop the\n",
            "                           running turn; press twice to quit\n"
        )
    };
}

macro_rules! help_mcp {
    () => {
        concat!(
            "=== WORKSPACE, SESSIONS & MCP ===\n",
            "  OPENMI.md       Project instructions loaded from the workspace root\n",
            "                  (falls back to CLAUDE.md, then AGENTS.md).\n",
            "  Sessions        Auto-saved to `~/.local/share/openmi/sessions/`; use `/resume`\n",
            "                  or `openmi --resume [ID]`.\n",
            "  MCP servers     Configure stdio servers in `~/.config/openmi/mcp.json` or `.openmi/mcp.json`.\n",
            "                  Tools register as `mcp__<server>__<tool>`. Use `/mcp status` or `/mcp restart`.\n"
        )
    };
}

macro_rules! help_cli {
    () => {
        concat!(
            "=== CLI COMMANDS (OUTSIDE TUI) ===\n",
            "  openmi login / whoami / logout       Authenticate or inspect current account\n",
            "  openmi [--resume [ID]]               Launch TUI (or resume latest/specific session)\n",
            "  openmi run \"<prompt>\" [--json] [-y]  Run a single headless prompt turn\n",
            "  openmi models [--json]               List available models\n",
            "  openmi credits [--json]              Check credit balance and active pass\n",
            "  openmi mcp list [--json]             List configured MCP servers and tools\n",
            "  openmi update [--check]              Check for or install CLI updates\n"
        )
    };
}

const HELP_STEPS: &str = help_steps!();
const HELP_COMMANDS: &str = help_commands!();
const HELP_MODES: &str = help_modes!();
const HELP_KEYS: &str = help_keys!();
const HELP_MCP: &str = help_mcp!();
const HELP_CLI: &str = help_cli!();

pub fn help_text() -> &'static str {
    concat!(
        "Openmi CLI Tutorial & Feature Guide\n",
        "===================================\n\n",
        help_steps!(),
        "\n",
        help_commands!(),
        "\n",
        help_modes!(),
        "\n",
        help_keys!(),
        "\n",
        help_mcp!(),
        "\n",
        help_cli!(),
        "\n",
        "Tip: Use PageUp/PageDown to scroll, or run `/help <topic>` (steps|commands|modes|keys|mcp|cli)."
    )
}

pub fn help_topic_text(topic: &str) -> Option<&'static str> {
    match topic.trim().to_ascii_lowercase().as_str() {
        "step" | "steps" | "tutorial" | "start" => Some(HELP_STEPS),
        "cmd" | "cmds" | "command" | "commands" | "slash" => Some(HELP_COMMANDS),
        "mode" | "modes" | "permission" | "permissions" | "tools" => Some(HELP_MODES),
        "key" | "keys" | "shortcut" | "shortcuts" => Some(HELP_KEYS),
        "mcp" | "workspace" | "session" | "sessions" => Some(HELP_MCP),
        "cli" => Some(HELP_CLI),
        _ => None,
    }
}

pub fn version_text() -> String {
    format!(
        "Openmi CLI v{} ({}-{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn tests_every_slash_command_and_full_access_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("notes.txt");
        let bom_crlf = b"\xEF\xBB\xBFalpha\r\nbeta\r\n";
        fs::write(&file, bom_crlf).unwrap();

        let mut cp = CheckpointStore::new();
        let run_id = cp.begin_run();
        let canon = fs::canonicalize(&file).unwrap();
        cp.snapshot_before_write(&canon).unwrap();
        fs::write(&canon, b"modified\n").unwrap();

        // /settings
        assert_eq!(
            execute_slash(
                parse_slash("/settings").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::OpenSettings
        );

        // /folder (and /project alias)
        assert_eq!(
            execute_slash(
                parse_slash("/folder").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::OpenFolderPicker
        );
        assert_eq!(
            execute_slash(
                parse_slash("/project ~/my-app").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::SetFolder("~/my-app".into())
        );

        assert_eq!(
            execute_slash(
                parse_slash("/resume").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::OpenResumePicker
        );
        assert_eq!(
            execute_slash(
                parse_slash("/init").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::InitProjectGuide
        );
        assert_eq!(
            execute_slash(
                parse_slash("/new").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::ClearHistory
        );

        // /model
        assert_eq!(
            execute_slash(
                parse_slash("/model").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::OpenModelSelector
        );
        assert_eq!(
            execute_slash(
                parse_slash("/model gpt-4.1").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::SetModel("gpt-4.1".into())
        );

        // /mode + FullAccess typed confirmation gate
        assert_eq!(
            execute_slash(
                parse_slash("/mode trusted").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::SetMode(PermissionMode::Trusted)
        );
        assert_eq!(
            execute_slash(
                parse_slash("/mode full-access").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::RequireFullAccessConfirm
        );

        // /diff
        match execute_slash(
            parse_slash("/diff").unwrap(),
            "openmi-default",
            PermissionMode::Ask,
            &cp,
            tmp.path(),
        ) {
            SlashOutcome::Diff(d) => assert!(d.contains("+modified")),
            other => panic!("expected Diff, got {other:?}"),
        }

        // /undo <file> restores exact BOM+CRLF bytes
        match execute_slash(
            parse_slash("/undo notes.txt").unwrap(),
            "openmi-default",
            PermissionMode::Ask,
            &cp,
            tmp.path(),
        ) {
            SlashOutcome::Message(m) => assert!(m.contains("Restored")),
            other => panic!("expected Message, got {other:?}"),
        }
        assert_eq!(fs::read(&canon).unwrap(), bom_crlf);

        // Edit again and test /undo <run_id>
        fs::write(&canon, b"edited-again").unwrap();
        match execute_slash(
            parse_slash(&format!("/undo {run_id}")).unwrap(),
            "openmi-default",
            PermissionMode::Ask,
            &cp,
            tmp.path(),
        ) {
            SlashOutcome::Message(m) => assert!(m.contains("Undid run")),
            other => panic!("expected Message, got {other:?}"),
        }
        assert_eq!(fs::read(&canon).unwrap(), bom_crlf);

        // /clear, /credits, /help, /quit
        assert_eq!(
            execute_slash(
                parse_slash("/clear").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::ClearHistory
        );
        assert_eq!(
            execute_slash(
                parse_slash("/credits").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::RefreshCredits
        );
        match execute_slash(
            parse_slash("/help").unwrap(),
            "openmi-default",
            PermissionMode::Ask,
            &cp,
            tmp.path(),
        ) {
            SlashOutcome::Message(m) => {
                assert!(m.contains("STEP-BY-STEP TUTORIAL"));
                assert!(m.contains("Step 1."));
                assert!(m.contains("Step 7."));
                assert!(m.contains("/undo"));
                assert!(m.contains("PERMISSION MODES"));
                assert!(m.contains("KEYBOARD SHORTCUTS"));
                assert!(m.contains("CLI COMMANDS"));
            }
            other => panic!("expected Help message, got {other:?}"),
        }
        match execute_slash(
            parse_slash("/help steps").unwrap(),
            "openmi-default",
            PermissionMode::Ask,
            &cp,
            tmp.path(),
        ) {
            SlashOutcome::Message(m) => assert!(m.contains("Step 1.")),
            other => panic!("expected Help steps message, got {other:?}"),
        }
        match execute_slash(
            parse_slash("/help nonexistent").unwrap(),
            "openmi-default",
            PermissionMode::Ask,
            &cp,
            tmp.path(),
        ) {
            SlashOutcome::Error(e) => assert!(e.contains("Unknown help topic")),
            other => panic!("expected Help error, got {other:?}"),
        }
        assert_eq!(
            execute_slash(
                parse_slash("/mcp").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::McpStatus
        );
        assert_eq!(
            execute_slash(
                parse_slash("/mcp restart").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::McpRestart(None)
        );
        assert_eq!(
            execute_slash(
                parse_slash("/mcp restart fs").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::McpRestart(Some("fs".into()))
        );
        match execute_slash(
            parse_slash("/version").unwrap(),
            "openmi-default",
            PermissionMode::Ask,
            &cp,
            tmp.path(),
        ) {
            SlashOutcome::Message(m) => {
                assert!(m.contains(env!("CARGO_PKG_VERSION")));
            }
            other => panic!("expected Version message, got {other:?}"),
        }
        assert_eq!(
            execute_slash(
                parse_slash("/quit").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::Quit
        );
    }
}
