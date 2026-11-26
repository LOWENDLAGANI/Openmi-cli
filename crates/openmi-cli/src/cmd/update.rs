use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, ensure, Context, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::exit_codes::CliError;

pub const DEFAULT_RELEASE_API_URL: &str =
    "https://api.github.com/repos/LOWENDLAGANI/Openmi-cli/releases/latest";

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

/// Return the release asset filename matching the currently running OS/architecture.
pub fn current_target_asset_name() -> &'static str {
    #[cfg(all(target_os = "android", target_arch = "aarch64"))]
    {
        "openmi-aarch64-linux-android.tar.gz"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "openmi-x86_64-unknown-linux-gnu.tar.gz"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "openmi-aarch64-unknown-linux-gnu.tar.gz"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "openmi-x86_64-apple-darwin.tar.gz"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "openmi-aarch64-apple-darwin.tar.gz"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "openmi-x86_64-pc-windows-msvc.zip"
    }
    #[cfg(not(any(
        all(target_os = "android", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    {
        "openmi-unknown.tar.gz"
    }
}

/// Compare two semver-like strings (`"0.1.0"` vs `"v0.2.0"`). Returns `true` if `latest` > `current`.
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .split(['.', '-'])
            .take(3)
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    parse(latest) > parse(current)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn find_expected_sha256(checksums_text: &str, asset_name: &str) -> Option<String> {
    for line in checksums_text.lines() {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        if file == asset_name || file.ends_with(&format!("/{asset_name}")) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

pub const AUTO_UPDATE_DISABLE_ENV: &str = "OPENMI_NO_AUTO_UPDATE";
const AUTO_UPDATED_GUARD_ENV: &str = "OPENMI_AUTO_UPDATED";

fn auto_update_disabled() -> bool {
    if std::env::var(AUTO_UPDATED_GUARD_ENV).as_deref() == Ok("1") {
        return true;
    }
    if let Ok(val) = std::env::var(AUTO_UPDATE_DISABLE_ENV) {
        let trimmed = val.trim().to_ascii_lowercase();
        if !trimmed.is_empty() && trimmed != "0" && trimmed != "false" {
            return true;
        }
    }
    false
}

/// Check for a newer release before booting the CLI.
///
/// Uses a short 3-second timeout for the version check so offline or slow networks never
/// stall startup. If a newer release exists, downloads, verifies SHA-256, replaces the
/// binary in place, and re-launches the updated binary with the original arguments.
/// Returns `Some(exit_code)` when the updated binary was re-launched and has finished;
/// returns `None` to continue booting the current binary normally.
pub async fn auto_update_before_boot() -> Option<std::process::ExitCode> {
    if auto_update_disabled() {
        return None;
    }

    let current_exe = std::env::current_exe().ok()?;
    let url = std::env::var("OPENMI_RELEASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_API_URL.to_string());
    let current_version = env!("CARGO_PKG_VERSION");

    // Fast metadata check — fail open if offline or GitHub is slow/rate-limited.
    let check_client = reqwest::Client::builder()
        .user_agent(format!("openmi-cli/{current_version}"))
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;

    let release: ReleaseInfo = check_client
        .get(&url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;

    if !is_newer_version(current_version, &release.tag_name) {
        return None;
    }

    let latest = release.tag_name.trim_start_matches('v');
    eprintln!("Updating openmi v{current_version} -> v{latest}...");

    let download_client = reqwest::Client::builder()
        .user_agent(format!("openmi-cli/{current_version}"))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .ok()?;

    if let Err(err) = install_release(&download_client, &release, &current_exe).await {
        eprintln!("Warning: auto-update failed ({err}); continuing with v{current_version}.");
        return None;
    }

    eprintln!("Updated openmi to v{latest}. Starting new version...");

    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match std::process::Command::new(&current_exe)
        .args(&args)
        .env(AUTO_UPDATED_GUARD_ENV, "1")
        .status()
    {
        Ok(status) => {
            let code = status.code().unwrap_or(1).clamp(0, 255) as u8;
            Some(std::process::ExitCode::from(code))
        }
        Err(err) => {
            eprintln!("Warning: failed to restart updated binary ({err}); continuing.");
            None
        }
    }
}

pub async fn execute(check_only: bool, release_url: Option<String>) -> Result<(), CliError> {
    let current_exe = std::env::current_exe().map_err(anyhow::Error::from)?;
    let url = release_url
        .or_else(|| std::env::var("OPENMI_RELEASE_URL").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_API_URL.to_string());

    run_update(&url, env!("CARGO_PKG_VERSION"), check_only, &current_exe)
        .await
        .map(|_| ())
        .map_err(CliError::Other)
}

pub async fn run_update(
    release_api_url: &str,
    current_version: &str,
    check_only: bool,
    target_binary: &Path,
) -> Result<bool> {
    let client = reqwest::Client::builder()
        .user_agent(format!("openmi-cli/{current_version}"))
        .build()?;

    let release: ReleaseInfo = client
        .get(release_api_url)
        .send()
        .await
        .context("failed to query release metadata")?
        .error_for_status()
        .context("release metadata returned non-success status")?
        .json()
        .await
        .context("failed to parse release JSON")?;

    let latest = release.tag_name.trim_start_matches('v');
    if !is_newer_version(current_version, &release.tag_name) {
        println!("openmi is already up to date (v{current_version}).");
        return Ok(false);
    }

    println!("Update available: v{current_version} -> v{latest}");
    if check_only {
        println!("Run `openmi update` to install v{latest}.");
        return Ok(true);
    }

    install_release(&client, &release, target_binary).await?;
    println!(
        "Updated openmi to v{latest} at {}.",
        target_binary.display()
    );
    Ok(true)
}

async fn install_release(
    client: &reqwest::Client,
    release: &ReleaseInfo,
    target_binary: &Path,
) -> Result<()> {
    let asset_name = current_target_asset_name();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow!("no release artifact found for target `{asset_name}`"))?;

    let archive_bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    // Verify SHA-256 if SHA256SUMS.txt or `<asset>.sha256` is published alongside the release.
    if let Some(sums_asset) = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS.txt" || a.name == format!("{asset_name}.sha256"))
    {
        let sums_txt = client
            .get(&sums_asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        if let Some(expected) = find_expected_sha256(&sums_txt, asset_name) {
            let actual = sha256_hex(&archive_bytes);
            ensure!(
                actual == expected,
                "checksum mismatch for {asset_name}: expected {expected}, got {actual}"
            );
        }
    }

    let binary_bytes = if asset_name.ends_with(".tar.gz") {
        extract_binary_from_tar_gz(&archive_bytes)?
    } else {
        archive_bytes.to_vec()
    };

    replace_executable(target_binary, &binary_bytes)
}

fn extract_binary_from_tar_gz(archive_bytes: &[u8]) -> Result<Vec<u8>> {
    let gz = GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "openmi" || name == "openmi.exe" {
            let mut out = Vec::new();
            entry.read_to_end(&mut out)?;
            return Ok(out);
        }
    }
    Err(anyhow!(
        "release archive did not contain an `openmi` binary"
    ))
}

fn replace_executable(target: &Path, new_bytes: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("cannot determine parent directory of {}", target.display()))?;
    fs::create_dir_all(parent)?;

    let staged = parent.join(format!(".openmi-update-{}.tmp", std::process::id()));
    fs::write(&staged, new_bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        fs::rename(&staged, target)?;
    }

    #[cfg(windows)]
    {
        let backup = parent.join("openmi.exe.old");
        let _ = fs::remove_file(&backup);
        if target.exists() {
            fs::rename(target, &backup)?;
        }
        fs::rename(&staged, target)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn compares_semver_versions_correctly() {
        assert!(is_newer_version("0.1.0", "v0.2.0"));
        assert!(is_newer_version("0.1.0", "0.1.1"));
        assert!(!is_newer_version("0.2.0", "v0.2.0"));
        assert!(!is_newer_version("0.2.0", "v0.1.9"));
    }

    #[tokio::test]
    async fn updates_and_verifies_sha256_against_mock_release_server() {
        // Build an in-memory `.tar.gz` containing a fake `openmi` binary.
        let new_bin = b"#!/bin/sh\necho openmi-0.2.0\n";
        let asset_name = current_target_asset_name().to_string();
        let archive_bytes = if asset_name.ends_with(".tar.gz") {
            let mut tar_buf = Vec::new();
            {
                let gz = GzEncoder::new(&mut tar_buf, Compression::default());
                let mut builder = tar::Builder::new(gz);
                let mut header = tar::Header::new_gnu();
                header.set_size(new_bin.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder
                    .append_data(&mut header, "openmi", &new_bin[..])
                    .unwrap();
                builder.into_inner().unwrap().finish().unwrap();
            }
            Arc::new(tar_buf)
        } else {
            Arc::new(new_bin.to_vec())
        };
        let expected_sha = sha256_hex(&archive_bytes);
        let sums_body = format!("{expected_sha}  {asset_name}\n");

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");

        let release_json = serde_json::json!({
            "tag_name": "v0.2.0",
            "assets": [
                {
                    "name": asset_name,
                    "browser_download_url": format!("{base}/download/archive")
                },
                {
                    "name": "SHA256SUMS.txt",
                    "browser_download_url": format!("{base}/download/sums")
                }
            ]
        })
        .to_string();

        let archive_srv = Arc::clone(&archive_bytes);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let rj = release_json.clone();
                let sb = sums_body.clone();
                let ab = Arc::clone(&archive_srv);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("");
                    let (ct, body): (&str, Vec<u8>) = match path {
                        "/latest" => ("application/json", rj.into_bytes()),
                        "/download/sums" => ("text/plain", sb.into_bytes()),
                        "/download/archive" => ("application/octet-stream", ab.as_ref().clone()),
                        _ => ("text/plain", b"not found".to_vec()),
                    };
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(header.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                });
            }
        });

        let tmp = tempfile::tempdir().unwrap();
        let fake_bin = tmp.path().join("openmi");
        fs::write(&fake_bin, b"old-binary").unwrap();

        let updated = run_update(&format!("{base}/latest"), "0.1.0", false, &fake_bin)
            .await
            .unwrap();
        assert!(updated);
        assert_eq!(fs::read(&fake_bin).unwrap(), new_bin);
    }
}
