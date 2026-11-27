use std::fs;
use std::io::Read;
use std::path::Path;
use ignore::WalkBuilder;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use openmi_platform::job_object::{configure_process_group, kill_process_tree};
use openmi_platform::{atomic_write, PathGuard};

use crate::permissions::PermissionMode;
use crate::session::{BeforeWriteHook, CoreError};

const MAX_READ_BYTES: usize = 200_000;
const DEFAULT_GREP_LIMIT: usize = 60;
const MAX_GREP_LIMIT: usize = 200;
const DEFAULT_GLOB_LIMIT: usize = 100;
const MAX_GLOB_LIMIT: usize = 300;
const MAX_GREP_FILE_BYTES: u64 = 1_048_576; // 1 MiB per file
pub const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;
pub const MAX_COMMAND_TIMEOUT_MS: u64 = 600_000;
pub const SHELL_OUTPUT_HEAD_BYTES: usize = 6 * 1024;
pub const SHELL_OUTPUT_TAIL_BYTES: usize = 10 * 1024;

/// Risk classification for built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    ReadOnly,
    Mutating,
}

pub fn classify_tool_risk(tool_name: &str) -> ToolRisk {
    match tool_name {
        "read_file" | "list_dir" | "grep" | "glob" | "todo_write" | "web_fetch" | "ask_user"
        | "read_task_output" => ToolRisk::ReadOnly,
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
        "read_file" => exec_read_file(&args, path_guard),
        "list_dir" => exec_list_dir(&args, path_guard),
        "grep" => exec_grep(&args, path_guard, cancel),
        "glob" => exec_glob(&args, path_guard, cancel),
        "web_fetch" => crate::web_fetch::exec_web_fetch(&args, cancel).await,
        "edit_file" => exec_edit_file(&args, path_guard, before_write_hook),
        "write_file" => exec_write_file(&args, path_guard, before_write_hook),
        "delete_file" => exec_delete_file(&args, path_guard, before_write_hook),
        "move_file" => exec_move_file(&args, path_guard, before_write_hook),
        "run_command" => {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| CoreError::Tool("run_command requires `command`".to_string()))?;
            let timeout_ms = args
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .map(|ms| ms.clamp(1, MAX_COMMAND_TIMEOUT_MS))
                .unwrap_or(DEFAULT_COMMAND_TIMEOUT_MS);
            run_cancellable_shell(command, path_guard.root(), timeout_ms, cancel).await
        }
        other => Err(CoreError::Tool(format!("unknown tool `{other}`"))),
    }
}

fn exec_read_file(args: &serde_json::Value, path_guard: &PathGuard) -> Result<String, CoreError> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("read_file requires `path`".to_string()))?;
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .map(|n| n.max(1) as usize);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n.max(1) as usize);

    let mut file = path_guard.open_existing_read(path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|e| CoreError::Tool(format!("read_file failed: {e}")))?;

    if offset.is_some() || limit.is_some() {
        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let start_1 = offset.unwrap_or(1);
        if start_1 > total {
            return Ok(format!(
                "(offset {start_1} is past end of file; total lines: {total})"
            ));
        }
        let start_idx = start_1 - 1;
        let max_lines = limit.unwrap_or(200);
        let end_idx = (start_idx + max_lines).min(total);
        let mut out = String::new();
        for (idx, line) in lines[start_idx..end_idx].iter().enumerate() {
            let line_no = start_idx + idx + 1;
            out.push_str(&format!("{line_no:>5} | {line}\n"));
        }
        if end_idx < total {
            out.push_str(&format!(
                "... (showing lines {start_1}-{end_idx} of {total}; pass offset={} to continue)",
                end_idx + 1
            ));
        }
        return Ok(out);
    }

    if content.len() > MAX_READ_BYTES {
        let mut cut = MAX_READ_BYTES;
        while cut > 0 && !content.is_char_boundary(cut) {
            cut -= 1;
        }
        let total_lines = content.lines().count();
        return Ok(format!(
            "{}\n... (truncated at {} bytes; total {} lines — use offset/limit to read specific ranges)",
            &content[..cut],
            cut,
            total_lines
        ));
    }

    Ok(content)
}

fn exec_list_dir(args: &serde_json::Value, path_guard: &PathGuard) -> Result<String, CoreError> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let resolved = path_guard.resolve_existing(path)?;
    let mut entries = Vec::new();
    for entry in
        fs::read_dir(&resolved).map_err(|e| CoreError::Tool(format!("list_dir failed: {e}")))?
    {
        let entry = entry.map_err(|e| CoreError::Tool(e.to_string()))?;
        let mut name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            name.push('/');
        }
        entries.push(name);
    }
    entries.sort();
    Ok(entries.join("\n"))
}

fn exec_edit_file(
    args: &serde_json::Value,
    path_guard: &PathGuard,
    before_write_hook: Option<&BeforeWriteHook>,
) -> Result<String, CoreError> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("edit_file requires `path`".to_string()))?;
    let old_string = args
        .get("old_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("edit_file requires `old_string`".to_string()))?;
    let new_string = args
        .get("new_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("edit_file requires `new_string`".to_string()))?;
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if old_string.is_empty() {
        return Err(CoreError::Tool(
            "edit_file `old_string` must not be empty".to_string(),
        ));
    }
    if old_string == new_string {
        return Err(CoreError::Tool(
            "edit_file `old_string` and `new_string` are identical".to_string(),
        ));
    }

    let mut file = path_guard.open_existing_read(path)?;
    let mut current = String::new();
    file.read_to_string(&mut current)
        .map_err(|e| CoreError::Tool(format!("edit_file read failed: {e}")))?;
    drop(file);

    let occurrences = current.matches(old_string).count();
    if occurrences == 0 {
        return Err(CoreError::Tool(format!(
            "old_string not found in `{path}`. Use read_file first to verify exact text and indentation."
        )));
    }
    if occurrences > 1 && !replace_all {
        return Err(CoreError::Tool(format!(
            "old_string matched {occurrences} times in `{path}`. Include more surrounding context to make it unique, or set replace_all=true."
        )));
    }

    let updated = if replace_all {
        current.replace(old_string, new_string)
    } else {
        current.replacen(old_string, new_string, 1)
    };
    let replaced_count = if replace_all { occurrences } else { 1 };

    let resolved = path_guard.resolve_for_write(path)?;
    if let Some(hook) = before_write_hook {
        hook(&resolved).map_err(CoreError::BeforeWriteHook)?;
    }
    atomic_write(&resolved, updated.as_bytes())
        .map_err(|e| CoreError::Tool(format!("edit_file write failed: {e}")))?;

    Ok(format!(
        "edited {} (replaced {} occurrence(s), {} -> {} bytes)",
        path,
        replaced_count,
        current.len(),
        updated.len()
    ))
}

fn exec_write_file(
    args: &serde_json::Value,
    path_guard: &PathGuard,
    before_write_hook: Option<&BeforeWriteHook>,
) -> Result<String, CoreError> {
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

fn exec_delete_file(
    args: &serde_json::Value,
    path_guard: &PathGuard,
    before_write_hook: Option<&BeforeWriteHook>,
) -> Result<String, CoreError> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("delete_file requires `path`".to_string()))?;

    let resolved = path_guard.resolve_existing(path)?;
    if resolved.is_dir() {
        return Err(CoreError::Tool(format!(
            "delete_file cannot delete directory `{path}`; only regular files are supported"
        )));
    }
    if let Some(hook) = before_write_hook {
        hook(&resolved).map_err(CoreError::BeforeWriteHook)?;
    }
    fs::remove_file(&resolved)
        .map_err(|e| CoreError::Tool(format!("delete_file failed: {e}")))?;
    Ok(format!("deleted {path}"))
}

fn exec_move_file(
    args: &serde_json::Value,
    path_guard: &PathGuard,
    before_write_hook: Option<&BeforeWriteHook>,
) -> Result<String, CoreError> {
    let from = args
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("move_file requires `from`".to_string()))?;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("move_file requires `to`".to_string()))?;

    let src = path_guard.resolve_existing(from)?;
    if src.is_dir() {
        return Err(CoreError::Tool(format!(
            "move_file cannot move directory `{from}`; only regular files are supported"
        )));
    }
    let dst = path_guard.resolve_for_write(to)?;
    if src == dst {
        return Err(CoreError::Tool(format!(
            "move_file source and destination are identical (`{from}`)"
        )));
    }

    // Snapshot both source (original bytes) and destination (None or overwritten bytes)
    // so `/undo` and `/diff` capture renames and moves completely.
    if let Some(hook) = before_write_hook {
        hook(&src).map_err(CoreError::BeforeWriteHook)?;
        hook(&dst).map_err(CoreError::BeforeWriteHook)?;
    }

    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| CoreError::Tool(format!("move_file create parent dir failed: {e}")))?;
    }

    if let Err(rename_err) = fs::rename(&src, &dst) {
        // Fallback across mount boundaries: copy + remove.
        fs::copy(&src, &dst).map_err(|e| {
            CoreError::Tool(format!("move_file failed (rename: {rename_err}; copy: {e})"))
        })?;
        fs::remove_file(&src)
            .map_err(|e| CoreError::Tool(format!("move_file cleanup failed: {e}")))?;
    }

    Ok(format!("moved {from} -> {to}"))
}

fn exec_grep(
    args: &serde_json::Value,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
) -> Result<String, CoreError> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("grep requires `pattern`".to_string()))?;
    if pattern.is_empty() {
        return Err(CoreError::Tool(
            "grep `pattern` must not be empty".to_string(),
        ));
    }
    let rel_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let include = args.get("include").and_then(|v| v.as_str());
    let case_sensitive = args
        .get("case_sensitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(1, MAX_GREP_LIMIT))
        .unwrap_or(DEFAULT_GREP_LIMIT);

    let search_root = path_guard.resolve_existing(rel_path)?;
    let workspace_root = path_guard.root();
    let needle = if case_sensitive {
        pattern.to_string()
    } else {
        pattern.to_lowercase()
    };

    let mut matches = Vec::new();
    let mut truncated = false;

    let mut builder = WalkBuilder::new(&search_root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|e| e.file_name() != ".git");

    for entry in builder.build() {
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        if let Some(inc) = include {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            if !glob_match(inc, &rel) && !glob_match(inc, &file_name) {
                continue;
            }
        }

        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_GREP_FILE_BYTES {
            continue;
        }

        let Ok(bytes) = fs::read(path) else { continue };
        // Skip binary files containing NUL bytes
        if bytes.iter().take(4096).any(|&b| b == 0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };

        for (idx, line) in text.lines().enumerate() {
            let found = if case_sensitive {
                line.contains(&needle)
            } else {
                line.to_lowercase().contains(&needle)
            };
            if found {
                let snippet: String = line.chars().take(200).collect();
                matches.push(format!("{rel}:{}: {snippet}", idx + 1));
                if matches.len() >= max_results {
                    truncated = true;
                    break;
                }
            }
        }
        if truncated {
            break;
        }
    }

    if matches.is_empty() {
        return Ok(format!("No matches for `{pattern}` in `{rel_path}`."));
    }
    if truncated {
        matches.push(format!(
            "... (truncated at {max_results} matches; narrow `path` or `include`)"
        ));
    }
    Ok(matches.join("\n"))
}

fn exec_glob(
    args: &serde_json::Value,
    path_guard: &PathGuard,
    cancel: &CancellationToken,
) -> Result<String, CoreError> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Tool("glob requires `pattern`".to_string()))?;
    if pattern.is_empty() {
        return Err(CoreError::Tool(
            "glob `pattern` must not be empty".to_string(),
        ));
    }
    let rel_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(1, MAX_GLOB_LIMIT))
        .unwrap_or(DEFAULT_GLOB_LIMIT);

    let search_root = path_guard.resolve_existing(rel_path)?;
    let workspace_root = path_guard.root();
    let match_basename_only = !pattern.contains('/');

    let mut found = Vec::new();
    let mut builder = WalkBuilder::new(&search_root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|e| e.file_name() != ".git");

    for entry in builder.build() {
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        let matched = if match_basename_only {
            let base = path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            glob_match(pattern, &base) || glob_match(pattern, &rel)
        } else {
            glob_match(pattern, &rel)
        };

        if matched {
            found.push(rel);
        }
    }

    found.sort();
    let total = found.len();
    if total == 0 {
        return Ok(format!("No files matching `{pattern}` in `{rel_path}`."));
    }
    if total > max_results {
        found.truncate(max_results);
        found.push(format!(
            "... (showing {max_results} of {total} matching files)"
        ));
    }
    Ok(found.join("\n"))
}

/// Lightweight glob matcher supporting `*` (single segment), `**` (any path), and `?`.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_slice(&p, &t)
}

fn glob_match_slice(p: &[char], t: &[char]) -> bool {
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_p: Option<usize> = None;
    let mut star_t: usize = 0;
    let mut star_double = false;

    while ti < t.len() {
        if pi + 1 < p.len() && p[pi] == '*' && p[pi + 1] == '*' {
            // Collapse consecutive `*` and optional following `/`
            while pi < p.len() && p[pi] == '*' {
                pi += 1;
            }
            if pi < p.len() && p[pi] == '/' {
                // Allow `**/` to match zero directories too
                if glob_match_slice(&p[pi + 1..], &t[ti..]) {
                    return true;
                }
            }
            star_p = Some(pi);
            star_t = ti;
            star_double = true;
        } else if pi < p.len() && p[pi] == '*' {
            pi += 1;
            star_p = Some(pi);
            star_t = ti;
            star_double = false;
        } else if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if let Some(sp) = star_p {
            if !star_double && t[star_t] == '/' {
                return false;
            }
            star_t += 1;
            ti = star_t;
            pi = sp;
        } else {
            return false;
        }
    }

    while pi < p.len() && (p[pi] == '*' || (p[pi] == '/' && pi > 0 && p[pi - 1] == '*')) {
        pi += 1;
    }
    pi == p.len()
}

/// Cap text to `head_bytes + tail_bytes` using a smart head+tail split that preserves
/// the beginning and end (e.g. compiler errors) while respecting UTF-8 char boundaries.
pub fn truncate_head_tail(text: &str, head_bytes: usize, tail_bytes: usize) -> String {
    let max_bytes = head_bytes.saturating_add(tail_bytes);
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let mut head_end = head_bytes.min(text.len());
    while head_end > 0 && !text.is_char_boundary(head_end) {
        head_end -= 1;
    }

    let mut tail_start = text.len().saturating_sub(tail_bytes);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }

    if tail_start <= head_end {
        return text.to_string();
    }

    let truncated = tail_start - head_end;
    format!(
        "{}\n... [truncated {truncated} bytes] ...\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

async fn run_cancellable_shell(
    command: &str,
    cwd: &Path,
    timeout_ms: u64,
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

    // Drain stdout/stderr concurrently so large outputs never deadlock the pipe buffer.
    let stdout_task = child.stdout.take().map(|mut s| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
            buf
        })
    });
    let stderr_task = child.stderr.take().map(|mut s| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
            buf
        })
    });

    let timeout_dur = std::time::Duration::from_millis(timeout_ms);
    let timed_out;
    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = kill_process_tree(&mut child).await;
            if let Some(t) = stdout_task { t.abort(); }
            if let Some(t) = stderr_task { t.abort(); }
            return Err(CoreError::Cancelled);
        }
        _ = tokio::time::sleep(timeout_dur) => {
            let _ = kill_process_tree(&mut child).await;
            timed_out = true;
            None
        }
        res = child.wait() => {
            timed_out = false;
            Some(res.map_err(|e| CoreError::Tool(format!("command wait failed: {e}")))?)
        }
    };

    let drain_timeout = std::time::Duration::from_millis(500);
    let out = match stdout_task {
        Some(t) => tokio::time::timeout(drain_timeout, t)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let err = match stderr_task {
        Some(t) => tokio::time::timeout(drain_timeout, t)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default(),
        None => Vec::new(),
    };

    let stdout_str = String::from_utf8_lossy(&out);
    let stderr_str = String::from_utf8_lossy(&err);

    let body = if stderr_str.is_empty() {
        stdout_str.into_owned()
    } else if stdout_str.is_empty() {
        format!("stderr:\n{stderr_str}")
    } else {
        format!("{stdout_str}\nstderr:\n{stderr_str}")
    };

    let capped_body = truncate_head_tail(&body, SHELL_OUTPUT_HEAD_BYTES, SHELL_OUTPUT_TAIL_BYTES);

    if timed_out {
        if capped_body.is_empty() {
            Ok(format!(
                "exit=timeout\nCommand timed out after {timeout_ms}ms and its process group was terminated."
            ))
        } else {
            Ok(format!(
                "exit=timeout\nCommand timed out after {timeout_ms}ms and its process group was terminated.\n{capped_body}"
            ))
        }
    } else {
        let code = status.and_then(|s| s.code()).unwrap_or(-1);
        Ok(format!("exit={code}\n{capped_body}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn edit_file_replaces_unique_and_invokes_checkpoint_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("demo.rs");
        fs::write(&file_path, "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n").unwrap();

        let guard = PathGuard::new(tmp.path()).unwrap();
        let hook_hits = Arc::new(AtomicUsize::new(0));
        let hook_hits_clone = Arc::clone(&hook_hits);
        let hook: BeforeWriteHook = Arc::new(move |_| {
            hook_hits_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        let args = serde_json::json!({
            "path": "demo.rs",
            "old_string": "let x = 1;",
            "new_string": "let x = 42;"
        })
        .to_string();
        let res = execute_tool(
            "edit_file",
            &args,
            &guard,
            Some(&hook),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(res.contains("replaced 1 occurrence"));
        assert_eq!(hook_hits.load(Ordering::SeqCst), 1);
        assert!(fs::read_to_string(&file_path).unwrap().contains("let x = 42;"));
    }

    #[tokio::test]
    async fn edit_file_rejects_ambiguous_unless_replace_all() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("dup.txt"), "foo\nbar\nfoo\n").unwrap();
        let guard = PathGuard::new(tmp.path()).unwrap();

        let err = execute_tool(
            "edit_file",
            r#"{"path":"dup.txt","old_string":"foo","new_string":"baz"}"#,
            &guard,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("matched 2 times"));

        let ok = execute_tool(
            "edit_file",
            r#"{"path":"dup.txt","old_string":"foo","new_string":"baz","replace_all":true}"#,
            &guard,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(ok.contains("replaced 2 occurrence"));
        assert_eq!(
            fs::read_to_string(tmp.path().join("dup.txt")).unwrap(),
            "baz\nbar\nbaz\n"
        );
    }

    #[tokio::test]
    async fn read_file_supports_offset_and_limit() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("lines.txt"), "a\nb\nc\nd\ne\n").unwrap();
        let guard = PathGuard::new(tmp.path()).unwrap();

        let out = execute_tool(
            "read_file",
            r#"{"path":"lines.txt","offset":2,"limit":2}"#,
            &guard,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(out.contains("2 | b"));
        assert!(out.contains("3 | c"));
        assert!(out.contains("offset=4"));
    }

    #[tokio::test]
    async fn grep_and_glob_work_in_read_only_mode() {
        assert_eq!(classify_tool_risk("grep"), ToolRisk::ReadOnly);
        assert_eq!(classify_tool_risk("glob"), ToolRisk::ReadOnly);
        assert_eq!(classify_tool_risk("edit_file"), ToolRisk::Mutating);

        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/lib.rs"), "pub fn hello() {}\n").unwrap();
        fs::write(tmp.path().join("README.md"), "# Hello\n").unwrap();
        let guard = PathGuard::new(tmp.path()).unwrap();

        let grep_out = execute_tool(
            "grep",
            r#"{"pattern":"hello","case_sensitive":false,"include":"*.rs"}"#,
            &guard,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(grep_out.contains("src/lib.rs:1: pub fn hello() {}"));
        assert!(!grep_out.contains("README.md"));

        let glob_out = execute_tool(
            "glob",
            r#"{"pattern":"**/*.rs"}"#,
            &guard,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(glob_out.trim(), "src/lib.rs");
    }

    #[test]
    fn truncate_head_tail_preserves_head_and_tail_safely() {
        let short = "hello world";
        assert_eq!(truncate_head_tail(short, 6 * 1024, 10 * 1024), short);

        let head = "A".repeat(7_000);
        let middle = "M".repeat(10_000);
        let tail = "Z".repeat(11_000);
        let full = format!("{head}{middle}{tail}");
        let capped = truncate_head_tail(&full, 6 * 1024, 10 * 1024);
        assert!(capped.starts_with(&"A".repeat(6 * 1024)));
        assert!(capped.ends_with(&"Z".repeat(10 * 1024)));
        assert!(capped.contains("[truncated "));
        assert!(capped.len() < full.len());
    }

    #[tokio::test]
    async fn run_command_times_out_and_kills_child() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path()).unwrap();
        let cmd = if cfg!(windows) {
            "ping -n 10 127.0.0.1 >nul"
        } else {
            "sleep 10"
        };
        let args = serde_json::json!({
            "command": cmd,
            "timeout_ms": 150
        })
        .to_string();
        let out = execute_tool("run_command", &args, &guard, None, &CancellationToken::new())
            .await
            .unwrap();
        assert!(out.contains("exit=timeout"));
        assert!(out.contains("150ms"));
    }

    #[tokio::test]
    async fn delete_file_and_move_file_invoke_checkpoint_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("old.txt");
        fs::write(&src, "hello move").unwrap();
        let guard = PathGuard::new(tmp.path()).unwrap();

        let hook_hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = Arc::clone(&hook_hits);
        let hook: BeforeWriteHook = Arc::new(move |_| {
            hits_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        let mv_out = execute_tool(
            "move_file",
            r#"{"from":"old.txt","to":"nested/new.txt"}"#,
            &guard,
            Some(&hook),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(mv_out.contains("moved old.txt -> nested/new.txt"));
        assert!(!src.exists());
        assert_eq!(
            fs::read_to_string(tmp.path().join("nested/new.txt")).unwrap(),
            "hello move"
        );
        // move_file snapshots both source and destination
        assert_eq!(hook_hits.load(Ordering::SeqCst), 2);

        let del_out = execute_tool(
            "delete_file",
            r#"{"path":"nested/new.txt"}"#,
            &guard,
            Some(&hook),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(del_out.contains("deleted nested/new.txt"));
        assert!(!tmp.path().join("nested/new.txt").exists());
        assert_eq!(hook_hits.load(Ordering::SeqCst), 3);
    }
}
