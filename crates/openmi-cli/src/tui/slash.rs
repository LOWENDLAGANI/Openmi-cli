use std::path::Path;

use openmi_checkpoints::CheckpointStore;
use openmi_core::PermissionMode;

use crate::tui::diff::render_latest_checkpoint_diff;

/// Confirmation phrase ported from `FullAccessModal` wording.
pub const FULL_ACCESS_CONFIRM_PHRASE: &str = "I UNDERSTAND FULL ACCESS";

/// Warning copy shown inside the FullAccess confirmation modal.
pub const FULL_ACCESS_MODAL_WARNING: &str = concat!(
    "FullAccess mode allows the agent to run commands and modify files ",
    "without asking for confirmation. Only enable this in trusted repositories.\n\n",
    "Type exactly `I UNDERSTAND FULL ACCESS` and press Enter to confirm, or Esc to cancel."
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Model(Option<String>),
    Mode(Option<String>),
    Undo(Option<String>),
    Diff,
    Clear,
    Credits,
    Mcp(Option<String>),
    Help,
    Quit,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashOutcome {
    Message(String),
    Diff(String),
    Error(String),
    SetModel(String),
    SetMode(PermissionMode),
    RequireFullAccessConfirm,
    ClearHistory,
    RefreshCredits,
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
        "/model" => SlashCommand::Model(arg),
        "/mode" => SlashCommand::Mode(arg),
        "/undo" => SlashCommand::Undo(arg),
        "/diff" => SlashCommand::Diff,
        "/clear" => SlashCommand::Clear,
        "/credits" => SlashCommand::Credits,
        "/mcp" => SlashCommand::Mcp(arg),
        "/help" => SlashCommand::Help,
        "/quit" | "/exit" => SlashCommand::Quit,
        other => SlashCommand::Unknown(other.to_string()),
    };
    Some(parsed)
}

/// Evaluate a parsed slash command against current model, permission mode, checkpoints, and cwd.
pub fn execute_slash(
    cmd: SlashCommand,
    current_model: &str,
    current_mode: PermissionMode,
    checkpoints: &CheckpointStore,
    cwd: &Path,
) -> SlashOutcome {
    match cmd {
        SlashCommand::Model(None) => {
            SlashOutcome::Message(format!("Current model: {current_model}"))
        }
        SlashCommand::Model(Some(model)) => SlashOutcome::SetModel(model),
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
        SlashCommand::Help => SlashOutcome::Message(help_text().to_string()),
        SlashCommand::Quit => SlashOutcome::Quit,
        SlashCommand::Unknown(name) => {
            SlashOutcome::Error(format!("Unknown slash command `{name}`. Try `/help`."))
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

pub fn help_text() -> &'static str {
    concat!(
        "Available slash commands:\n",
        "  /model [id]         Show or switch active model\n",
        "  /mode [mode]        Show or switch mode (read-only|ask|trusted|full-access)\n",
        "  /undo [file|run]    Restore a file or checkpoint run to exact pre-write bytes\n",
        "  /diff               Show colored unified diff for the latest run\n",
        "  /clear              Clear conversation history and transcript\n",
        "  /credits            Show current Openmi credit balance\n",
        "  /mcp [status|restart [server]]  Inspect or restart stdio MCP servers\n",
        "  /help               Show this help text\n",
        "  /quit               Exit Openmi TUI\n",
        "\nTips: Type @path + Tab for fuzzy file completion; Shift/Alt+Enter for newline."
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

        // /model
        assert_eq!(
            execute_slash(
                parse_slash("/model").unwrap(),
                "openmi-default",
                PermissionMode::Ask,
                &cp,
                tmp.path()
            ),
            SlashOutcome::Message("Current model: openmi-default".into())
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
            SlashOutcome::Message(m) => assert!(m.contains("/undo")),
            other => panic!("expected Help message, got {other:?}"),
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
