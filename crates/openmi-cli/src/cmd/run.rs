use std::env;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use openmi_checkpoints::CheckpointStore;
use openmi_core::{PermissionMode, ProxyConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, TokenStore};
use crate::exit_codes::CliError;
use crate::output::json::SingleJsonRenderer;
use crate::output::jsonl::JsonlRenderer;
use crate::output::text::TextRenderer;
use crate::output::{EventRenderer, OutputFormat};
use crate::session::{build_session_with_options, select_headless_approval_handler};
use crate::settings::UserSettings;
use crate::tui::mention::load_image_attachments;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub prompt: String,
    pub model: String,
    pub permission: Option<String>,
    pub cwd: Option<PathBuf>,
    pub output_format: OutputFormat,
    pub yes: bool,
    pub api_url: Option<String>,
    pub max_steps: Option<u32>,
}

pub async fn execute(opts: RunOptions) -> Result<(), CliError> {
    let mode: PermissionMode = match opts.permission {
        Some(raw) => raw.parse().map_err(CliError::Usage)?,
        None => UserSettings::load_default().effective_permission_mode(),
    };
    let workdir = match opts.cwd {
        Some(dir) => dir,
        None => env::current_dir().map_err(anyhow::Error::from)?,
    };

    // Resolve optional managed proxy config if logged in or explicit API URL is provided.
    let store = default_token_store()?;
    let stored = store.load()?;
    let (proxy_config, tier_max_steps) = if let Some(ref tok) = stored {
        let base_url = resolve_api_base_url(opts.api_url.as_deref(), Some(tok));
        let client = DeviceAuthClient::new(&base_url)?;
        let valid = client.ensure_valid_tokens(&store).await?;
        let credits_ep = format!("{base_url}/functions/v1/credits");
        let tier_steps = openmi_core::fetch_credit_status(&credits_ep, &valid.access_token)
            .await
            .ok()
            .map(|s| s.effective_max_steps());
        (
            Some(ProxyConfig {
                endpoint: format!("{base_url}/functions/v1/chat"),
                bearer_token: valid.access_token,
            }),
            tier_steps,
        )
    } else {
        (
            opts.api_url.as_deref().map(|explicit| ProxyConfig {
                endpoint: format!("{}/functions/v1/chat", explicit.trim_end_matches('/')),
                bearer_token: String::new(),
            }),
            None,
        )
    };

    let checkpoints = Arc::new(Mutex::new(CheckpointStore::new()));
    checkpoints.lock().expect("checkpoint mutex").begin_run();

    let approval_handler = select_headless_approval_handler(opts.yes);
    let mut session = build_session_with_options(
        &workdir,
        &opts.model,
        mode,
        checkpoints,
        approval_handler,
        proxy_config,
    )?;
    match (tier_max_steps, opts.max_steps) {
        (Some(tier_cap), Some(requested)) => session.set_max_steps(requested.min(tier_cap)),
        (Some(tier_cap), None) => session.set_max_steps(tier_cap),
        (None, Some(requested)) => session.set_max_steps(requested),
        (None, None) => {}
    }

    // Verify workspace trust before spawning any project-level `.openmi/mcp.json` servers.
    let workspace_trusted = crate::trust::ensure_workspace_trust(&workdir, false);
    let mut mcp_manager =
        openmi_core::load_merged_mcp_config_with_trust(None, &workdir, workspace_trusted)
            .ok()
            .filter(|cfg| !cfg.mcp_servers.is_empty())
            .map(|cfg| openmi_core::McpManager::from_config(&workdir, cfg));
    if let Some(mgr) = mcp_manager.as_mut() {
        mgr.start_all(session.tool_registry_mut()).await;
    }

    let renderer: Box<dyn EventRenderer> = match opts.output_format {
        OutputFormat::Text => Box::new(TextRenderer::new(io::stdout())),
        OutputFormat::Json => Box::new(SingleJsonRenderer::new(io::stdout())),
        OutputFormat::StreamJson => Box::new(JsonlRenderer::new(io::stdout())),
    };

    let cancel = CancellationToken::new();
    let cancel_sig = cancel.clone();
    let ctrl_c_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_sig.cancel();
        }
    });

    let attachments = load_image_attachments(&opts.prompt, &workdir);
    let res = run_with_renderer_and_attachments(
        &mut session,
        &opts.prompt,
        attachments,
        renderer,
        cancel,
    )
    .await;
    ctrl_c_task.abort();
    if let Some(mgr) = mcp_manager.as_mut() {
        mgr.shutdown_all().await;
    }
    res
}

#[allow(dead_code)]
pub async fn run_with_renderer(
    session: &mut openmi_core::AgentSession,
    prompt: &str,
    renderer: Box<dyn EventRenderer>,
    cancel: CancellationToken,
) -> Result<(), CliError> {
    run_with_renderer_and_attachments(session, prompt, Vec::new(), renderer, cancel).await
}

pub async fn run_with_renderer_and_attachments(
    session: &mut openmi_core::AgentSession,
    prompt: &str,
    attachments: Vec<openmi_core::ImageAttachment>,
    mut renderer: Box<dyn EventRenderer>,
    cancel: CancellationToken,
) -> Result<(), CliError> {
    let (tx, mut rx) = mpsc::channel(64);
    let render_task = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = renderer.render_event(&ev);
        }
        let _ = renderer.finish();
    });

    let turn_res = session
        .run_turn_with_attachments(prompt, attachments, tx, cancel)
        .await;
    let _ = render_task.await;
    turn_res.map_err(CliError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    async fn mock_sse(handler: impl Fn(usize) -> (u16, String) + Send + Sync + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(handler);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let c = Arc::clone(&count);
                let h = Arc::clone(&handler);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16384];
                    let _ = sock.read(&mut buf).await;
                    let idx = c.fetch_add(1, Ordering::SeqCst);
                    let (status, body) = h(idx);
                    let resp = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}/functions/v1/chat")
    }

    #[tokio::test]
    async fn snapshot_mock_sse_jsonl_and_text_renderers() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = mock_sse(|_| {
            (
                200,
                concat!(
                    "data: {\"type\":\"text_delta\",\"text\":\"Hello \"}\n\n",
                    "data: {\"type\":\"text_delta\",\"text\":\"Openmi!\"}\n\n",
                    "data: {\"type\":\"done\",\"finish_reason\":\"stop\"}\n\n"
                )
                .to_string(),
            )
        })
        .await;

        let cp = Arc::new(Mutex::new(CheckpointStore::new()));
        let mut session = build_session_with_options(
            tmp.path(),
            "openmi-default",
            PermissionMode::Ask,
            cp,
            select_headless_approval_handler(true),
            Some(ProxyConfig {
                endpoint,
                bearer_token: "tok".into(),
            }),
        )
        .unwrap();

        let out = Arc::new(Mutex::new(Vec::new()));
        run_with_renderer(
            &mut session,
            "say hello",
            Box::new(JsonlRenderer::new(SharedBuf(Arc::clone(&out)))),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        let jsonl = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert!(lines
            .iter()
            .all(|l| l.starts_with(r#"{"schema_version":"1","event":"#)));
        assert!(jsonl.contains(r#""type":"text_delta","text":"Hello ""#));
        assert!(jsonl.contains(r#""type":"text_delta","text":"Openmi!""#));
        assert!(jsonl.contains(r#""type":"turn_completed","finish_reason":"stop""#));
    }

    #[tokio::test]
    async fn non_interactive_ask_denies_write_unless_yes_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = mock_sse(|step| {
            if step == 0 {
                (
                    200,
                    "data: {\"type\":\"tool_call\",\"call_id\":\"c1\",\"tool_name\":\"write_file\",\"arguments\":{\"path\":\"a.txt\",\"content\":\"ok\"}}\n\n"
                        .into(),
                )
            } else {
                (
                    200,
                    "data: {\"type\":\"text_delta\",\"text\":\"done\"}\n\ndata: {\"type\":\"done\",\"finish_reason\":\"stop\"}\n\n"
                        .into(),
                )
            }
        })
        .await;

        // Without --yes and non-interactive deny handler -> file is NOT created.
        let cp = Arc::new(Mutex::new(CheckpointStore::new()));
        let mut session = build_session_with_options(
            tmp.path(),
            "openmi-default",
            PermissionMode::Ask,
            cp,
            Arc::new(crate::session::NonInteractiveDenyHandler),
            Some(ProxyConfig {
                endpoint: endpoint.clone(),
                bearer_token: "tok".into(),
            }),
        )
        .unwrap();

        let out = Arc::new(Mutex::new(Vec::new()));
        run_with_renderer(
            &mut session,
            "write a.txt",
            Box::new(TextRenderer::new(SharedBuf(Arc::clone(&out)))),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(!tmp.path().join("a.txt").exists());
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        assert!(text.contains("[tool:err] write_file: user denied tool `write_file`"));
    }
}
