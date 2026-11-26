mod cmd;
mod session;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "openmi", version, about = "Openmi CLI coding agent")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run a built-in portability and core-wiring smoke test.
    SmokeTest,

    /// Execute a single non-interactive prompt turn.
    Run {
        /// Prompt to send to the agent.
        prompt: String,

        /// Model identifier override.
        #[arg(long, default_value = "openmi-default")]
        model: String,

        /// Permission mode: read-only, ask, trusted, full-access.
        #[arg(long, default_value = "ask")]
        permission: String,

        /// Working directory for the workspace root.
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::SmokeTest => cmd::smoke::execute().await,
        Commands::Run {
            prompt,
            model,
            permission,
            cwd,
        } => cmd::run::execute(prompt, model, permission, cwd).await,
    }
}
