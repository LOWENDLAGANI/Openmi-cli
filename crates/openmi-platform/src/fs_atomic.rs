use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

/// Atomically write `bytes` to `path` by writing to a sibling temp file and renaming.
///
/// Preserves exact byte sequences (including UTF-8 BOM and CRLF line endings).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_inner(path, bytes, false)
}

/// Atomically write `bytes` to `path` with owner-only (`0600` on Unix) permissions.
pub fn atomic_write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_inner(path, bytes, true)
}

fn write_atomic_inner(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;

    let mut tmp = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = private;

    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_preserves_bom_and_crlf() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("nested/file.txt");
        let payload = b"\xEF\xBB\xBFline1\r\nline2\r\n";

        atomic_write(&target, payload).unwrap();
        let read_back = fs::read(&target).unwrap();
        assert_eq!(read_back, payload);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_private_sets_0600_mode() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("token.json");
        atomic_write_private(&target, b"{\"token\":\"secret\"}").unwrap();

        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
