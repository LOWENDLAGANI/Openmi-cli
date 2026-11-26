use std::sync::{Arc, Mutex};

use anyhow::{ensure, Result};
use openmi_checkpoints::CheckpointStore;
use openmi_core::{AgentEvent, PermissionMode};
use openmi_platform::{atomic_write, PathGuard};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::session::build_session;

/// Verifies core, checkpoint, and platform primitives end-to-end without network access.
pub async fn execute() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("openmi-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;

    let res = run_smoke_in(&dir).await;
    let _ = std::fs::remove_dir_all(&dir);
    res?;

    println!("openmi smoke-test: OK");
    Ok(())
}

async fn run_smoke_in(dir: &std::path::Path) -> Result<()> {
    // 1. Verify PathGuard blocks traversal outside workspace.
    let guard = PathGuard::new(dir)?;
    ensure!(
        guard.resolve_for_write("../escape.txt").is_err(),
        "PathGuard failed to block .. traversal"
    );

    // 2. Verify atomic write + checkpoint rollback preserve exact bytes.
    let sample = dir.join("sample.txt");
    let original = b"\xEF\xBB\xBFsmoke\r\n";
    atomic_write(&sample, original)?;

    let checkpoints = Arc::new(Mutex::new(CheckpointStore::new()));
    checkpoints.lock().expect("mutex").begin_run();

    let mut session = build_session(
        dir,
        "openmi-default",
        PermissionMode::Ask,
        Arc::clone(&checkpoints),
    )?;
    let target = session.prepare_write("sample.txt")?;
    atomic_write(&target, b"modified")?;
    checkpoints.lock().expect("mutex").undo_file(&target)?;
    ensure!(
        std::fs::read(&sample)? == original,
        "Checkpoint rollback did not restore exact bytes"
    );

    // 3. Verify AgentSession turn loop emits TextDelta + TurnCompleted.
    let (tx, mut rx) = mpsc::channel(16);
    let cancel = CancellationToken::new();
    session.run_turn("ping", tx, cancel).await?;

    let mut saw_delta = false;
    let mut saw_done = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            AgentEvent::TextDelta { .. } => saw_delta = true,
            AgentEvent::TurnCompleted { .. } => saw_done = true,
            _ => {}
        }
    }
    ensure!(
        saw_delta && saw_done,
        "AgentSession did not emit expected events"
    );
    Ok(())
}
