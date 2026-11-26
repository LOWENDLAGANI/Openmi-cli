use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use openmi_core::{
    AgentEvent, AgentSession, McpConfigFile, McpManager, McpServerConfig, PermissionMode,
    ProxyConfig,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// When invoked with `OPENMI_MOCK_MCP_SERVER=1`, acts as a real stdio JSON-RPC 2.0
/// filesystem MCP server (works identically on Linux, macOS, Windows, and Termux).
fn run_mock_stdio_mcp_server() {
    let root = PathBuf::from(
        std::env::var("OPENMI_MOCK_MCP_ROOT").expect("OPENMI_MOCK_MCP_ROOT must be set"),
    );
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line_res in stdin.lock().lines() {
        let Ok(line) = line_res else {
            break;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();

        // Notifications have no `id`.
        let Some(id) = id else {
            continue;
        };

        let result = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "mock-filesystem", "version": "1.0.0" }
            }),
            "tools/list" => serde_json::json!({
                "tools": [
                    {
                        "name": "read_text",
                        "description": "Read a text file from root",
                        "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } } }
                    },
                    {
                        "name": "write_text",
                        "description": "Write a text file under root",
                        "inputSchema": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } } }
                    }
                ]
            }),
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or_default();
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_default();
                let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let target = root.join(rel);

                match name {
                    "read_text" => {
                        let content = fs::read_to_string(&target).unwrap_or_default();
                        serde_json::json!({
                            "content": [{ "type": "text", "text": content }]
                        })
                    }
                    "write_text" => {
                        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let _ = fs::write(&target, content);
                        serde_json::json!({
                            "content": [{ "type": "text", "text": format!("wrote {rel}") }]
                        })
                    }
                    other => serde_json::json!({
                        "content": [{ "type": "text", "text": format!("unknown tool {other}") }]
                    }),
                }
            }
            _ => serde_json::json!({}),
        };

        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        let _ = writeln!(stdout, "{resp}");
        let _ = stdout.flush();
    }
}

async fn spawn_mock_sse(responder: impl Fn(usize) -> String + Send + Sync + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let responder = Arc::new(responder);

    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let c = Arc::clone(&count);
            let r = Arc::clone(&responder);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let _ = sock.read(&mut buf).await;
                let idx = c.fetch_add(1, Ordering::SeqCst);
                let body = r(idx);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/functions/v1/chat")
}

fn main() {
    if std::env::var("OPENMI_MOCK_MCP_SERVER").as_deref() == Ok("1") {
        run_mock_stdio_mcp_server();
        return;
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("hello.txt"), "hello from mcp filesystem").unwrap();

        let self_exe = std::env::current_exe().unwrap();
        let mut env = BTreeMap::new();
        env.insert("OPENMI_MOCK_MCP_SERVER".to_string(), "1".to_string());
        env.insert(
            "OPENMI_MOCK_MCP_ROOT".to_string(),
            tmp.path().display().to_string(),
        );

        let mut servers = BTreeMap::new();
        servers.insert(
            "filesystem".to_string(),
            McpServerConfig {
                command: self_exe.display().to_string(),
                args: Vec::new(),
                env,
                // Allowlist read_text in Trusted mode; write_text must still require approval!
                allowlist: vec!["read_text".to_string()],
            },
        );

        let mut manager = McpManager::from_config(tmp.path(), McpConfigFile { mcp_servers: servers });
        let mut session = AgentSession::new(tmp.path(), "openmi-default").unwrap();
        session.set_permission_mode(PermissionMode::Trusted);

        manager.start_all(session.tool_registry_mut()).await;

        let statuses = manager.statuses();
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].running, "mock MCP server should be running");
        assert!(statuses[0]
            .tools
            .contains(&"mcp__filesystem__read_text".to_string()));
        assert!(statuses[0]
            .tools
            .contains(&"mcp__filesystem__write_text".to_string()));

        // Mock LLM calls allowlisted read_text (succeeds in Trusted) and non-allowlisted write_text
        // (denied by default DenyAllApprovalHandler because MCP never auto-approves in Trusted unless allowlisted).
        let endpoint = spawn_mock_sse(|step| {
            if step == 0 {
                concat!(
                    "data: {\"type\":\"tool_call\",\"call_id\":\"c1\",\"tool_name\":\"mcp__filesystem__read_text\",\"arguments\":{\"path\":\"hello.txt\"}}\n\n",
                    "data: {\"type\":\"tool_call\",\"call_id\":\"c2\",\"tool_name\":\"mcp__filesystem__write_text\",\"arguments\":{\"path\":\"blocked.txt\",\"content\":\"nope\"}}\n\n"
                )
                .to_string()
            } else {
                "data: {\"type\":\"text_delta\",\"text\":\"all done\"}\n\ndata: {\"type\":\"done\",\"finish_reason\":\"stop\"}\n\n"
                    .to_string()
            }
        })
        .await;

        session.set_proxy_config(Some(ProxyConfig {
            endpoint,
            bearer_token: "tok".into(),
        }));

        let (tx, mut rx) = mpsc::channel(32);
        session
            .run_turn("read and write via mcp", tx, CancellationToken::new())
            .await
            .unwrap();

        let mut read_ok = false;
        let mut write_denied = false;
        while let Some(ev) = rx.recv().await {
            if let AgentEvent::ToolCompleted {
                tool_name,
                ok,
                output,
                ..
            } = ev
            {
                if tool_name == "mcp__filesystem__read_text"
                    && ok
                    && output.contains("hello from mcp filesystem")
                {
                    read_ok = true;
                }
                if tool_name == "mcp__filesystem__write_text" && !ok && output.contains("denied") {
                    write_denied = true;
                }
            }
        }

        assert!(read_ok, "allowlisted MCP tool should succeed in Trusted mode");
        assert!(
            write_denied,
            "non-allowlisted MCP tool must NOT auto-approve in Trusted mode"
        );
        assert!(!tmp.path().join("blocked.txt").exists());

        // Verify restart + clean shutdown.
        manager
            .restart(Some("filesystem"), session.tool_registry_mut())
            .await
            .unwrap();
        assert!(manager.statuses()[0].running);

        manager.shutdown_all().await;
        assert!(!manager.statuses()[0].running);

        println!("mcp_stdio integration test: OK");
    });
}
