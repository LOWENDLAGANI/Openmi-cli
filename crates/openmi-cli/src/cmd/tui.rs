use std::env;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use openmi_checkpoints::CheckpointStore;
use openmi_core::{
    fetch_credit_status, fetch_models_with_fallback, AgentEvent, ApprovalDecision, CreditStatus,
    ModelInfo, PermissionMode, ProxyConfig,
};
use openmi_platform::AppPaths;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, TokenStore};
use crate::exit_codes::CliError;
use crate::session::{build_session_with_options, generate_starter_openmi_md};
use crate::session_store::{SavedSession, SessionStore};
use crate::settings::UserSettings;
use crate::tui::app::{
    resolve_folder_input, FolderPickerState, FullAccessModalState, ModelSelectorState,
    ResumePickerState, SettingsModalState, TuiApp,
};
use crate::tui::approval::{PendingApproval, TuiApprovalHandler};
use crate::tui::render;
use crate::tui::slash::{execute_slash, parse_slash, SlashOutcome, FULL_ACCESS_CONFIRM_PHRASE};

#[derive(Debug, Clone)]
pub struct TuiOptions {
    pub model: String,
    pub permission: Option<String>,
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
    let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
}

pub async fn execute(opts: TuiOptions) -> Result<(), CliError> {
    let user_settings = UserSettings::load_default();
    let mode: PermissionMode = match opts.permission {
        Some(raw) => raw.parse().map_err(CliError::Usage)?,
        None => user_settings.effective_permission_mode(),
    };
    let mut workdir = match opts.cwd {
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

    let mut session_id = resumed
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
        Arc::clone(&approval_handler) as Arc<dyn openmi_core::ApprovalHandler>,
        proxy_config.clone(),
    )?;
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
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste).map_err(anyhow::Error::from)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(anyhow::Error::from)?;
    terminal.clear().map_err(anyhow::Error::from)?;

    let mut app = TuiApp::new(initial_model, mode, workdir.clone());
    app.settings = user_settings;
    if let Some(ref saved) = resumed {
        app.hydrate_from_history(&saved.messages);
        app.push_system("Resumed your previous chat.");
    }

    let (agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(128);
    let (credit_tx, mut credit_rx) = mpsc::channel::<CreditStatus>(8);
    let (model_tx, mut model_rx) = mpsc::channel::<Vec<ModelInfo>>(2);
    let mut active_cancel: Option<CancellationToken> = None;

    // Load the model catalog in the background so display names show up right away.
    {
        let tx = model_tx.clone();
        let endpoint = base_url_for_credits
            .as_deref()
            .map(|b| format!("{b}/functions/v1/models"));
        let token = bearer_for_credits.clone();
        let cache_path = AppPaths::resolve().ok().map(|p| p.models_cache_file());
        tokio::spawn(async move {
            let models = fetch_models_with_fallback(
                endpoint.as_deref(),
                token.as_deref(),
                cache_path.as_deref(),
            )
            .await;
            let _ = tx.send(models).await;
        });
    }

    // Fetch initial credit/pass status in background so status bar populates right away.
    if let (Some(base), Some(tok)) = (base_url_for_credits.clone(), bearer_for_credits.clone()) {
        let tx = credit_tx.clone();
        tokio::spawn(async move {
            let ep = format!("{base}/functions/v1/credits");
            if let Ok(status) = fetch_credit_status(&ep, &tok).await {
                let _ = tx.send(status).await;
            }
        });
    }

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
                    Event::Paste(text) => {
                        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                        if let Some(modal) = app.full_access_modal.as_mut() {
                            modal.typed.push_str(normalized.trim());
                            modal.error = None;
                        } else if app.pending_approval.is_none()
                            && app.model_selector.is_none()
                            && app.folder_picker.is_none()
                            && app.resume_picker.is_none()
                            && app.settings_modal.is_none()
                        {
                            for ch in normalized.chars() {
                                if ch == '\n' {
                                    app.input.insert_newline();
                                } else {
                                    app.input.insert_char(ch);
                                }
                            }
                            app.refresh_slash_suggestions();
                            app.refresh_mention_suggestions();
                        }
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
                            KeyAction::SelectedModel(chosen) => {
                                app.model = chosen.id.clone();
                                session.lock().await.set_model(chosen.id.clone());
                                app.push_system(format_model_switch_message(&app, &chosen));
                            }
                            KeyAction::SelectedFolder(chosen) => {
                                switch_project_folder(
                                    chosen,
                                    &mut workdir,
                                    &mut app,
                                    &session,
                                    &checkpoints,
                                    &approval_handler,
                                    &proxy_config,
                                    &mut mcp_manager,
                                )
                                .await;
                            }
                            KeyAction::SettingsChanged(new_mode) => {
                                if let Some(m) = new_mode {
                                    session.lock().await.set_permission_mode(m);
                                }
                                let _ = app.settings.save_default();
                                app.status_hint = "Settings saved".to_string();
                            }
                            KeyAction::CycleMode(next_mode) => {
                                session.lock().await.set_permission_mode(next_mode);
                                let label = match next_mode {
                                    PermissionMode::Ask => "Ask (confirm edits)",
                                    PermissionMode::Trusted => "Trusted (auto-approve workspace)",
                                    PermissionMode::ReadOnly => "Read-only (no edits)",
                                    PermissionMode::FullAccess => "Full access",
                                };
                                app.status_hint = format!("Mode: {label}");
                            }
                            KeyAction::SelectedSession(saved) => {
                                session_id = saved.id.clone();
                                app.model = saved.model.clone();
                                app.transcript.clear();
                                app.tools.clear();
                                app.hydrate_from_history(&saved.messages);
                                if saved.cwd.is_dir() && saved.cwd != workdir {
                                    switch_project_folder(
                                        saved.cwd.clone(),
                                        &mut workdir,
                                        &mut app,
                                        &session,
                                        &checkpoints,
                                        &approval_handler,
                                        &proxy_config,
                                        &mut mcp_manager,
                                    )
                                    .await;
                                }
                                {
                                    let mut guard = session.lock().await;
                                    guard.set_model(saved.model);
                                    guard.set_history(saved.messages);
                                }
                                app.push_system("Resumed your previous chat.");
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
                                        SlashOutcome::OpenSettings => {
                                            app.settings_modal =
                                                Some(SettingsModalState::default());
                                        }
                                        SlashOutcome::OpenResumePicker => {
                                            match session_store.list_recent(Some(&workdir), 20) {
                                                Ok(sessions) => {
                                                    app.resume_picker =
                                                        Some(ResumePickerState::new(sessions));
                                                }
                                                Err(err) => app.push_error(format!(
                                                    "Could not load saved chats: {err}"
                                                )),
                                            }
                                        }
                                        SlashOutcome::InitProjectGuide => {
                                            let target = workdir.join("OPENMI.md");
                                            if target.exists() {
                                                app.push_system(
                                                    "OPENMI.md already exists in this folder.",
                                                );
                                            } else {
                                                let content = generate_starter_openmi_md(&workdir);
                                                match openmi_platform::atomic_write(
                                                    &target,
                                                    content.as_bytes(),
                                                ) {
                                                    Ok(()) => {
                                                        session
                                                            .lock()
                                                            .await
                                                            .set_system_prompt(Some(content));
                                                        app.push_system(
                                                            "Created OPENMI.md in your project folder and loaded it.",
                                                        );
                                                    }
                                                    Err(err) => app.push_error(format!(
                                                        "Could not create OPENMI.md: {err}"
                                                    )),
                                                }
                                            }
                                        }
                                        SlashOutcome::OpenFolderPicker => {
                                            app.folder_picker =
                                                Some(FolderPickerState::new(workdir.clone()));
                                        }
                                        SlashOutcome::SetFolder(raw) => {
                                            let target = resolve_folder_input(&raw, &workdir);
                                            switch_project_folder(
                                                target,
                                                &mut workdir,
                                                &mut app,
                                                &session,
                                                &checkpoints,
                                                &approval_handler,
                                                &proxy_config,
                                                &mut mcp_manager,
                                            )
                                            .await;
                                        }
                                        SlashOutcome::OpenModelSelector => {
                                            let endpoint = base_url_for_credits
                                                .as_deref()
                                                .map(|b| format!("{b}/functions/v1/models"));
                                            let cache_path = AppPaths::resolve()
                                                .ok()
                                                .map(|p| p.models_cache_file());
                                            let models = fetch_models_with_fallback(
                                                endpoint.as_deref(),
                                                bearer_for_credits.as_deref(),
                                                cache_path.as_deref(),
                                            )
                                            .await;
                                            app.apply_model_catalog(&models);
                                            app.model_selector =
                                                Some(ModelSelectorState::new(models, &app.model));
                                        }
                                        SlashOutcome::SetModel(m) => {
                                            // Accept either the model ID or its display name.
                                            let m = app
                                                .model_names
                                                .iter()
                                                .find(|(id, name)| {
                                                    id.eq_ignore_ascii_case(&m) || name.eq_ignore_ascii_case(&m)
                                                })
                                                .map(|(id, _)| id.clone())
                                                .unwrap_or(m);
                                            app.model = m.clone();
                                            session.lock().await.set_model(m.clone());
                                            let active_note = app
                                                .credit_status
                                                .as_ref()
                                                .and_then(|cs| cs.active_pass_remaining())
                                                .map(|(am, mins)| {
                                                    if am == m {
                                                        format!(" Active pass already running ({mins}m left).")
                                                    } else {
                                                        format!(" Note: sending a prompt will replace your active `{am}` pass ({mins}m left).")
                                                    }
                                                })
                                                .unwrap_or_else(|| " Your 1-hour pass starts when you send your first prompt.".to_string());
                                            let shown = app.display_name_for(&m).to_string();
                                            app.push_system(format!("Switched to {shown}.{active_note}"));
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
                                            app.status_hint.clear();
                                        }
                                        SlashOutcome::RefreshCredits => {
                                            if let (Some(base), Some(tok)) = (base_url_for_credits.as_deref(), bearer_for_credits.as_deref()) {
                                                let ep = format!("{base}/functions/v1/credits");
                                                match fetch_credit_status(&ep, tok).await {
                                                    Ok(status) => {
                                                        let summary = format_credit_summary(&status);
                                                        app.apply_credit_status(status);
                                                        app.push_system(summary);
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
                    // Refresh credit/pass status in background so status bar updates live.
                    if let (Some(base), Some(tok)) = (
                        base_url_for_credits.clone(),
                        bearer_for_credits.clone(),
                    ) {
                        let tx = credit_tx.clone();
                        tokio::spawn(async move {
                            let ep = format!("{base}/functions/v1/credits");
                            if let Ok(status) = fetch_credit_status(&ep, &tok).await {
                                let _ = tx.send(status).await;
                            }
                        });
                    }
                }
                app.apply_agent_event(agent_ev);
                dirty = true;
            }
            Some(models) = model_rx.recv() => {
                app.apply_model_catalog(&models);
                dirty = true;
            }
            Some(status) = credit_rx.recv() => {
                session.lock().await.set_max_steps(status.effective_max_steps());
                app.apply_credit_status(status);
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

fn format_credit_summary(status: &CreditStatus) -> String {
    let cap = status
        .daily_limit
        .unwrap_or(if status.is_paid { 15 } else { 6 });
    let token_cap = status.effective_daily_token_limit();
    let token_line = if token_cap > 0 {
        format!(
            "{}/{} tok",
            openmi_core::format_token_count(status.daily_tokens_used),
            openmi_core::format_token_count(token_cap)
        )
    } else {
        format!(
            "{} tok",
            openmi_core::format_token_count(status.daily_tokens_used)
        )
    };
    let plan_line = if status.is_paid {
        match status.paid_until.as_deref() {
            Some(u) => format!("PAID (until {})", u.get(..10).unwrap_or(u)),
            None => "PAID".to_string(),
        }
    } else {
        "FREE".to_string()
    };
    let pass_line = match status.active_pass_remaining() {
        Some((m, mins)) => format!("{m} ({mins}m remaining)"),
        None => "none (starts on first prompt)".to_string(),
    };
    let mut out = format!(
        "Plan: {plan_line} | Daily: {}/{cap} cr | Tokens: {token_line} | Max turns: {} | Wallet: {} cr | Active pass: {pass_line}",
        status.balance,
        status.effective_max_steps(),
        status.wallet_credit
    );
    if !status.is_paid {
        out.push_str(&format!(
            "\nUpgrade to Paid (15 cr/day, 7M tok/day, 70 turns + Paid models): {}",
            status.effective_upgrade_url()
        ));
    }
    out
}

fn format_model_switch_message(app: &TuiApp, chosen: &ModelInfo) -> String {
    let is_paid = app
        .credit_status
        .as_ref()
        .map(|cs| cs.is_paid)
        .unwrap_or(false);
    if chosen.paid_only && !is_paid {
        let url = app
            .credit_status
            .as_ref()
            .map(|cs| cs.effective_upgrade_url())
            .unwrap_or("https://sociabuzz.com/minetallestmc/tribe");
        return format!(
            "Selected {} ({} cr/hr) — Paid pass required before prompting. Upgrade: {url}",
            chosen.name(),
            chosen.credit_cost
        );
    }
    if let Some((active_id, mins)) = app
        .credit_status
        .as_ref()
        .and_then(|cs| cs.active_pass_remaining())
    {
        if active_id == chosen.id {
            return format!(
                "Switched to {} — active 1-hour pass running ({mins}m remaining).",
                chosen.name()
            );
        }
        return format!(
            "Switched to {} ({} cr/hr). Your 1-hour pass starts when you send your first prompt (replaces active {} pass, {mins}m left).",
            chosen.name(),
            chosen.credit_cost,
            app.display_name_for(active_id)
        );
    }
    format!(
        "Switched to {} ({} cr/hr). Your 1-hour pass starts when you send your first prompt.",
        chosen.name(),
        chosen.credit_cost
    )
}

#[allow(clippy::too_many_arguments)]
async fn switch_project_folder(
    target: PathBuf,
    workdir: &mut PathBuf,
    app: &mut TuiApp,
    session: &Arc<tokio::sync::Mutex<openmi_core::AgentSession>>,
    checkpoints: &Arc<Mutex<CheckpointStore>>,
    approval_handler: &Arc<TuiApprovalHandler>,
    proxy_config: &Option<ProxyConfig>,
    mcp_manager: &mut Option<openmi_core::McpManager>,
) {
    if !target.is_dir() {
        app.push_error(format!("Folder not found: {}", target.display()));
        return;
    }
    let canonical = std::fs::canonicalize(&target).unwrap_or(target);
    let mut new_session = match build_session_with_options(
        &canonical,
        &app.model,
        app.mode,
        Arc::clone(checkpoints),
        Arc::clone(approval_handler) as Arc<dyn openmi_core::ApprovalHandler>,
        proxy_config.clone(),
    ) {
        Ok(s) => s,
        Err(err) => {
            app.push_error(format!("Could not open folder: {err}"));
            return;
        }
    };

    if let Some(old_mgr) = mcp_manager.as_mut() {
        old_mgr.shutdown_all().await;
    }
    let mut next_mgr = openmi_core::load_merged_mcp_config(None, &canonical)
        .ok()
        .map(|cfg| openmi_core::McpManager::from_config(&canonical, cfg));
    if let Some(mgr) = next_mgr.as_mut() {
        mgr.start_all(new_session.tool_registry_mut()).await;
    }
    *mcp_manager = next_mgr;

    {
        let mut guard = session.lock().await;
        let prev_history = guard.history().to_vec();
        new_session.set_history(prev_history);
        *guard = new_session;
    }

    *workdir = canonical.clone();
    app.set_cwd(canonical);
    if app.transcript.is_empty() {
        app.status_hint = format!("Project folder: {}", app.friendly_cwd());
    } else {
        app.push_system(format!(
            "Switched project folder to {}.",
            app.friendly_cwd()
        ));
    }
}

enum KeyAction {
    None,
    Submit(String),
    ConfirmedFullAccess,
    SelectedModel(ModelInfo),
    SelectedFolder(PathBuf),
    SettingsChanged(Option<PermissionMode>),
    CycleMode(PermissionMode),
    SelectedSession(SavedSession),
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

    // 0. Interactive `/settings` modal.
    if app.settings_modal.is_some() {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(m) = app.settings_modal.as_mut() {
                    m.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(m) = app.settings_modal.as_mut() {
                    m.move_down();
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let changed_mode = app.toggle_selected_setting();
                return KeyAction::SettingsChanged(changed_mode);
            }
            _ => {}
        }
        return KeyAction::None;
    }

    // 1. Approval popup intercepts ↑/↓/Enter as well as y / a / n.
    if app.pending_approval.is_some() {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => app.approval_move_up(),
            KeyCode::Down | KeyCode::Char('j') => app.approval_move_down(),
            KeyCode::Enter => {
                let decision = app.approval_selected_decision();
                app.resolve_approval(decision);
            }
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

    // 3. Interactive `/model` selector modal intercepts Up/Down/j/k/Enter until selected or Esc.
    if let Some(selector) = app.model_selector.as_mut() {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => selector.move_up(),
            KeyCode::Down | KeyCode::Char('j') => selector.move_down(),
            KeyCode::Enter => {
                if let Some(chosen) = selector.current().cloned() {
                    app.model_selector = None;
                    return KeyAction::SelectedModel(chosen);
                }
                app.model_selector = None;
            }
            _ => {}
        }
        return KeyAction::None;
    }

    // 4. Interactive `/folder` picker modal.
    if let Some(picker) = app.folder_picker.as_mut() {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => picker.move_up(),
            KeyCode::Down | KeyCode::Char('j') => picker.move_down(),
            KeyCode::Backspace | KeyCode::Left => picker.go_up(),
            KeyCode::Enter | KeyCode::Right => {
                if let Some(chosen) = picker.enter() {
                    app.folder_picker = None;
                    return KeyAction::SelectedFolder(chosen);
                }
            }
            KeyCode::Char(' ') => {
                let chosen = picker.select_highlighted();
                app.folder_picker = None;
                return KeyAction::SelectedFolder(chosen);
            }
            _ => {}
        }
        return KeyAction::None;
    }

    // 5. Interactive `/resume` session picker modal.
    if let Some(resume) = app.resume_picker.as_mut() {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => resume.move_up(),
            KeyCode::Down | KeyCode::Char('j') => resume.move_down(),
            KeyCode::Enter => {
                if let Some(chosen) = resume.current().cloned() {
                    app.resume_picker = None;
                    return KeyAction::SelectedSession(chosen);
                }
                app.resume_picker = None;
            }
            _ => {}
        }
        return KeyAction::None;
    }

    // 6. Shift+Tab cycles safe permission modes (Ask -> Trusted -> Read-only -> Ask).
    if key.code == KeyCode::BackTab
        || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
    {
        let next = app.cycle_permission_mode();
        return KeyAction::CycleMode(next);
    }

    // 7. Normal prompt input + live `/` command palette + `@path` Tab completion.
    let action = match key.code {
        KeyCode::Tab => {
            if !app.slash_suggestions.is_empty() {
                app.apply_selected_slash_completion();
            } else {
                app.apply_top_mention_completion();
            }
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
                if let Some(cmd) = app.take_selected_slash_command() {
                    KeyAction::Submit(cmd)
                } else {
                    app.input
                        .take_submission()
                        .map(KeyAction::Submit)
                        .unwrap_or(KeyAction::None)
                }
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
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                app.scroll_up(3);
            } else if !app.slash_suggestions.is_empty() {
                app.slash_move_up();
            } else {
                app.input.history_prev();
            }
            KeyAction::None
        }
        KeyCode::Down => {
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                app.scroll_down(3);
            } else if !app.slash_suggestions.is_empty() {
                app.slash_move_down();
            } else {
                app.input.history_next();
            }
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

    app.refresh_slash_suggestions();
    app.refresh_mention_suggestions();
    action
}
