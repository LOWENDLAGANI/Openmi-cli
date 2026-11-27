use tokio::process::{Child, Command};

/// Configure a `tokio::process::Command` so its spawned child and any grandchildren
/// can be cleanly terminated together on cancellation.
pub fn configure_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        // Windows job-object binding can be layered here without changing callers.
        let _ = cmd;
    }
}

/// Terminate a child process and, on Unix, its entire process group.
pub async fn kill_process_tree(child: &mut Child) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: sending SIGKILL to the process group (-pgid) created via process_group(0).
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    Ok(())
}

/// Best-effort termination of a Unix process group by leader PID (no-op on non-Unix).
pub fn kill_process_group_pid(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: sending SIGKILL to the process group (-pgid) created via process_group(0).
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn kill_process_tree_stops_running_child() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 10 127.0.0.1 >nul"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 30"]);
            c
        };

        configure_process_group(&mut cmd);
        let mut child = cmd.spawn().expect("spawn sleep");
        kill_process_tree(&mut child).await.unwrap();
        assert!(child.try_wait().unwrap().is_some());
    }
}
