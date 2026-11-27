use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use openmi_platform::job_object::configure_process_group;

use crate::session::CoreError;
use crate::tools::{truncate_head_tail, SHELL_OUTPUT_HEAD_BYTES, SHELL_OUTPUT_TAIL_BYTES};

const MAX_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct TaskState {
    command: String,
    pid: Option<u32>,
    running: bool,
    exit_code: Option<i32>,
    output: Vec<u8>,
}

impl TaskState {
    fn append_output(&mut self, chunk: &[u8]) {
        self.output.extend_from_slice(chunk);
        if self.output.len() > MAX_BUFFER_BYTES {
            let excess = self.output.len() - MAX_BUFFER_BYTES;
            self.output.drain(..excess);
        }
    }
}

/// Manages background shell jobs spawned via `run_command(..., run_in_background: true)`.
#[derive(Debug, Default)]
pub struct BackgroundTaskManager {
    next_id: u64,
    tasks: HashMap<String, Arc<Mutex<TaskState>>>,
}

impl BackgroundTaskManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a background shell command in its own process group and return its `task_id`.
    pub fn spawn(&mut self, command: &str, cwd: &Path) -> Result<String, CoreError> {
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
            .map_err(|e| CoreError::Tool(format!("failed to spawn background command: {e}")))?;

        let pid = child.id();
        self.next_id += 1;
        let task_id = format!("bg_{}", self.next_id);
        let state = Arc::new(Mutex::new(TaskState {
            command: command.to_string(),
            pid,
            running: true,
            exit_code: None,
            output: Vec::new(),
        }));

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Some(mut s) = stdout {
            let st = Arc::clone(&state);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut guard) = st.lock() {
                        guard.append_output(&buf[..n]);
                    }
                }
            });
        }

        if let Some(mut s) = stderr {
            let st = Arc::clone(&state);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut guard) = st.lock() {
                        guard.append_output(&buf[..n]);
                    }
                }
            });
        }

        let st_wait = Arc::clone(&state);
        tokio::spawn(async move {
            let code = child
                .wait()
                .await
                .ok()
                .and_then(|s| s.code())
                .unwrap_or(-1);
            if let Ok(mut guard) = st_wait.lock() {
                guard.running = false;
                guard.exit_code = Some(code);
            }
        });

        self.tasks.insert(task_id.clone(), state);
        Ok(task_id)
    }

    /// Read current status and buffered stdout/stderr for a background task.
    pub fn read_output(&self, task_id: &str) -> Result<String, CoreError> {
        let state = self
            .tasks
            .get(task_id)
            .ok_or_else(|| CoreError::Tool(format!("unknown background task_id `{task_id}`")))?;
        let guard = state
            .lock()
            .map_err(|_| CoreError::Tool("background task state poisoned".to_string()))?;

        let status_line = if guard.running {
            format!("task_id={task_id} status=running command=`{}`", guard.command)
        } else {
            format!(
                "task_id={task_id} status=exited exit={} command=`{}`",
                guard.exit_code.unwrap_or(-1),
                guard.command
            )
        };
        let raw_text = String::from_utf8_lossy(&guard.output);
        let capped = truncate_head_tail(
            &raw_text,
            SHELL_OUTPUT_HEAD_BYTES,
            SHELL_OUTPUT_TAIL_BYTES,
        );
        if capped.is_empty() {
            Ok(format!("{status_line}\n(no output yet)"))
        } else {
            Ok(format!("{status_line}\n{capped}"))
        }
    }

    /// Terminate any still-running background process groups.
    pub fn shutdown_all(&mut self) {
        for state in self.tasks.values() {
            if let Ok(mut guard) = state.lock() {
                if guard.running {
                    if let Some(pid) = guard.pid {
                        openmi_platform::job_object::kill_process_group_pid(pid);
                    }
                    guard.running = false;
                }
            }
        }
        self.tasks.clear();
    }
}

impl Drop for BackgroundTaskManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn background_task_spawns_and_returns_captured_output() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mgr = BackgroundTaskManager::new();
        let cmd = if cfg!(windows) {
            "echo bg-hello"
        } else {
            "echo bg-hello"
        };
        let id = mgr.spawn(cmd, tmp.path()).unwrap();
        assert!(id.starts_with("bg_"));

        // Give the tiny echo command a moment to flush and exit.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let out = mgr.read_output(&id).unwrap();
        assert!(out.contains("bg-hello"));
        assert!(out.contains("status=exited"));
    }
}
