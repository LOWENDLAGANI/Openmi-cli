use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("checkpoint not found: {0}")]
    NotFound(String),

    #[error("file not tracked in checkpoint: {0}")]
    FileNotTracked(PathBuf),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Snapshot of file states captured before a run modifies them.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub id: String,
    /// Map from absolute file path to its original raw bytes (`None` if the file did not exist).
    pub originals: HashMap<PathBuf, Option<Vec<u8>>>,
}

/// In-memory checkpoint history supporting exact byte-level rollback per file or per run.
#[derive(Debug, Default)]
pub struct CheckpointStore {
    checkpoints: Vec<Checkpoint>,
}

impl CheckpointStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new checkpoint for an agent run and return its ID.
    pub fn begin_run(&mut self) -> String {
        let id = Uuid::new_v4().to_string();
        self.checkpoints.push(Checkpoint {
            id: id.clone(),
            originals: HashMap::new(),
        });
        id
    }

    /// Record the pre-write contents of `path` in the latest checkpoint (if not already recorded).
    pub fn snapshot_before_write(&mut self, path: &Path) -> Result<(), CheckpointError> {
        let cp = match self.checkpoints.last_mut() {
            Some(cp) => cp,
            None => {
                self.begin_run();
                self.checkpoints.last_mut().expect("just created")
            }
        };

        let key = path.to_path_buf();
        if cp.originals.contains_key(&key) {
            return Ok(());
        }

        let original = if path.exists() {
            Some(fs::read(path)?)
        } else {
            None
        };
        cp.originals.insert(key, original);
        Ok(())
    }

    /// Undo a single file from the most recent checkpoint that touched it.
    pub fn undo_file(&self, path: &Path) -> Result<(), CheckpointError> {
        for cp in self.checkpoints.iter().rev() {
            if let Some(original) = cp.originals.get(path) {
                return restore_entry(path, original.as_deref());
            }
        }
        Err(CheckpointError::FileNotTracked(path.to_path_buf()))
    }

    /// Undo all files touched in a specific checkpoint run (or the latest run if `id` is `None`).
    pub fn undo_run(&self, id: Option<&str>) -> Result<usize, CheckpointError> {
        let cp = match id {
            Some(target_id) => self
                .checkpoints
                .iter()
                .find(|c| c.id == target_id)
                .ok_or_else(|| CheckpointError::NotFound(target_id.to_string()))?,
            None => self
                .checkpoints
                .last()
                .ok_or_else(|| CheckpointError::NotFound("latest".to_string()))?,
        };

        let mut count = 0;
        for (path, original) in &cp.originals {
            restore_entry(path, original.as_deref())?;
            count += 1;
        }
        Ok(count)
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }
}

fn restore_entry(path: &Path, original: Option<&[u8]>) -> Result<(), CheckpointError> {
    match original {
        Some(bytes) => {
            openmi_platform::atomic_write(path, bytes)?;
        }
        None => {
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_restores_exact_bom_and_crlf_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("doc.txt");
        let original = b"\xEF\xBB\xBFhello\r\nworld\r\n";
        fs::write(&file, original).unwrap();

        let mut store = CheckpointStore::new();
        let run_id = store.begin_run();
        store.snapshot_before_write(&file).unwrap();

        // Simulate agent editing the file twice during the same run.
        fs::write(&file, b"changed once").unwrap();
        store.snapshot_before_write(&file).unwrap();
        fs::write(&file, b"changed twice").unwrap();

        let restored = store.undo_run(Some(&run_id)).unwrap();
        assert_eq!(restored, 1);
        assert_eq!(fs::read(&file).unwrap(), original);
    }

    #[test]
    fn undo_removes_newly_created_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("brand_new.txt");

        let mut store = CheckpointStore::new();
        store.begin_run();
        store.snapshot_before_write(&file).unwrap();
        fs::write(&file, b"created").unwrap();
        assert!(file.exists());

        store.undo_file(&file).unwrap();
        assert!(!file.exists());
    }
}
