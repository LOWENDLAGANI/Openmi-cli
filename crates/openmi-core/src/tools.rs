use std::fs;
use std::io::Read;
use std::path::Path;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use openmi_platform::job_object::{configure_process_group, kill_process_tree};
use openmi_platform::{atomic_write, PathGuard};

use crate::permissions::PermissionMode;
use crate::session::{BeforeWriteHook, CoreError};

/// Risk classification for built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    ReadOnly,
    Mutating,
}

pub fn classify_tool_risk(tool_name: &str) -> ToolRisk {
    match tool_name {
        "read_file" | "list_dir" => ToolRisk::ReadOnly,
        _ => ToolRisk::Mutating,
    }
}

/// Whether a tool requires interactive/explicit approval in the current permission mode.
pub fn needs_approval(tool_name: &str, mode: PermissionMode) -> Result<bool, String> {
    let risk = classify_tool_risk(tool_name);
    match (risk, mode) {
        (ToolRisk::ReadOnly, _) => Ok(false),
        (ToolRisk::Mutating, PermissionMode::ReadOnly) => Err(format!(
            "tool `{tool_name}` is blocked in ReadOnly permission mode"
        )),
        (ToolRisk::Mutating, PermissionMode::Ask) => Ok(true),
        (ToolRisk::Mutating, PermissionMode::Trusted | PermissionMode::FullAccess) => Ok(false),
    }
}

/// Execute a built-in workspace tool strictly honoring `PathGuard`, `before_write_hook`,
/// and `CancellationToken` (killing child process trees on cancel).
pub async fn execute_tool(
    tool_name: &str,
    arguments_json: &str,
    path_guard: &PathGuard,
    before_write_hook: Option<&BeforeWriteHook>,
    cancel: &CancellationToken,
) -> Result<String, CoreError> {
    let args: serde_json::Value = serde_json::from_str(arguments_json)
        .map_err(|e| CoreError::Tool(format!("invalid arguments JSON: {e}")))?;

    match tool_name {
        "read_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| CoreError::Tool("read_file requires `path`".to_string()))?;
            let mut file = path_guard.open_existing_read(path)?;
            let mut content = String::new();
            file.read_to_string(&mut content)
                .map_err(|e| CoreError::Tool(format!("read_file failed: {e}")))?;
            Ok(content)
        }
        "list_dir" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let resolved = path_guard.resolve_existing(path)?;
            let mut entries = Vec::new();
            for entry in fs::read_dir(&resolved)
                .map_err(|e| CoreError::Tool(format!("list_dir failed: {e}")))?
            {
                let entry = entry.map_err(|e| CoreError::Tool(e.to_string()))?;
                entries.push(entry.file_name().to_string_lossy().into_owned());
            }
            entries.sort();
            Ok(entries.join("\n"))
        }
        "write_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| CoreError::Tool("write_file requires `path`".to_string()))?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| CoreError::Tool("write_file requires `content`".to_string()))?;

            let resolved = path_guard.resolve_for_write(path)?;
            if let Some(hook) = before_write_hook {
                hook(&resolved).map_err(CoreError::BeforeWriteHook)?;
            }
            atomic_write(&resolved, content.as_bytes())
                .map_err(|e| CoreError::Tool(format!("write_file failed: {e}")))?;
            Ok(format!(
                "wrote {} bytes to {}",
                content.len(),
                resolved.display()
            ))
        }
        "run_command" => {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| CoreError::Tool("run_command requires `command`".to_string()))?;
            run_cancellable_shell(command, path_guard.root(), cancel).await
        }
        other => Err(CoreError::Tool(format!("unknown tool `{other}`"))),
    }
}

async fn run_cancellable_shell(
    command: &str,
    cwd: &Path,
    cancel: &CancellationToken,
) -> Result<String, CoreError> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };

    cmd.current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_process_group(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Tool(format!("failed to spawn command: {e}")))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = kill_process_tree(&mut child).await;
            return Err(CoreError::Cancelled);
        }
        res = child.wait() => {
            res.map_err(|e| CoreError::Tool(format!("command wait failed: {e}")))?
        }
    };

    let mut out = Vec::new();
    let mut err = Vec::new();
    if let Some(mut s) = stdout_pipe.take() {
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut out).await;
    }
    if let Some(mut s) = stderr_pipe.take() {
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut err).await;
    }

    let stdout_str = String::from_utf8_lossy(&out);
    let stderr_str = String::from_utf8_lossy(&err);
    let code = status.code().unwrap_or(-1);

    if stderr_str.is_empty() {
        Ok(format!("exit={code}\n{stdout_str}"))
    } else {
        Ok(format!("exit={code}\n{stdout_str}\nstderr:\n{stderr_str}"))
    }
}
