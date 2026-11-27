use std::env;
use std::process::{Command, Stdio};

/// Best-effort cross-platform browser launcher (Android/Termux, macOS, Windows, Linux/WSL).
/// Spawns detached with stdio silenced so it never blocks or corrupts the terminal/TUI.
/// Returns `true` if a launcher command was spawned successfully.
pub fn open_url(url: &str) -> bool {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return false;
    }

    // Termux / Android: prefer `termux-open-url`, falling back to `am start`.
    if cfg!(target_os = "android") || env::var_os("TERMUX_VERSION").is_some() {
        if spawn_silent("termux-open-url", &[trimmed]) {
            return true;
        }
        return spawn_silent(
            "am",
            &["start", "-a", "android.intent.action.VIEW", "-d", trimmed],
        );
    }

    #[cfg(target_os = "macos")]
    {
        spawn_silent("open", &[trimmed])
    }

    #[cfg(target_os = "windows")]
    {
        // `cmd /C start "" <url>` opens the default browser without treating the URL as a window title.
        spawn_silent("cmd", &["/C", "start", "", trimmed])
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // On WSL, `wslview` or `explorer.exe` opens the Windows host browser.
        if env::var_os("WSL_DISTRO_NAME").is_some() && spawn_silent("wslview", &[trimmed]) {
            return true;
        }
        spawn_silent("xdg-open", &[trimmed])
    }
}

fn spawn_silent(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_urls() {
        assert!(!open_url("javascript:alert(1)"));
        assert!(!open_url("file:///etc/passwd"));
        assert!(!open_url(""));
    }
}
