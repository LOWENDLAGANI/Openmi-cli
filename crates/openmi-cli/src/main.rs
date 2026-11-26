mod auth;
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
    /// Authenticate with Openmi using device-code pairing (or --token fallback).
    Login {
        /// Paste a Supabase JWT directly instead of running device-code pairing.
        #[arg(long)]
        token: Option<String>,

        /// Optional refresh token when using --token.
        #[arg(long)]
        refresh_token: Option<String>,

        /// Override the managed Openmi API base URL.
        #[arg(long)]
        api_url: Option<String>,
    },

    /// Clear locally stored authentication credentials.
    Logout,

    /// Display the currently authenticated Openmi user.
    Whoami {
        /// Override the managed Openmi API base URL.
        #[arg(long)]
        api_url: Option<String>,
    },

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
        Commands::Login {
            token,
            refresh_token,
            api_url,
        } => cmd::login::execute(token, refresh_token, api_url).await,
        Commands::Logout => cmd::logout::execute().await,
        Commands::Whoami { api_url } => cmd::whoami::execute(api_url).await,
        Commands::SmokeTest => cmd::smoke::execute().await,
        Commands::Run {
            prompt,
            model,
            permission,
            cwd,
        } => cmd::run::execute(prompt, model, permission, cwd).await,
    }
}
