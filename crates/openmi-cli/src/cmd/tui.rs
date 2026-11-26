use std::env;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use openmi_checkpoints::CheckpointStore;
use openmi_core::{AgentEvent, ApprovalDecision, PermissionMode, ProxyConfig};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, TokenStore};
use crate::exit_codes::CliError;
use crate::session::build_session_with_options;
use crate::tui::app::TuiApp;
use crate::tui::approval::{PendingApproval, TuiApprovalHandler};
use crate::tui::render;

#[derive(Debug, Clone)]
pub struct TuiOptions {
    pub model: String,
    pub permission: String,
    pub cwd: Option<PathBuf>,
    pub api_url: Option<String>,
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
}

pub async fn execute(opts: TuiOptions) -> Result<(), CliError> {
    let mode: PermissionMode = opts.permission.parse().map_err(CliError::Usage)?;
    let workdir = match opts.cwd {
        Some(dir) => dir,
        None => env::current_dir().map_err(anyhow::Error::from)?,
    };

    let store = default_token_store()?;
    let stored = store.load()?;
    let proxy_config = if let Some(ref tok) = stored {
        let base_url = resolve_api_base_url(opts.api_url.as_deref(), Some(tok));
        let client = DeviceAuthClient::new(&base_url)?;
        let valid = client.ensure_valid_tokens(&store).await?;
        Some(ProxyConfig {
            endpoint: format!("{base_url}/functions/v1/chat"),
            bearer_token: valid.access_token,
        })
    } else {
        opts.api_url.as_deref().map(|explicit| ProxyConfig {
            endpoint: format!("{}/functions/v1/chat", explicit.trim_end_matches('/')),
            bearer_token: String::new(),
        })
    };

    let checkpoints = Arc::new(Mutex::new(CheckpointStore::new()));
    let (approval_tx, mut approval_rx) = mpsc::channel::<PendingApproval>(8);
    let approval_handler = Arc::new(TuiApprovalHandler::new(approval_tx));

    let session = Arc::new(tokio::sync::Mutex::new(build_session_with_options(
        &workdir,
        &opts.model,
        mode,
        Arc::clone(&checkpoints),
        approval_handler,
        proxy_config,
    )?));

    install_panic_hook();
    enable_raw_mode().map_err(anyhow::Error::from)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(anyhow::Error::from)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(anyhow::Error::from)?;
    terminal.clear().map_err(anyhow::Error::from)?;

    let mut app = TuiApp::new(opts.model, mode, workdir);
    let (agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(128);
    let mut active_cancel: Option<CancellationToken> = None;

    let mut events = EventStream::new();
    let mut ticker = interval(Duration::from_millis(120));
    let mut dirty = true;

    while !app.should_quit {
        if dirty {
            terminal
                .draw(|f| render::draw(f, &app))
                .map_err(anyhow::Error::from)?;
            dirty = false;
        }

        tokio::select! {
            maybe_ev = events.next() => {
                let Some(Ok(ev)) = maybe_ev else {
                    break;
                };
                match ev {
                    Event::Resize(_, _) => {
                        dirty = true;
                    }
                    Event::Key(key) => {
                        dirty = true;
                        if let Some(prompt) = handle_key_event(&mut app, key, &mut active_cancel) {
                            checkpoints.lock().expect("checkpoint mutex").begin_run();
                            app.push_user(prompt.clone());
                            let cancel = CancellationToken::new();
                            active_cancel = Some(cancel.clone());
                            let sess = Arc::clone(&session);
                            let tx = agent_tx.clone();
                            tokio::spawn(async move {
                                let mut s = sess.lock().await;
                                if let Err(err) = s.run_turn(&prompt, tx.clone(), cancel).await {
                                    let _ = tx.send(AgentEvent::Error {
                                        code: "turn_error".to_string(),
                                        message: err.to_string(),
                                    }).await;
                                }
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some(agent_ev) = agent_rx.recv() => {
                if matches!(agent_ev, AgentEvent::ControlStateChanged { state: openmi_core::ControlState::Idle }) {
                    active_cancel = None;
                }
                app.apply_agent_event(agent_ev);
                dirty = true;
            }
            Some(pending) = approval_rx.recv() => {
                app.pending_approval = Some(pending);
                dirty = true;
            }
            _ = ticker.tick() => {
                if app.is_busy() {
                    app.tick = app.tick.wrapping_add(1);
                    dirty = true;
                }
            }
        }
    }

    if let Some(cancel) = active_cancel.take() {
        cancel.cancel();
    }
    Ok(())
}

fn handle_key_event(
    app: &mut TuiApp,
    key: KeyEvent,
    active_cancel: &mut Option<CancellationToken>,
) -> Option<String> {
    // Ctrl-C or Esc: cancel first, quit second.
    if key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        let should_cancel = app.handle_interrupt();
        if should_cancel {
            if let Some(tok) = active_cancel.as_ref() {
                tok.cancel();
            }
        }
        return None;
    }

    // Any other key clears armed quit.
    app.armed_quit = false;

    // Modal approval popup intercepts y / n / a.
    if app.pending_approval.is_some() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.resolve_approval(ApprovalDecision::Approve);
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                app.resolve_approval(ApprovalDecision::ApproveAlways);
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.resolve_approval(ApprovalDecision::Deny);
            }
            _ => {}
        }
        return None;
    }

    match key.code {
        KeyCode::Enter => {
            if key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
            {
                app.input.insert_newline();
                None
            } else if !app.is_busy() {
                app.input.take_submission()
            } else {
                None
            }
        }
        KeyCode::Backspace => {
            app.input.backspace();
            None
        }
        KeyCode::Delete => {
            app.input.delete();
            None
        }
        KeyCode::Left => {
            app.input.move_left();
            None
        }
        KeyCode::Right => {
            app.input.move_right();
            None
        }
        KeyCode::Home => {
            app.input.move_home();
            None
        }
        KeyCode::End => {
            app.input.move_end();
            None
        }
        KeyCode::Up => {
            app.input.history_prev();
            None
        }
        KeyCode::Down => {
            app.input.history_next();
            None
        }
        KeyCode::PageUp => {
            app.scroll_up(5);
            None
        }
        KeyCode::PageDown => {
            app.scroll_down(5);
            None
        }
        KeyCode::Char(c) => {
            app.input.insert_char(c);
            None
        }
        _ => None,
    }
}
