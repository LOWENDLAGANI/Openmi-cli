use std::env;
use std::path::{Path, PathBuf};

/// Cross-platform application directory layout for `openmi`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppPaths {
    /// Resolve application paths from the current environment.
    pub fn resolve() -> std::io::Result<Self> {
        Self::resolve_with(|k| env::var_os(k).map(PathBuf::from))
    }

    /// Resolve application paths using a custom environment lookup (testable without mutating env).
    pub fn resolve_with<F>(get_env: F) -> std::io::Result<Self>
    where
        F: Fn(&str) -> Option<PathBuf>,
    {
        #[cfg(windows)]
        {
            let app_data = get_env("APPDATA")
                .or_else(|| get_env("USERPROFILE").map(|h| h.join("AppData").join("Roaming")))
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "APPDATA/USERPROFILE not set")
                })?;
            let local_app_data = get_env("LOCALAPPDATA")
                .or_else(|| get_env("USERPROFILE").map(|h| h.join("AppData").join("Local")))
                .unwrap_or_else(|| app_data.clone());

            Ok(Self {
                config_dir: app_data.join("openmi"),
                data_dir: local_app_data.join("openmi").join("data"),
                cache_dir: local_app_data.join("openmi").join("cache"),
            })
        }

        #[cfg(not(windows))]
        {
            let home = get_env("HOME").ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "HOME environment variable not set",
                )
            })?;

            let config_base = get_env("XDG_CONFIG_HOME")
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| home.join(".config"));
            let data_base = get_env("XDG_DATA_HOME")
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| home.join(".local").join("share"));
            let cache_base = get_env("XDG_CACHE_HOME")
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| home.join(".cache"));

            Ok(Self {
                config_dir: config_base.join("openmi"),
                data_dir: data_base.join("openmi"),
                cache_dir: cache_base.join("openmi"),
            })
        }
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.data_dir.join("sessions")
    }

    pub fn credentials_file(&self) -> PathBuf {
        self.config_dir.join("credentials.json")
    }

    pub fn models_cache_file(&self) -> PathBuf {
        self.cache_dir.join("models.json")
    }

    pub fn mcp_config_file(&self) -> PathBuf {
        self.config_dir.join("mcp.json")
    }

    /// Resolve the per-project external memory file:
    /// `~/.config/openmi/projects/<workspace-hash>/memory.md`.
    pub fn project_memory_file(&self, workspace_root: &Path) -> PathBuf {
        use sha2::{Digest, Sha256};

        let canonical =
            std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
        let normalized = canonical.to_string_lossy().replace('\\', "/");
        let digest = Sha256::digest(normalized.as_bytes());
        let mut hex = String::with_capacity(16);
        for byte in &digest[..8] {
            use std::fmt::Write;
            let _ = write!(&mut hex, "{byte:02x}");
        }
        self.config_dir.join("projects").join(hex).join("memory.md")
    }

    /// Ensure config, data, and cache directories exist on disk.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [&self.config_dir, &self.data_dir, &self.cache_dir] {
            ensure_dir_exists(dir)?;
        }
        Ok(())
    }
}

fn ensure_dir_exists(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn resolves_default_paths_from_home() {
        let mut vars = HashMap::new();
        #[cfg(windows)]
        {
            vars.insert("APPDATA", PathBuf::from(r"C:\Users\alice\AppData\Roaming"));
            vars.insert(
                "LOCALAPPDATA",
                PathBuf::from(r"C:\Users\alice\AppData\Local"),
            );
        }
        #[cfg(not(windows))]
        {
            vars.insert("HOME", PathBuf::from("/home/alice"));
        }

        let paths = AppPaths::resolve_with(|k| vars.get(k).cloned()).unwrap();
        assert!(paths.config_dir.ends_with("openmi"));
        assert!(paths.sessions_dir().ends_with("sessions"));
        assert!(paths.credentials_file().ends_with("credentials.json"));
        assert!(paths.models_cache_file().ends_with("models.json"));
        assert!(paths.mcp_config_file().ends_with("mcp.json"));
    }

    #[cfg(not(windows))]
    #[test]
    fn respects_xdg_overrides_on_unix_and_termux() {
        let mut vars = HashMap::new();
        vars.insert("HOME", PathBuf::from("/data/data/com.termux/files/home"));
        vars.insert("XDG_CONFIG_HOME", PathBuf::from("/custom/config"));
        vars.insert("XDG_DATA_HOME", PathBuf::from("/custom/data"));
        vars.insert("XDG_CACHE_HOME", PathBuf::from("/custom/cache"));

        let paths = AppPaths::resolve_with(|k| vars.get(k).cloned()).unwrap();
        assert_eq!(paths.config_dir, PathBuf::from("/custom/config/openmi"));
        assert_eq!(paths.data_dir, PathBuf::from("/custom/data/openmi"));
        assert_eq!(paths.cache_dir, PathBuf::from("/custom/cache/openmi"));
    }

    #[cfg(not(windows))]
    #[test]
    fn empty_xdg_falls_back_to_home() {
        let mut vars = HashMap::new();
        vars.insert("HOME", PathBuf::from("/home/bob"));
        vars.insert("XDG_CONFIG_HOME", PathBuf::from(""));

        let paths = AppPaths::resolve_with(|k| vars.get(k).cloned()).unwrap();
        assert_eq!(paths.config_dir, PathBuf::from("/home/bob/.config/openmi"));
    }

    #[test]
    fn ensure_dirs_creates_all_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_dir: tmp.path().join("cfg/openmi"),
            data_dir: tmp.path().join("data/openmi"),
            cache_dir: tmp.path().join("cache/openmi"),
        };
        paths.ensure_dirs().unwrap();
        assert!(paths.config_dir.is_dir());
        assert!(paths.data_dir.is_dir());
        assert!(paths.cache_dir.is_dir());
    }

    #[test]
    fn project_memory_file_is_deterministic_per_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_a = tmp.path().join("repo-a");
        let ws_b = tmp.path().join("repo-b");
        std::fs::create_dir_all(&ws_a).unwrap();
        std::fs::create_dir_all(&ws_b).unwrap();

        let paths = AppPaths {
            config_dir: tmp.path().join("cfg/openmi"),
            data_dir: tmp.path().join("data/openmi"),
            cache_dir: tmp.path().join("cache/openmi"),
        };
        let mem_a1 = paths.project_memory_file(&ws_a);
        let mem_a2 = paths.project_memory_file(&ws_a);
        let mem_b = paths.project_memory_file(&ws_b);

        assert_eq!(mem_a1, mem_a2);
        assert_ne!(mem_a1, mem_b);
        assert!(mem_a1.ends_with("memory.md"));
        assert!(mem_a1.starts_with(paths.config_dir.join("projects")));
    }
}
