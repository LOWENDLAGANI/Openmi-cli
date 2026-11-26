use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathGuardError {
    #[error("path escapes workspace root: {0}")]
    OutsideWorkspace(PathBuf),

    #[error("symlink traversal is not permitted: {0}")]
    SymlinkRejected(PathBuf),

    #[error("invalid path component in: {0}")]
    InvalidPath(PathBuf),

    #[error("I/O error resolving path: {0}")]
    Io(#[from] std::io::Error),
}

/// Enforces that file operations stay strictly within a canonical workspace root.
#[derive(Debug, Clone)]
pub struct PathGuard {
    root: PathBuf,
}

impl PathGuard {
    /// Construct a `PathGuard` anchored at the canonical form of `root`.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, PathGuardError> {
        let canonical = fs::canonicalize(root.as_ref())?;
        Ok(Self { root: canonical })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a path that must already exist, ensuring its canonical location is inside `root`.
    pub fn resolve_existing(&self, candidate: impl AsRef<Path>) -> Result<PathBuf, PathGuardError> {
        let joined = self.join_candidate(candidate.as_ref());
        let canonical = fs::canonicalize(&joined)?;
        if !canonical.starts_with(&self.root) {
            return Err(PathGuardError::OutsideWorkspace(
                candidate.as_ref().to_path_buf(),
            ));
        }
        Ok(canonical)
    }

    /// Resolve a path intended for writing (the leaf or intermediate dirs may not exist yet).
    ///
    /// Walks up to the nearest existing ancestor, canonicalizes it, verifies it is inside `root`,
    /// and rejects any `..` or absolute/prefix components in the non-existent tail.
    pub fn resolve_for_write(
        &self,
        candidate: impl AsRef<Path>,
    ) -> Result<PathBuf, PathGuardError> {
        let joined = self.join_candidate(candidate.as_ref());

        // Fast path if it already exists.
        if joined.exists() || fs::symlink_metadata(&joined).is_ok() {
            // If it's a dangling symlink, symlink_metadata succeeds while canonicalize fails.
            // Reject dangling symlinks rather than allowing writes through them.
            if let Ok(meta) = fs::symlink_metadata(&joined) {
                if meta.file_type().is_symlink() {
                    let canonical = fs::canonicalize(&joined).map_err(|_| {
                        PathGuardError::SymlinkRejected(candidate.as_ref().to_path_buf())
                    })?;
                    if !canonical.starts_with(&self.root) {
                        return Err(PathGuardError::OutsideWorkspace(
                            candidate.as_ref().to_path_buf(),
                        ));
                    }
                    return Ok(canonical);
                }
            }
            return self.resolve_existing(&joined);
        }

        // Normalize lexical `..` before splitting into existing ancestor + tail, so valid paths
        // like `sub/../file.txt` are checked cleanly while escaping `../outside` is rejected.
        let normalized = lexical_normalize(&joined)?;

        let mut existing = normalized.as_path();
        let mut tail_parts: Vec<std::ffi::OsString> = Vec::new();

        while !existing.exists() {
            // If an intermediate component is a dangling symlink, reject it.
            if let Ok(meta) = fs::symlink_metadata(existing) {
                if meta.file_type().is_symlink() {
                    return Err(PathGuardError::SymlinkRejected(existing.to_path_buf()));
                }
            }
            let name = existing
                .file_name()
                .ok_or_else(|| PathGuardError::InvalidPath(candidate.as_ref().to_path_buf()))?;
            tail_parts.push(name.to_os_string());
            existing = existing.parent().ok_or_else(|| {
                PathGuardError::OutsideWorkspace(candidate.as_ref().to_path_buf())
            })?;
        }

        let canonical_ancestor = fs::canonicalize(existing)?;
        if !canonical_ancestor.starts_with(&self.root) {
            return Err(PathGuardError::OutsideWorkspace(
                candidate.as_ref().to_path_buf(),
            ));
        }

        let mut result = canonical_ancestor;
        for part in tail_parts.into_iter().rev() {
            result.push(part);
        }
        Ok(result)
    }

    /// Open an existing regular file for reading with TOCTOU protection.
    ///
    /// On Unix, opens with `O_NOFOLLOW` so a symlink swapped in between validation and `open()`
    /// cannot redirect the file descriptor outside the workspace.
    pub fn open_existing_read(&self, candidate: impl AsRef<Path>) -> Result<File, PathGuardError> {
        let joined = self.join_candidate(candidate.as_ref());

        // Reject if any component from root to target is a symlink on Unix (strict TOCTOU guard).
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            let canonical = self.resolve_existing(&joined)?;
            let mut opts = fs::OpenOptions::new();
            opts.read(true).custom_flags(libc::O_NOFOLLOW);

            // Re-check that the path itself is not a symlink right at open time.
            let file = opts.open(&canonical).map_err(|err| {
                if err.raw_os_error() == Some(libc::ELOOP) {
                    PathGuardError::SymlinkRejected(canonical.clone())
                } else {
                    PathGuardError::Io(err)
                }
            })?;

            // Re-verify canonical path still resides inside workspace after open.
            let post_canonical = fs::canonicalize(&canonical)?;
            if !post_canonical.starts_with(&self.root) {
                return Err(PathGuardError::OutsideWorkspace(post_canonical));
            }
            Ok(file)
        }

        #[cfg(not(unix))]
        {
            let canonical = self.resolve_existing(&joined)?;
            Ok(File::open(canonical)?)
        }
    }

    fn join_candidate(&self, candidate: &Path) -> PathBuf {
        if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        }
    }
}

fn lexical_normalize(path: &Path) -> Result<PathBuf, PathGuardError> {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(PathGuardError::OutsideWorkspace(path.to_path_buf()));
                }
            }
            Component::Normal(seg) => out.push(seg),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_existing_file_inside_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("hello.txt");
        fs::write(&file, b"hi").unwrap();

        let guard = PathGuard::new(tmp.path()).unwrap();
        let resolved = guard.resolve_existing("hello.txt").unwrap();
        assert_eq!(resolved, fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn allows_new_nested_file_inside_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path()).unwrap();

        let target = guard.resolve_for_write("a/b/c.txt").unwrap();
        assert!(target.starts_with(guard.root()));
        assert!(target.ends_with(Path::new("a/b/c.txt")));
    }

    #[test]
    fn rejects_dotdot_escape_existing_and_write() {
        let parent = tempfile::tempdir().unwrap();
        let ws = parent.path().join("workspace");
        let outside = parent.path().join("secret.txt");
        fs::create_dir(&ws).unwrap();
        fs::write(&outside, b"secret").unwrap();

        let guard = PathGuard::new(&ws).unwrap();
        assert!(matches!(
            guard.resolve_existing("../secret.txt"),
            Err(PathGuardError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            guard.resolve_for_write("../new_secret.txt"),
            Err(PathGuardError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            guard.resolve_for_write("sub/../../escape.txt"),
            Err(PathGuardError::OutsideWorkspace(_))
        ));
    }

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let parent = tempfile::tempdir().unwrap();
        let ws = parent.path().join("workspace");
        let outside = parent.path().join("outside.txt");
        fs::create_dir(&ws).unwrap();
        fs::write(&outside, b"nope").unwrap();

        let guard = PathGuard::new(&ws).unwrap();
        assert!(matches!(
            guard.resolve_existing(&outside),
            Err(PathGuardError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            guard.resolve_for_write(parent.path().join("nope2.txt")),
            Err(PathGuardError::OutsideWorkspace(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_to_outside_file_and_dir() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let ws = parent.path().join("workspace");
        let outside_dir = parent.path().join("outside_dir");
        let outside_file = outside_dir.join("secret.txt");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(&outside_file, b"top-secret").unwrap();

        // Symlink directly to outside file.
        let link_file = ws.join("leak.txt");
        symlink(&outside_file, &link_file).unwrap();

        // Symlink to outside directory.
        let link_dir = ws.join("leak_dir");
        symlink(&outside_dir, &link_dir).unwrap();

        // Dangling symlink pointing outside.
        let dangling = ws.join("dangling.txt");
        symlink(outside_dir.join("future.txt"), &dangling).unwrap();

        let guard = PathGuard::new(&ws).unwrap();

        assert!(matches!(
            guard.resolve_existing("leak.txt"),
            Err(PathGuardError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            guard.resolve_for_write("leak.txt"),
            Err(PathGuardError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            guard.resolve_for_write("leak_dir/new_file.txt"),
            Err(PathGuardError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            guard.resolve_for_write("dangling.txt"),
            Err(PathGuardError::SymlinkRejected(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn open_existing_read_uses_nofollow_to_guard_toctou() {
        use std::io::Read;
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let ws = parent.path().join("workspace");
        fs::create_dir_all(&ws).unwrap();

        let legit = ws.join("data.txt");
        fs::write(&legit, b"safe-bytes").unwrap();

        let guard = PathGuard::new(&ws).unwrap();
        let mut f = guard.open_existing_read("data.txt").unwrap();
        let mut buf = String::new();
        f.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "safe-bytes");

        // Swap the file for a symlink pointing outside the workspace.
        let outside = parent.path().join("outside.txt");
        fs::write(&outside, b"secret").unwrap();
        fs::remove_file(&legit).unwrap();
        symlink(&outside, &legit).unwrap();

        assert!(guard.open_existing_read("data.txt").is_err());
    }
}
