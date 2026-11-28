use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use openmi_platform::job_object::{configure_process_group, kill_process_tree};
use openmi_platform::AppPaths;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::registry::{format_mcp_tool_name, DynamicTool, DynamicToolSpec, ToolRegistry};
use crate::session::CoreError;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("MCP server `{server}` protocol error: {message}")]
    Protocol { server: String, message: String },

    #[error("MCP server `{0}` not found")]
    UnknownServer(String),
}

/// Configuration for a single stdio MCP server in `mcp.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Optional list of remote tool names (or `"*"`) allowed to auto-approve in `Trusted` mode.
    #[serde(default)]
    pub allowlist: Vec<String>,
}

/// Root schema for `~/.config/openmi/mcp.json` and `<workspace>/.openmi/mcp.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigFile {
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

/// Tool metadata returned by MCP `tools/list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// Runtime status summary for a configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub command: String,
    pub running: bool,
    pub tools: Vec<String>,
    pub error: Option<String>,
}

/// Load and merge `~/.config/openmi/mcp.json` (global) and optionally `<workspace>/.openmi/mcp.json` (project).
/// Project definitions override global servers with the same name when `include_project` is `true`.
pub fn load_merged_mcp_config_with_trust(
    global_path: Option<&Path>,
    workspace_root: &Path,
    include_project: bool,
) -> Result<McpConfigFile, McpError> {
    let mut merged = McpConfigFile::default();

    let resolved_global = match global_path {
        Some(p) => Some(p.to_path_buf()),
        None => AppPaths::resolve().ok().map(|p| p.mcp_config_file()),
    };

    if let Some(path) = resolved_global {
        if path.exists() {
            let raw = fs::read(&path)?;
            let parsed: McpConfigFile = serde_json::from_slice(&raw)?;
            for (k, v) in parsed.mcp_servers {
                merged.mcp_servers.insert(k, v);
            }
        }
    }

    if include_project {
        let project_path = workspace_root.join(".openmi").join("mcp.json");
        if project_path.exists() {
            let raw = fs::read(&project_path)?;
            let parsed: McpConfigFile = serde_json::from_slice(&raw)?;
            for (k, v) in parsed.mcp_servers {
                merged.mcp_servers.insert(k, v);
            }
        }
    }

    Ok(merged)
}

/// Load and merge `~/.config/openmi/mcp.json` (global) and `<workspace>/.openmi/mcp.json` (project).
/// Project definitions override global servers with the same name.
pub fn load_merged_mcp_config(
    global_path: Option<&Path>,
    workspace_root: &Path,
) -> Result<McpConfigFile, McpError> {
    load_merged_mcp_config_with_trust(global_path, workspace_root, true)
}

struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// JSON-RPC 2.0 client speaking newline-delimited MCP over stdio.
pub struct McpStdioClient {
    server_name: String,
    next_id: AtomicU64,
    transport: Mutex<StdioTransport>,
}

impl McpStdioClient {
    pub async fn spawn(
        server_name: impl Into<String>,
        cfg: &McpServerConfig,
        cwd: &Path,
    ) -> Result<Arc<Self>, McpError> {
        let server_name = server_name.into();
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear();
        const MCP_ENV_ALLOWLIST: &[&str] = &[
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "SHELL",
            "TERM",
            "LANG",
            "LC_ALL",
            "TMPDIR",
            "TEMP",
            "TMP",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "SYSTEMROOT",
            "COMSPEC",
            "PATHEXT",
            "APPDATA",
            "LOCALAPPDATA",
            "USERPROFILE",
            "PREFIX",
            "ANDROID_DATA",
            "ANDROID_ROOT",
        ];
        for key in MCP_ENV_ALLOWLIST {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        configure_process_group(&mut cmd);

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| McpError::Protocol {
            server: server_name.clone(),
            message: "failed to capture stdin".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| McpError::Protocol {
            server: server_name.clone(),
            message: "failed to capture stdout".into(),
        })?;

        let client = Arc::new(Self {
            server_name,
            next_id: AtomicU64::new(1),
            transport: Mutex::new(StdioTransport {
                child,
                stdin,
                stdout: BufReader::new(stdout),
            }),
        });

        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<(), McpError> {
        let _ = self
            .request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "openmi-cli", "version": env!("CARGO_PKG_VERSION") }
                }),
                &CancellationToken::new(),
            )
            .await?;

        self.notify("notifications/initialized", serde_json::json!({}))
            .await?;
        Ok(())
    }

    async fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), McpError> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');
        let mut guard = self.transport.lock().await;
        guard.stdin.write_all(line.as_bytes()).await?;
        guard.stdin.flush().await?;
        Ok(())
    }

    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, McpError> {
        const MCP_REQUEST_TIMEOUT_SECS: u64 = 60;
        const MCP_MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');

        let mut guard = self.transport.lock().await;
        guard.stdin.write_all(line.as_bytes()).await?;
        guard.stdin.flush().await?;

        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS);
        loop {
            let mut resp_line = String::new();
            let read_res = tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(McpError::Protocol {
                        server: self.server_name.clone(),
                        message: "cancelled".into(),
                    });
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(McpError::Protocol {
                        server: self.server_name.clone(),
                        message: format!("request `{method}` timed out after {MCP_REQUEST_TIMEOUT_SECS}s"),
                    });
                }
                res = guard.stdout.read_line(&mut resp_line) => res?,
            };
            if read_res == 0 {
                return Err(McpError::Protocol {
                    server: self.server_name.clone(),
                    message: "server closed stdout unexpectedly".into(),
                });
            }
            if resp_line.len() > MCP_MAX_LINE_BYTES {
                return Err(McpError::Protocol {
                    server: self.server_name.clone(),
                    message: "server response exceeded 4 MiB line limit".into(),
                });
            }
            let trimmed = resp_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let val: serde_json::Value = serde_json::from_str(trimmed)?;
            // Ignore server-initiated notifications without matching id.
            if val.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = val.get("error") {
                return Err(McpError::Protocol {
                    server: self.server_name.clone(),
                    message: err.to_string(),
                });
            }
            return Ok(val
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
        let result = self
            .request(
                "tools/list",
                serde_json::json!({}),
                &CancellationToken::new(),
            )
            .await?;
        let tools_val = result
            .get("tools")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
        Ok(serde_json::from_value(tools_val)?)
    }

    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<String, McpError> {
        let result = self
            .request(
                "tools/call",
                serde_json::json!({
                    "name": tool_name,
                    "arguments": arguments,
                }),
                cancel,
            )
            .await?;

        if let Some(items) = result.get("content").and_then(|c| c.as_array()) {
            let texts: Vec<String> = items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                })
                .collect();
            if !texts.is_empty() {
                return Ok(texts.join("\n"));
            }
        }
        Ok(result.to_string())
    }

    pub async fn shutdown(&self) {
        let mut guard = self.transport.lock().await;
        let _ = kill_process_tree(&mut guard.child).await;
    }
}

struct McpDynamicToolAdapter {
    client: Arc<McpStdioClient>,
    remote_name: String,
}

#[async_trait]
impl DynamicTool for McpDynamicToolAdapter {
    async fn call(
        &self,
        arguments_json: &str,
        cancel: &CancellationToken,
    ) -> Result<String, CoreError> {
        let args: serde_json::Value = serde_json::from_str(arguments_json)
            .map_err(|e| CoreError::Tool(format!("invalid MCP arguments JSON: {e}")))?;
        self.client
            .call_tool(&self.remote_name, args, cancel)
            .await
            .map_err(|e| {
                if cancel.is_cancelled() {
                    CoreError::Cancelled
                } else {
                    CoreError::Tool(e.to_string())
                }
            })
    }
}

struct RunningServer {
    config: McpServerConfig,
    client: Option<Arc<McpStdioClient>>,
    tools: Vec<String>,
    error: Option<String>,
}

/// Manages lifecycle of all configured stdio MCP servers and binds their tools into `ToolRegistry`.
pub struct McpManager {
    cwd: std::path::PathBuf,
    servers: BTreeMap<String, RunningServer>,
}

impl McpManager {
    pub fn from_config(cwd: impl Into<std::path::PathBuf>, config: McpConfigFile) -> Self {
        let servers = config
            .mcp_servers
            .into_iter()
            .map(|(name, cfg)| {
                (
                    name,
                    RunningServer {
                        config: cfg,
                        client: None,
                        tools: Vec::new(),
                        error: None,
                    },
                )
            })
            .collect();
        Self {
            cwd: cwd.into(),
            servers,
        }
    }

    /// Start all configured servers and register their tools into `registry`.
    /// Individual server failures are captured in status so one broken server doesn't block startup.
    pub async fn start_all(&mut self, registry: &mut ToolRegistry) {
        let names: Vec<String> = self.servers.keys().cloned().collect();
        for name in names {
            let _ = self.start_one(&name, registry).await;
        }
    }

    async fn start_one(&mut self, name: &str, registry: &mut ToolRegistry) -> Result<(), McpError> {
        let cfg = self
            .servers
            .get(name)
            .ok_or_else(|| McpError::UnknownServer(name.to_string()))?
            .config
            .clone();

        registry.unregister_server(name);

        match McpStdioClient::spawn(name, &cfg, &self.cwd).await {
            Ok(client) => match client.list_tools().await {
                Ok(tool_infos) => {
                    let mut registered_names = Vec::new();
                    for info in tool_infos {
                        let full_name = format_mcp_tool_name(name, &info.name);
                        let allowlisted = cfg
                            .allowlist
                            .iter()
                            .any(|a| a == "*" || a == &info.name || a == &full_name);

                        let spec = DynamicToolSpec {
                            name: full_name.clone(),
                            server: name.to_string(),
                            remote_name: info.name.clone(),
                            description: info.description,
                            input_schema: info.input_schema,
                            auto_approve_in_trusted: allowlisted,
                        };
                        let adapter = Arc::new(McpDynamicToolAdapter {
                            client: Arc::clone(&client),
                            remote_name: info.name,
                        });
                        registry.register(spec, adapter);
                        registered_names.push(full_name);
                    }
                    if let Some(entry) = self.servers.get_mut(name) {
                        entry.client = Some(client);
                        entry.tools = registered_names;
                        entry.error = None;
                    }
                    Ok(())
                }
                Err(err) => {
                    client.shutdown().await;
                    if let Some(entry) = self.servers.get_mut(name) {
                        entry.client = None;
                        entry.tools.clear();
                        entry.error = Some(err.to_string());
                    }
                    Err(err)
                }
            },
            Err(err) => {
                if let Some(entry) = self.servers.get_mut(name) {
                    entry.client = None;
                    entry.tools.clear();
                    entry.error = Some(err.to_string());
                }
                Err(err)
            }
        }
    }

    /// Restart a specific server (or all servers if `server` is `None`) and refresh `registry`.
    pub async fn restart(
        &mut self,
        server: Option<&str>,
        registry: &mut ToolRegistry,
    ) -> Result<(), McpError> {
        match server {
            Some(name) => {
                if let Some(entry) = self.servers.get_mut(name) {
                    if let Some(client) = entry.client.take() {
                        client.shutdown().await;
                    }
                } else {
                    return Err(McpError::UnknownServer(name.to_string()));
                }
                self.start_one(name, registry).await
            }
            None => {
                self.shutdown_all().await;
                self.start_all(registry).await;
                Ok(())
            }
        }
    }

    pub fn statuses(&self) -> Vec<McpServerStatus> {
        self.servers
            .iter()
            .map(|(name, s)| McpServerStatus {
                name: name.clone(),
                command: s.config.command.clone(),
                running: s.client.is_some(),
                tools: s.tools.clone(),
                error: s.error.clone(),
            })
            .collect()
    }

    pub async fn shutdown_all(&mut self) {
        for entry in self.servers.values_mut() {
            if let Some(client) = entry.client.take() {
                client.shutdown().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_global_and_project_mcp_configs_with_project_override() {
        let tmp = tempfile::tempdir().unwrap();
        let global_file = tmp.path().join("global_mcp.json");
        let project_dir = tmp.path().join("ws");
        fs::create_dir_all(project_dir.join(".openmi")).unwrap();

        fs::write(
            &global_file,
            r#"{
              "mcpServers": {
                "filesystem": { "command": "global-fs", "args": ["/"] },
                "git": { "command": "mcp-git" }
              }
            }"#,
        )
        .unwrap();

        fs::write(
            project_dir.join(".openmi/mcp.json"),
            r#"{
              "mcpServers": {
                "filesystem": { "command": "project-fs", "args": ["."], "allowlist": ["read_file"] }
              }
            }"#,
        )
        .unwrap();

        let merged = load_merged_mcp_config(Some(&global_file), &project_dir).unwrap();
        assert_eq!(merged.mcp_servers.len(), 2);
        assert_eq!(merged.mcp_servers["filesystem"].command, "project-fs");
        assert_eq!(
            merged.mcp_servers["filesystem"].allowlist,
            vec!["read_file"]
        );
        assert_eq!(merged.mcp_servers["git"].command, "mcp-git");
    }
}
