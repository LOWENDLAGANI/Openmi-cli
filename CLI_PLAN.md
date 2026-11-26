# Openmi CLI — Final Plan (planner → builder handoff)

Roles: **Planner** writes/updates this spec and reviews. **Builder** implements one milestone at a time, never deviates without raising a question in `docs/CLI_QUESTIONS.md`.

## Goal
`openmi`: interactive ratatui TUI coding agent + headless `openmi run`, on Windows/Linux/macOS/Termux. Reuses `openmi-core`, `openmi-checkpoints`, `openmi-platform` unchanged where possible. Keeps managed Supabase proxy + credits (no BYOK).

## Ground rules for the builder
1. Core stays UI-agnostic. UI only touches `AgentEvent` (stream), `ApprovalHandler`, `AgentSession` setters, `CancellationToken`.
2. Don't weaken Path Guard / permission modes. Any change in `openmi-platform` paths needs tests.
3. `cargo test --workspace`, `cargo clippy -- -D warnings`, `cargo fmt --check` green before each milestone is declared done.
4. rustls only (no OpenSSL) so Termux builds work.
5. Small PRs, one per milestone, branch `cli/mN-<name>`.
6. Existing `smoke-test` subcommand must keep passing.

## Facts from audit
- `openmi-platform` already has `cfg(not(windows))` fallbacks (paths, job_object, fs_atomic, pty). Needs verification, not rewrite.
- `AgentEvent`: StepStarted, TextDelta, ToolCallRequested, ApprovalRequired, ToolStarted, ToolCompleted, ControlStateChanged, TurnCompleted, Error.
- `PermissionMode`: ReadOnly, Ask, Trusted, FullAccess. `AgentSession` has `run_turn`, `set_model`, `set_permission_mode`, `clear_history`, `set_before_write_hook`.
- `catalog.rs`: `fetch_models_with_fallback`, `fetch_credit_status`, model cache.
- Existing `openmi-cli` has clap, `smoke-test`, `run`, stdin approval handler. Supabase JWT is currently supplied manually.

## Architecture
```
crates/openmi-cli (bin: openmi)
  src/main.rs          clap dispatch
  src/cmd/{login,logout,whoami,models,run,tui,smoke}.rs
  src/auth/{device_flow,store}.rs     token store: keyring, 0600 file fallback
  src/session.rs       builds AgentSession + checkpoint hook (shared by run & tui)
  src/tui/{app,input,render,approval,diff,slash}.rs
  src/output/{text,jsonl}.rs          headless renderers
supabase/functions/cli-auth/          device-code pairing (new)
```

## Milestones (each ends with acceptance criteria)

### M0 — Portability baseline
- Run workspace tests on Linux; fix failures. Add Linux+macOS to CI (`.github/workflows/ci.yml`). Verify `cargo build -p openmi-cli` on Termux (aarch64).
- **Accept:** CI green on win/linux/mac; Termux build documented.

### M1 — Auth
- Edge function `cli-auth`: `POST /start` → {device_code, user_code, verify_url}; `POST /poll`; web page/approval path completes with logged-in user. Table `device_codes` (TTL 10 min, single-use, rate-limited).
- CLI: `login` (prints code, polls, stores refresh+access token), `logout`, `whoami`, auto-refresh.
- **Accept:** login on a headless box; token file perms 0600; expired token refreshes; revoked token gives clear error.

### M2 — Headless `run` + models
- `openmi run "<prompt>" [--model] [--permission] [--cwd] [--json]`; `openmi models`; `openmi credits`.
- Text renderer (stream deltas, tool lines) and JSONL renderer (one `AgentEvent` per line, stable schema, versioned).
- Exit codes: 0 ok, 1 error, 2 usage, 3 auth, 4 out-of-credits (402), 5 loop-cap, 130 cancelled.
- Non-interactive approvals: Ask mode with no TTY → deny and say so (flag `--yes` for approve).
- **Accept:** snapshot tests with the mock SSE LLM; Ctrl-C cancels cleanly and kills child processes.

### M3 — TUI MVP
- ratatui+crossterm; event loop `select!` over terminal events, `AgentEvent` channel, tick.
- Layout: transcript (scroll), tool timeline, multiline input (Enter send, Shift/Alt+Enter newline, ↑ history), status bar (model, mode, credits, cwd, branch).
- Approval popup (y/n/a=always-this-session) via oneshot-channel `TuiApprovalHandler`.
- Esc/Ctrl-C: first cancels turn, second quits. Terminal restored on panic.
- **Accept:** works at 80x24 and over SSH; no flicker; resize safe; manual QA script in `docs/CLI_QA.md`.

### M4 — Workflow features
- (Session resume, OPENMI.md and `openmi update` are v1 scope, built in M4/M5 as listed.)
- Slash commands: `/model`, `/mode`, `/undo [file|run]`, `/diff`, `/clear`, `/credits`, `/help`, `/quit`.
- Checkpoints: create before each run via `BeforeWriteHook`; `/diff` renders unified diff with colors; undo per file/run.
- FullAccess requires typed confirmation (port of `FullAccessModal` wording).
- `@path` mention with fuzzy completion (`ignore` crate). Session save/resume (`~/.local/share/openmi/sessions`, `--resume`).
- Project instructions file (`OPENMI.md`) read into system context.
- **Accept:** undo restores bytes exactly incl. BOM/CRLF; tests for each slash command.

### M5 — Packaging
- cargo-dist (or Actions matrix): win-x64, linux x64/arm64, mac x64/arm64, android aarch64 (Termux). Checksums.
- `install.sh`, `install.ps1`; `openmi update` (checks release JSON, replaces binary).
- README + man-style `--help`.
- **Accept:** fresh-machine install on each OS runs `openmi login` → `run`.

## Decisions (confirmed by user)
- Auth: device code + web approval page. Binary: `openmi`. Plan lives on `main`.
- Releases: **v0.1 = M0–M2 headless** (ship first); **v0.2 = M3–M5 TUI + workflow + packaging**; **v1.0 = M6 MCP**.
- Prebuilt aarch64 Termux/Android binary is a REQUIRED release artifact (M5, and also attached to v0.1 via a minimal release job in M2).
- v1 extras chosen: MCP, session resume, OPENMI.md, `openmi update`. BYOK excluded.

### M6 — MCP support (v1.0)
- `~/.config/openmi/mcp.json` + project `.openmi/mcp.json`: stdio servers (command, args, env); HTTP/SSE optional later.
- Spawn servers under the platform process-group/job handling; list tools via MCP `tools/list`; expose as `mcp__<server>__<tool>` in `ToolRegistry` (needs dynamic tool registration in `openmi-core` — design note first in `docs/CLI_QUESTIONS.md`).
- All MCP calls classified at least `Ask` risk; never auto-approved in Trusted unless allowlisted in config.
- `/mcp` slash command (status, restart); `openmi mcp list`.
- **Accept:** works with a reference filesystem MCP server; killed cleanly on exit; tests with a mock stdio server.

## Out of scope
BYOK, plugins, image input, multi-agent, GUI-parity changes pane.

## Risks
| Risk | Mitigation |
|---|---|
| Supabase has no device flow | M1 custom pairing function; fallback paste-token |
| Termux PTY/TLS quirks | rustls, early M0 check |
| Unix Path Guard weaker than Windows | symlink/`..`/TOCTOU tests in M0 |
| TUI terminal compatibility | test tmux, Termux, Windows Terminal |

## Review protocol
Builder posts milestone summary + test output; planner reviews against acceptance criteria and either approves or lists fixes.
