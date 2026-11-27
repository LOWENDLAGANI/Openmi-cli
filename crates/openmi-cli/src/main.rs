mod auth;
mod browser;
mod cmd;
mod exit_codes;
mod output;
mod session;
mod session_store;
mod settings;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::cmd::run::RunOptions;
use crate::cmd::tui::TuiOptions;
use crate::exit_codes::{CliError, EXIT_OK, EXIT_USAGE};

const AFTER_HELP: &str = "\
PERMISSION MODES:
  read-only    Allow read_file and list_dir only; block all mutating tools.
  ask          Prompt for approval before writes/commands (denies without TTY unless --yes).
  trusted      Auto-approve workspace tools and commands within PathGuard boundaries (default).
  full-access  Unrestricted tool execution (requires typed confirmation in TUI).

EXIT CODES:
  0    Success
  1    General error
  2    CLI usage / invalid arguments
  3    Authentication required, expired, or revoked
  4    Out of credits (HTTP 402)
  5    Agent loop-cap exceeded
  130  Cancelled (Ctrl-C / SIGINT)

FILES:
  ~/.config/openmi/credentials.json   Auth tokens (0600 permissions on Unix/Termux)
  ~/.config/openmi/settings.json      Saved TUI preferences (/settings)
  ~/.config/openmi/mcp.json           Global stdio MCP server configuration
  .openmi/mcp.json                    Project-level stdio MCP server overrides
  ~/.local/share/openmi/sessions/     Saved interactive sessions (--resume)
  ~/.cache/openmi/models.json         Cached model catalog
  OPENMI.md                           Project instructions in workspace root (or CLAUDE.md / AGENTS.md)

EXAMPLES:
  openmi login
  openmi
  openmi --resume
  openmi mcp list
  openmi run \"explain src/main.rs\" --permission read-only
  openmi run \"add unit tests\" --json --yes
  openmi update --check";

#[derive(Debug, Parser)]
#[command(
    name = "openmi",
    version,
    about = "Openmi CLI — interactive TUI and headless coding agent",
    long_about = "Openmi CLI is a cross-platform coding agent for Windows, Linux, macOS, and Termux (Android aarch64).\nRun `openmi` with no subcommand to launch the interactive ratatui TUI, or `openmi run` for headless scripting.",
    after_help = AFTER_HELP
)]
struct Cli {
    /// Model identifier override (when launching the default TUI).
    #[arg(long, default_value = "openmi-default")]
    model: String,

    /// Permission mode override (default comes from `/settings`, initially `trusted`): read-only, ask, trusted, full-access.
    #[arg(long)]
    permission: Option<String>,

    /// Working directory for the workspace root (when launching the default TUI).
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Override the managed Openmi API base URL.
    #[arg(long)]
    api_url: Option<String>,

    /// Resume the latest saved session (or pass a specific session ID).
    #[arg(long, num_args = 0..=1, value_name = "SESSION_ID")]
    resume: Option<Option<String>>,

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

        /// Permission mode override: read-only, ask, trusted, full-access.
        #[arg(long)]
        permission: Option<String>,

        /// Working directory for the workspace root.
        #[arg(long)]
        cwd: Option<PathBuf>,

        /// Override the managed Openmi API base URL.
        #[arg(long)]
        api_url: Option<String>,

        /// Resume the latest saved session (or pass a specific session ID).
        #[arg(long, num_args = 0..=1, value_name = "SESSION_ID")]
        resume: Option<Option<String>>,
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

        /// Print the sign-in URL instead of automatically opening a browser.
        #[arg(long)]
        no_browser: bool,
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

    /// Open the Openmi Paid Pass upgrade page (/pay) in your browser.
    Upgrade {
        /// Override the managed Openmi API base URL.
        #[arg(long)]
        api_url: Option<String>,

        /// Print the upgrade URL instead of automatically opening a browser.
        #[arg(long)]
        no_browser: bool,
    },

    /// Manage and inspect configured stdio MCP servers.
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },

    /// Check for updates and replace the `openmi` binary from GitHub Releases.
    Update {
        /// Check for a newer version without modifying the installed binary.
        #[arg(long)]
        check: bool,

        /// Override the GitHub Releases JSON endpoint (useful for testing/mirrors).
        #[arg(long)]
        release_url: Option<String>,
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

        /// Permission mode override: read-only, ask, trusted, full-access.
        #[arg(long)]
        permission: Option<String>,

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

#[derive(Debug, Subcommand)]
enum McpCommands {
    /// List configured MCP servers and their discovered `mcp__<server>__<tool>` tools.
    List {
        /// Working directory for project `.openmi/mcp.json` discovery.
        #[arg(long)]
        cwd: Option<PathBuf>,

        /// Emit raw JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
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

    if !matches!(cli.command, Some(Commands::Update { .. })) {
        if let Some(exit_code) = cmd::update::auto_update_before_boot().await {
            return exit_code;
        }
    }

    let result: Result<(), CliError> = match cli.command {
        None => {
            cmd::tui::execute(TuiOptions {
                model: cli.model,
                permission: cli.permission,
                cwd: cli.cwd,
                api_url: cli.api_url,
                resume: cli.resume,
            })
            .await
        }
        Some(Commands::Tui {
            model,
            permission,
            cwd,
            api_url,
            resume,
        }) => {
            cmd::tui::execute(TuiOptions {
                model,
                permission,
                cwd,
                api_url,
                resume,
            })
            .await
        }
        Some(Commands::Login {
            token,
            refresh_token,
            api_url,
            no_browser,
        }) => cmd::login::execute(token, refresh_token, api_url, no_browser)
            .await
            .map_err(map_anyhow_auth),
        Some(Commands::Logout) => cmd::logout::execute().await.map_err(CliError::Other),
        Some(Commands::Whoami { api_url }) => {
            cmd::whoami::execute(api_url).await.map_err(map_anyhow_auth)
        }
        Some(Commands::Models { json, api_url }) => cmd::models::execute(json, api_url).await,
        Some(Commands::Credits { json, api_url }) => cmd::credits::execute(json, api_url).await,
        Some(Commands::Upgrade {
            api_url,
            no_browser,
        }) => cmd::upgrade::execute(api_url, no_browser).await,
        Some(Commands::Mcp { command }) => match command {
            McpCommands::List { cwd, json } => cmd::mcp::list(cwd, json).await,
        },
        Some(Commands::Update { check, release_url }) => {
            cmd::update::execute(check, release_url).await
        }
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
