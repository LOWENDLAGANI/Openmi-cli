use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use openmi_platform::job_object::{configure_process_group, kill_process_tree};

const DEFAULT_HOOK_TIMEOUT_MS: u64 = 30_000;
const MAX_HOOK_TIMEOUT_MS: u64 = 120_000;

/// A single shell hook rule triggered on `PreToolUse`, `PostToolUse`, or `Stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRule {
    /// Tool name matcher: `*` (all), exact name, glob, or `|`-separated list
    /// (e.g. `"edit_file|write_file"`). Ignored for `Stop` hooks if empty.
    #[serde(default)]
    pub matcher: String,
    /// Shell command to execute in the workspace root.
    pub command: String,
    /// Optional timeout in milliseconds (default 30_000, max 120_000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Lifecycle hooks configuration loaded from `~/.config/openmi/settings.json`
/// and `.openmi/settings.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default, rename = "PreToolUse", alias = "pre_tool_use")]
    pub pre_tool_use: Vec<HookRule>,
    #[serde(default, rename = "PostToolUse", alias = "post_tool_use")]
    pub post_tool_use: Vec<HookRule>,
    #[serde(default, rename = "Stop", alias = "stop")]
    pub stop: Vec<HookRule>,
}

impl HooksConfig {
    pub fn is_empty(&self) -> bool {
        self.pre_tool_use.is_empty() && self.post_tool_use.is_empty() && self.stop.is_empty()
    }

    pub fn merge(&self, other: &HooksConfig) -> HooksConfig {
        let mut merged = self.clone();
        merged.pre_tool_use.extend(other.pre_tool_use.iter().cloned());
        merged
            .post_tool_use
            .extend(other.post_tool_use.iter().cloned());
        merged.stop.extend(other.stop.iter().cloned());
        merged
    }
}

/// Outcome of running a lifecycle hook batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// All matching hooks succeeded (exit 0); optional combined stdout/stderr notes.
    Allow(Option<String>),
    /// A hook exited non-zero, blocking execution (for `PreToolUse`) or reporting failure.
    Block(String),
}

/// Check if a hook `matcher` matches `tool_name`.
/// Empty or `*` matches all tools; `|` separates alternatives (e.g. `"edit_file|write_file"`).
pub fn hook_matcher_matches(matcher: &str, tool_name: &str) -> bool {
    let trimmed = matcher.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return true;
    }
    trimmed.split('|').any(|part| {
        let pat = part.trim();
        !pat.is_empty() && crate::tools::glob_match(pat, tool_name)
    })
}

/// Execute matching `PreToolUse` hooks. Stops on the first non-zero exit code and returns `Block`.
pub async fn run_pre_tool_hooks(
    hooks: &[HookRule],
    tool_name: &str,
    arguments_json: &str,
    cwd: &Path,
    cancel: &CancellationToken,
) -> HookOutcome {
    let payload = serde_json::json!({
        "event": "PreToolUse",
        "tool_name": tool_name,
        "arguments": serde_json::from_str::<serde_json::Value>(arguments_json)
            .unwrap_or(serde_json::Value::String(arguments_json.to_string())),
    })
    .to_string();

    for rule in hooks {
        if !hook_matcher_matches(&rule.matcher, tool_name) {
            continue;
        }
        match exec_single_hook(rule, "PreToolUse", Some(tool_name), &payload, cwd, cancel).await {
            Ok((0, _)) => {}
            Ok((code, feedback)) => {
                let msg = if feedback.trim().is_empty() {
                    format!("PreToolUse hook `{}` blocked `{tool_name}` (exit {code})", rule.command)
                } else {
                    format!("PreToolUse hook blocked `{tool_name}` (exit {code}): {}", feedback.trim())
                };
                return HookOutcome::Block(msg);
            }
            Err(err) => {
                return HookOutcome::Block(format!(
                    "PreToolUse hook `{}` failed: {err}",
                    rule.command
                ));
            }
        }
    }
    HookOutcome::Allow(None)
}

/// Execute matching `PostToolUse` hooks after a tool succeeds.
pub async fn run_post_tool_hooks(
    hooks: &[HookRule],
    tool_name: &str,
    arguments_json: &str,
    tool_output: &str,
    cwd: &Path,
    cancel: &CancellationToken,
) -> Option<String> {
    let payload = serde_json::json!({
        "event": "PostToolUse",
        "tool_name": tool_name,
        "arguments": serde_json::from_str::<serde_json::Value>(arguments_json)
            .unwrap_or(serde_json::Value::String(arguments_json.to_string())),
        "output": tool_output.chars().take(4096).collect::<String>(),
    })
    .to_string();

    let mut notes = Vec::new();
    for rule in hooks {
        if !hook_matcher_matches(&rule.matcher, tool_name) {
            continue;
        }
        match exec_single_hook(rule, "PostToolUse", Some(tool_name), &payload, cwd, cancel).await {
            Ok((0, _)) => {}
            Ok((code, feedback)) => {
                let detail = feedback.trim();
                if detail.is_empty() {
                    notes.push(format!("[PostToolUse hook `{}` exited {code}]", rule.command));
                } else {
                    notes.push(format!(
                        "[PostToolUse hook `{}` exited {code}: {detail}]",
                        rule.command
                    ));
                }
            }
            Err(err) => {
                notes.push(format!("[PostToolUse hook `{}` error: {err}]", rule.command));
            }
        }
    }
    if notes.is_empty() {
        None
    } else {
        Some(notes.join("\n"))
    }
}

/// Execute `Stop` hooks when an agent turn completes.
pub async fn run_stop_hooks(
    hooks: &[HookRule],
    finish_reason: &str,
    cwd: &Path,
    cancel: &CancellationToken,
) {
    if hooks.is_empty() {
        return;
    }
    let payload = serde_json::json!({
        "event": "Stop",
        "finish_reason": finish_reason,
    })
    .to_string();

    for rule in hooks {
        let _ = exec_single_hook(rule, "Stop", None, &payload, cwd, cancel).await;
    }
}

async fn exec_single_hook(
    rule: &HookRule,
    event_name: &str,
    tool_name: Option<&str>,
    stdin_json: &str,
    cwd: &Path,
    cancel: &CancellationToken,
) -> Result<(i32, String), String> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", &rule.command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &rule.command]);
        c
    };

    cmd.current_dir(cwd)
        .env("OPENMI_HOOK_EVENT", event_name)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(tool) = tool_name {
        cmd.env("OPENMI_TOOL_NAME", tool);
    }
    configure_process_group(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn hook: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_json.as_bytes()).await;
        drop(stdin);
    }

    let stderr_task = child.stderr.take().map(|mut s| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
            buf
        })
    });
    let stdout_task = child.stdout.take().map(|mut s| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
            buf
        })
    });

    let timeout_ms = rule
        .timeout_ms
        .map(|ms| ms.clamp(100, MAX_HOOK_TIMEOUT_MS))
        .unwrap_or(DEFAULT_HOOK_TIMEOUT_MS);

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = kill_process_tree(&mut child).await;
            return Err("cancelled".to_string());
        }
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {
            let _ = kill_process_tree(&mut child).await;
            return Err(format!("timed out after {timeout_ms}ms"));
        }
        res = child.wait() => {
            res.map_err(|e| format!("wait failed: {e}"))?
        }
    };

    let stderr = match stderr_task {
        Some(t) => t.await.ok().unwrap_or_default(),
        None => Vec::new(),
    };
    let stdout = match stdout_task {
        Some(t) => t.await.ok().unwrap_or_default(),
        None => Vec::new(),
    };
    let stderr_str = String::from_utf8_lossy(&stderr).into_owned();
    let stdout_str = String::from_utf8_lossy(&stdout).into_owned();
    let feedback = if !stderr_str.trim().is_empty() {
        stderr_str
    } else {
        stdout_str
    };
    Ok((status.code().unwrap_or(-1), feedback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_matcher_handles_pipe_and_glob() {
        assert!(hook_matcher_matches("*", "edit_file"));
        assert!(hook_matcher_matches("", "run_command"));
        assert!(hook_matcher_matches("edit_file|write_file", "write_file"));
        assert!(!hook_matcher_matches("edit_file|write_file", "run_command"));
        assert!(hook_matcher_matches("mcp__*", "mcp__fs__read"));
    }

    #[tokio::test]
    async fn pre_tool_hook_blocks_on_nonzero_exit_with_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks = vec![HookRule {
            matcher: "run_command".into(),
            command: "echo 'blocked by policy' >&2; exit 2".into(),
            timeout_ms: Some(5000),
        }];
        let out = run_pre_tool_hooks(
            &hooks,
            "run_command",
            r#"{"command":"echo hi"}"#,
            tmp.path(),
            &CancellationToken::new(),
        )
        .await;
        match out {
            HookOutcome::Block(msg) => {
                assert!(msg.contains("blocked by policy"));
                assert!(msg.contains("exit 2"));
            }
            HookOutcome::Allow(_) => panic!("expected PreToolUse hook to block"),
        }
    }

    #[tokio::test]
    async fn post_tool_hook_runs_and_reports_warning_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks = vec![HookRule {
            matcher: "edit_file|write_file".into(),
            command: "echo 'format failed' >&2; exit 1".into(),
            timeout_ms: Some(5000),
        }];
        let note = run_post_tool_hooks(
            &hooks,
            "edit_file",
            r#"{"path":"a.rs"}"#,
            "edited a.rs",
            tmp.path(),
            &CancellationToken::new(),
        )
        .await
        .expect("expected warning note");
        assert!(note.contains("format failed"));
    }
}
