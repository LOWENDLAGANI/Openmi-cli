# Openmi CLI — Design Notes & Open Questions

## M6: Dynamic Tool Registration in `openmi-core` for MCP Support

### Context
`openmi-core` originally dispatched only static built-in tools (`read_file`, `list_dir`, `write_file`, `run_command`). M6 requires exposing tools discovered at runtime from external stdio MCP servers (`~/.config/openmi/mcp.json` and `<workspace>/.openmi/mcp.json`) as `mcp__<server>__<tool>` while keeping `openmi-core` completely UI-agnostic.

### Design
1. **`DynamicTool` trait & `ToolRegistry` (`crates/openmi-core/src/registry.rs`)**:
   - `DynamicTool` exposes `async fn call(&self, arguments_json: &str, cancel: &CancellationToken) -> Result<String, CoreError>`.
   - `ToolRegistry` stores `RegisteredTool` entries (`spec: DynamicToolSpec`, `handler: Arc<dyn DynamicTool>`) keyed by normalized tool name `mcp__<server>__<tool>`.
   - `AgentSession` owns a `ToolRegistry` (accessible via `tool_registry()` / `tool_registry_mut()`) and consults it during `run_proxy_loop` whenever a tool call is not one of the four static built-ins.
2. **Permission & Risk Policy**:
   - Every MCP tool is classified as at least `Ask` risk.
   - In `PermissionMode::ReadOnly`: MCP tool calls are denied.
   - In `PermissionMode::Ask`: MCP tool calls always trigger `ApprovalRequired` and invoke the UI-provided `ApprovalHandler`.
   - In `PermissionMode::Trusted`: MCP tool calls are **never auto-approved** unless explicitly listed in the server's `allowlist` in `mcp.json` (or `"*"` for that server).
   - In `PermissionMode::FullAccess`: MCP tool calls execute without prompting.
3. **Process Lifecycle (`crates/openmi-core/src/mcp.rs`)**:
   - Stdio MCP servers are spawned via `openmi_platform::job_object::configure_process_group`.
   - `McpManager::shutdown_all()` terminates all server child process trees via `kill_process_tree` on CLI/TUI exit, and `McpManager::restart()` cleanly stops and respawns a server and refreshes its entries in `ToolRegistry`.
