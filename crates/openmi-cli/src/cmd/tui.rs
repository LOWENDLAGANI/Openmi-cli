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
use openmi_core::{fetch_credit_status, AgentEvent, ApprovalDecision, PermissionMode, ProxyConfig};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, TokenStore};
use crate::exit_codes::CliError;
use crate::session::build_session_with_options;
use crate::session_store::SessionStore;
use crate::tui::app::{FullAccessModalState, TuiApp};
use crate::tui::approval::{PendingApproval, TuiApprovalHandler};
use crate::tui::render;
use crate::tui::slash::{execute_slash, parse_slash, SlashOutcome, FULL_ACCESS_CONFIRM_PHRASE};

#[derive(Debug, Clone)]
pub struct TuiOptions {
    pub model: String,
    pub permission: String,
    pub cwd: Option<PathBuf>,
    pub api_url: Option<String>,
    /// `None` = new session; `Some(None)` = resume latest; `Some(Some(id))` = resume specific ID.
    pub resume: Option<Option<String>>,
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
    let (proxy_config, base_url_for_credits, bearer_for_credits) = if let Some(ref tok) = stored {
        let base_url = resolve_api_base_url(opts.api_url.as_deref(), Some(tok));
        let client = DeviceAuthClient::new(&base_url)?;
        let valid = client.ensure_valid_tokens(&store).await?;
        let token = valid.access_token;
        (
            Some(ProxyConfig {
                endpoint: format!("{base_url}/functions/v1/chat"),
                bearer_token: token.clone(),
            }),
            Some(base_url),
            Some(token),
        )
    } else {
        let cfg = opts.api_url.as_deref().map(|explicit| ProxyConfig {
            endpoint: format!("{}/functions/v1/chat", explicit.trim_end_matches('/')),
            bearer_token: String::new(),
        });
        (cfg, opts.api_url.clone(), None)
    };

    let session_store = SessionStore::default_store()?;
    let resumed = match &opts.resume {
        None => None,
        Some(None) => session_store.load_latest(Some(&workdir))?,
        Some(Some(id)) => Some(session_store.load_by_id(id)?),
    };

    let session_id = resumed
        .as_ref()
        .map(|s| s.id.clone())
        .unwrap_or_else(SessionStore::new_session_id);
    let initial_model = resumed
        .as_ref()
        .map(|s| s.model.clone())
        .unwrap_or(opts.model);

    let checkpoints = Arc::new(Mutex::new(CheckpointStore::new()));
    let (approval_tx, mut approval_rx) = mpsc::channel::<PendingApproval>(8);
    let approval_handler = Arc::new(TuiApprovalHandler::new(approval_tx));

    let mut raw_session = build_session_with_options(
        &workdir,
        &initial_model,
        mode,
        Arc::clone(&checkpoints),
        approval_handler,
        proxy_config,
    )?;
    let has_openmi_md = raw_session.system_prompt().is_some();
    if let Some(ref saved) = resumed {
        raw_session.set_history(saved.messages.clone());
    }

    let mut mcp_manager = openmi_core::load_merged_mcp_config(None, &workdir)
        .ok()
        .map(|cfg| openmi_core::McpManager::from_config(&workdir, cfg));
    if let Some(mgr) = mcp_manager.as_mut() {
        mgr.start_all(raw_session.tool_registry_mut()).await;
    }

    let session = Arc::new(tokio::sync::Mutex::new(raw_session));

    install_panic_hook();
    enable_raw_mode().map_err(anyhow::Error::from)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(anyhow::Error::from)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(anyhow::Error::from)?;
    terminal.clear().map_err(anyhow::Error::from)?;

    let mut app = TuiApp::new(initial_model, mode, workdir.clone());
    if has_openmi_md {
        app.push_system("Loaded project instructions from OPENMI.md.");
    }
    if let Some(ref saved) = resumed {
        app.hydrate_from_history(&saved.messages);
        app.push_system(format!(
            "Resumed session `{}` ({} message(s)).",
            saved.id,
            saved.messages.len()
        ));
    }

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
                        match handle_key_event(&mut app, key, &mut active_cancel) {
                            KeyAction::None => {}
                            KeyAction::ConfirmedFullAccess => {
                                app.mode = PermissionMode::FullAccess;
                                session.lock().await.set_permission_mode(PermissionMode::FullAccess);
                                app.push_system("Permission mode switched to FullAccess.");
                            }
                            KeyAction::Submit(input_text) => {
                                if let Some(slash_cmd) = parse_slash(&input_text) {
                                    let outcome = {
                                        let cp = checkpoints.lock().expect("checkpoint mutex");
                                        execute_slash(slash_cmd, &app.model, app.mode, &cp, &workdir)
                                    };
                                    match outcome {
                                        SlashOutcome::Message(msg) => app.push_system(msg),
                                        SlashOutcome::Diff(diff_txt) => app.push_diff(diff_txt),
                                        SlashOutcome::Error(err) => app.push_error(err),
                                        SlashOutcome::SetModel(m) => {
                                            app.model = m.clone();
                                            session.lock().await.set_model(m.clone());
                                            app.push_system(format!("Switched model to `{m}`."));
                                        }
                                        SlashOutcome::SetMode(new_mode) => {
                                            app.mode = new_mode;
                                            session.lock().await.set_permission_mode(new_mode);
                                            app.push_system(format!("Switched permission mode to {new_mode:?}."));
                                        }
                                        SlashOutcome::RequireFullAccessConfirm => {
                                            app.full_access_modal = Some(FullAccessModalState::default());
                                        }
                                        SlashOutcome::ClearHistory => {
                                            session.lock().await.clear_history();
                                            app.transcript.clear();
                                            app.tools.clear();
                                            app.push_system("Cleared conversation history.");
                                        }
                                        SlashOutcome::RefreshCredits => {
                                            if let (Some(base), Some(tok)) = (base_url_for_credits.as_deref(), bearer_for_credits.as_deref()) {
                                                let ep = format!("{base}/functions/v1/credits");
                                                match fetch_credit_status(&ep, tok).await {
                                                    Ok(status) => {
                                                        app.credits = Some(status.balance);
                                                        app.push_system(format!("Credits: {} (plan: {})", status.balance, status.plan));
                                                    }
                                                    Err(err) => app.push_error(format!("Failed to fetch credits: {err}")),
                                                }
                                            } else {
                                                app.push_system("Not logged in — run `openmi login` to check credits.");
                                            }
                                        }
                                        SlashOutcome::McpStatus => {
                                            if let Some(mgr) = mcp_manager.as_ref() {
                                                let statuses = mgr.statuses();
                                                if statuses.is_empty() {
                                                    app.push_system("No MCP servers configured (add ~/.config/openmi/mcp.json or .openmi/mcp.json).");
                                                } else {
                                                    let summary = statuses
                                                        .iter()
                                                        .map(|s| {
                                                            let st = if s.running { "running" } else { "stopped" };
                                                            if let Some(err) = &s.error {
                                                                format!("{} [{}]: error: {}", s.name, st, err)
                                                            } else {
                                                                format!("{} [{}]: {} tool(s)", s.name, st, s.tools.len())
                                                            }
                                                        })
                                                        .collect::<Vec<_>>()
                                                        .join("\n");
                                                    app.push_system(summary);
                                                }
                                            } else {
                                                app.push_system("No MCP servers configured.");
                                            }
                                        }
                                        SlashOutcome::McpRestart(target) => {
                                            if let Some(mgr) = mcp_manager.as_mut() {
                                                let mut sess = session.lock().await;
                                                match mgr.restart(target.as_deref(), sess.tool_registry_mut()).await {
                                                    Ok(()) => app.push_system(format!(
                                                        "Restarted MCP server(s): {}",
                                                        target.as_deref().unwrap_or("all")
                                                    )),
                                                    Err(err) => app.push_error(format!("MCP restart failed: {err}")),
                                                }
                                            } else {
                                                app.push_system("No MCP servers configured.");
                                            }
                                        }
                                        SlashOutcome::Quit => {
                                            app.should_quit = true;
                                        }
                                    }
                                } else {
                                    checkpoints.lock().expect("checkpoint mutex").begin_run();
                                    app.push_user(input_text.clone());
                                    let cancel = CancellationToken::new();
                                    active_cancel = Some(cancel.clone());
                                    let sess = Arc::clone(&session);
                                    let tx = agent_tx.clone();
                                    tokio::spawn(async move {
                                        let mut s = sess.lock().await;
                                        if let Err(err) = s.run_turn(&input_text, tx.clone(), cancel).await {
                                            let _ = tx.send(AgentEvent::Error {
                                                code: "turn_error".to_string(),
                                                message: err.to_string(),
                                            }).await;
                                        }
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(agent_ev) = agent_rx.recv() => {
                let became_idle = matches!(
                    agent_ev,
                    AgentEvent::ControlStateChanged { state: openmi_core::ControlState::Idle }
                );
                if became_idle {
                    active_cancel = None;
                    // Persist session snapshot after each completed turn.
                    let msgs = session.lock().await.history().to_vec();
                    let snap = SessionStore::make_snapshot(
                        session_id.clone(),
                        app.model.clone(),
                        workdir.clone(),
                        msgs,
                    );
                    let _ = session_store.save(&snap);
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

    if let Some(mgr) = mcp_manager.as_mut() {
        mgr.shutdown_all().await;
    }

    // Final session save on clean exit if any history exists.
    let final_msgs = session.lock().await.history().to_vec();
    if !final_msgs.is_empty() {
        let snap = SessionStore::make_snapshot(session_id, app.model, workdir, final_msgs);
        let _ = session_store.save(&snap);
    }

    Ok(())
}

enum KeyAction {
    None,
    Submit(String),
    ConfirmedFullAccess,
}

fn handle_key_event(
    app: &mut TuiApp,
    key: KeyEvent,
    active_cancel: &mut Option<CancellationToken>,
) -> KeyAction {
    if key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        let should_cancel = app.handle_interrupt();
        if should_cancel {
            if let Some(tok) = active_cancel.as_ref() {
                tok.cancel();
            }
        }
        return KeyAction::None;
    }

    app.armed_quit = false;

    // 1. Approval popup intercepts y / n / a.
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
        return KeyAction::None;
    }

    // 2. FullAccess typed confirmation modal intercepts all input until confirmed or Esc.
    if let Some(modal) = app.full_access_modal.as_mut() {
        match key.code {
            KeyCode::Enter => {
                if modal.typed.trim() == FULL_ACCESS_CONFIRM_PHRASE {
                    app.full_access_modal = None;
                    return KeyAction::ConfirmedFullAccess;
                }
                modal.error = Some(format!(
                    "Confirmation phrase does not match `{FULL_ACCESS_CONFIRM_PHRASE}`."
                ));
            }
            KeyCode::Backspace => {
                modal.typed.pop();
                modal.error = None;
            }
            KeyCode::Char(c) => {
                modal.typed.push(c);
                modal.error = None;
            }
            _ => {}
        }
        return KeyAction::None;
    }

    // 3. Normal prompt input + @path Tab completion.
    let action = match key.code {
        KeyCode::Tab => {
            app.apply_top_mention_completion();
            KeyAction::None
        }
        KeyCode::Enter => {
            if key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
            {
                app.input.insert_newline();
                KeyAction::None
            } else if !app.is_busy() {
                app.mention_suggestions.clear();
                app.input
                    .take_submission()
                    .map(KeyAction::Submit)
                    .unwrap_or(KeyAction::None)
            } else {
                KeyAction::None
            }
        }
        KeyCode::Backspace => {
            app.input.backspace();
            KeyAction::None
        }
        KeyCode::Delete => {
            app.input.delete();
            KeyAction::None
        }
        KeyCode::Left => {
            app.input.move_left();
            KeyAction::None
        }
        KeyCode::Right => {
            app.input.move_right();
            KeyAction::None
        }
        KeyCode::Home => {
            app.input.move_home();
            KeyAction::None
        }
        KeyCode::End => {
            app.input.move_end();
            KeyAction::None
        }
        KeyCode::Up => {
            app.input.history_prev();
            KeyAction::None
        }
        KeyCode::Down => {
            app.input.history_next();
            KeyAction::None
        }
        KeyCode::PageUp => {
            app.scroll_up(5);
            KeyAction::None
        }
        KeyCode::PageDown => {
            app.scroll_down(5);
            KeyAction::None
        }
        KeyCode::Char(c) => {
            app.input.insert_char(c);
            KeyAction::None
        }
        _ => KeyAction::None,
    };

    app.refresh_mention_suggestions();
    action
}
