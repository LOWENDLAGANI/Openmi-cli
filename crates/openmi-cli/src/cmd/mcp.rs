use std::env;
use std::path::PathBuf;

use openmi_core::{load_merged_mcp_config_with_trust, McpManager, ToolRegistry};

use crate::exit_codes::CliError;

pub async fn list(cwd: Option<PathBuf>, json: bool) -> Result<(), CliError> {
    let workdir = match cwd {
        Some(dir) => dir,
        None => env::current_dir().map_err(anyhow::Error::from)?,
    };

    let trusted = crate::trust::ensure_workspace_trust(&workdir, !json);
    let cfg =
        load_merged_mcp_config_with_trust(None, &workdir, trusted).map_err(anyhow::Error::from)?;
    if cfg.mcp_servers.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No MCP servers configured.");
            println!("Add servers to ~/.config/openmi/mcp.json or <workspace>/.openmi/mcp.json.");
        }
        return Ok(());
    }

    let mut manager = McpManager::from_config(&workdir, cfg);
    let mut registry = ToolRegistry::new();
    manager.start_all(&mut registry).await;
    let statuses = manager.statuses();
    manager.shutdown_all().await;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&statuses).map_err(anyhow::Error::from)?
        );
    } else {
        for s in statuses {
            let state = if s.running { "ok" } else { "error" };
            println!("{} [{}] ({})", s.name, state, s.command);
            if let Some(err) = s.error {
                println!("  error: {err}");
            } else if s.tools.is_empty() {
                println!("  (no tools)");
            } else {
                for tool in s.tools {
                    println!("  - {tool}");
                }
            }
        }
    }

    Ok(())
}
