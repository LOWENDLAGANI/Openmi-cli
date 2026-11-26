mod auth;
mod cmd;
mod exit_codes;
mod output;
mod session;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::cmd::run::RunOptions;
use crate::cmd::tui::TuiOptions;
use crate::exit_codes::{CliError, EXIT_OK, EXIT_USAGE};

#[derive(Debug, Parser)]
#[command(name = "openmi", version, about = "Openmi CLI coding agent")]
struct Cli {
    /// Model identifier override (when launching the default TUI).
    #[arg(long, default_value = "openmi-default")]
    model: String,

    /// Permission mode (when launching the default TUI): read-only, ask, trusted, full-access.
    #[arg(long, default_value = "ask")]
    permission: String,

    /// Working directory for the workspace root (when launching the default TUI).
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Override the managed Openmi API base URL.
    #[arg(long)]
    api_url: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Launch the interactive ratatui TUI (also the default when no subcommand is given).
    Tui {
        /// Model identifier override.
        #[arg(long, default_value = "openmi-default")]
        model: String,

        /// Permission mode: read-only, ask, trusted, full-access.
        #[arg(long, default_value = "ask")]
        permission: String,

        /// Working directory for the workspace root.
        #[arg(long)]
        cwd: Option<PathBuf>,

        /// Override the managed Openmi API base URL.
        #[arg(long)]
        api_url: Option<String>,
    },

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

    /// List available Openmi models (with local cache and offline fallback).
    Models {
        /// Emit raw JSON instead of a formatted table.
        #[arg(long)]
        json: bool,

        /// Override the managed Openmi API base URL.
        #[arg(long)]
        api_url: Option<String>,
    },

    /// Show current Openmi credit balance and plan.
    Credits {
        /// Emit raw JSON instead of human-readable text.
        #[arg(long)]
        json: bool,

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

        /// Stream versioned JSONL events (one AgentEvent per line).
        #[arg(long)]
        json: bool,

        /// Auto-approve tool prompts in Ask mode (useful in non-interactive pipelines).
        #[arg(long, short = 'y')]
        yes: bool,

        /// Override the managed Openmi API base URL.
        #[arg(long)]
        api_url: Option<String>,

        /// Maximum tool-calling steps before returning loop-cap exit code 5.
        #[arg(long)]
        max_steps: Option<u32>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(err) => {
            let is_help_or_version = matches!(
                err.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            let _ = err.print();
            return if is_help_or_version {
                ExitCode::from(EXIT_OK)
            } else {
                ExitCode::from(EXIT_USAGE)
            };
        }
    };

    let result: Result<(), CliError> = match cli.command {
        None => {
            cmd::tui::execute(TuiOptions {
                model: cli.model,
                permission: cli.permission,
                cwd: cli.cwd,
                api_url: cli.api_url,
            })
            .await
        }
        Some(Commands::Tui {
            model,
            permission,
            cwd,
            api_url,
        }) => {
            cmd::tui::execute(TuiOptions {
                model,
                permission,
                cwd,
                api_url,
            })
            .await
        }
        Some(Commands::Login {
            token,
            refresh_token,
            api_url,
        }) => cmd::login::execute(token, refresh_token, api_url)
            .await
            .map_err(map_anyhow_auth),
        Some(Commands::Logout) => cmd::logout::execute().await.map_err(CliError::Other),
        Some(Commands::Whoami { api_url }) => {
            cmd::whoami::execute(api_url).await.map_err(map_anyhow_auth)
        }
        Some(Commands::Models { json, api_url }) => cmd::models::execute(json, api_url).await,
        Some(Commands::Credits { json, api_url }) => cmd::credits::execute(json, api_url).await,
        Some(Commands::SmokeTest) => cmd::smoke::execute().await.map_err(CliError::Other),
        Some(Commands::Run {
            prompt,
            model,
            permission,
            cwd,
            json,
            yes,
            api_url,
            max_steps,
        }) => {
            cmd::run::execute(RunOptions {
                prompt,
                model,
                permission,
                cwd,
                json,
                yes,
                api_url,
                max_steps,
            })
            .await
        }
    };

    match result {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(err) => {
            eprintln!("error: {err}");
            err.exit_code()
        }
    }
}

fn map_anyhow_auth(err: anyhow::Error) -> CliError {
    match err.downcast::<auth::AuthError>() {
        Ok(auth_err) => CliError::Auth(auth_err),
        Err(other) => CliError::Other(other),
    }
}
