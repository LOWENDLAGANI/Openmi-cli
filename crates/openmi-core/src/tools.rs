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

/// Risk classification for built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    ReadOnly,
    Mutating,
}

pub fn classify_tool_risk(tool_name: &str) -> ToolRisk {
    match tool_name {
        "read_file" | "list_dir" | "grep" | "glob" => ToolRisk::ReadOnly,
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
        "edit_file" => exec_edit_file(&args, path_guard, before_write_hook),
        "write_file" => exec_write_file(&args, path_guard, before_write_hook),
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
}
