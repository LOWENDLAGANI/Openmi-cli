use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use openmi_checkpoints::CheckpointStore;
use openmi_core::{AgentEvent, PermissionMode};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::session::build_session;

pub async fn execute(
    prompt: String,
    model: String,
    permission: String,
    cwd: Option<PathBuf>,
) -> Result<()> {
    let mode: PermissionMode = permission.parse().map_err(|e: String| anyhow!(e))?;
    let workdir = match cwd {
        Some(dir) => dir,
        None => env::current_dir()?,
    };

    let checkpoints = Arc::new(Mutex::new(CheckpointStore::new()));
    {
        let mut guard = checkpoints.lock().expect("checkpoint mutex");
        guard.begin_run();
    }

    let mut session = build_session(&workdir, &model, mode, checkpoints)?;
    let (tx, mut rx) = mpsc::channel(32);
    let cancel = CancellationToken::new();

    let reader = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let AgentEvent::TextDelta { text } = ev {
                print!("{text}");
            }
        }
        println!();
    });

    session.run_turn(&prompt, tx, cancel).await?;
    reader.await?;
    Ok(())
}
