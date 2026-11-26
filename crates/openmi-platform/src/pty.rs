use std::path::Path;
use tokio::process::Command;

use crate::job_object::configure_process_group;

/// Output captured from a shell command invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run a shell command in `cwd` using the platform's default shell (`sh -c` on Unix/Termux, `cmd /C` on Windows).
pub async fn run_shell_capture(command: &str, cwd: &Path) -> std::io::Result<CommandOutput> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };

    cmd.current_dir(cwd);
    configure_process_group(&mut cmd);

    let output = cmd.output().await?;
    Ok(CommandOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_simple_echo_command() {
        let tmp = tempfile::tempdir().unwrap();
        let out = run_shell_capture("echo openmi-ok", tmp.path())
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("openmi-ok"));
    }
}
