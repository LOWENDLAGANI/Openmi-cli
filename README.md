# Openmi CLI

`openmi` is an interactive `ratatui` TUI and headless coding agent for **Windows**, **Linux**, **macOS**, and **Termux (Android `aarch64`)**. It uses **rustls-only** TLS (no OpenSSL dependency), strict workspace `PathGuard` sandboxing, exact-byte checkpoints, and managed Supabase auth & credits.

## Installation

### Linux, macOS, and Termux (Android `aarch64`)
```sh
curl -fsSL https://raw.githubusercontent.com/LOWENDLAGANI/Openmi-cli/main/install.sh | sh
```

### Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/LOWENDLAGANI/Openmi-cli/main/install.ps1 | iex
```

### Build from source
```sh
cargo build -p openmi-cli --release
./target/release/openmi smoke-test
```

See [docs/TERMUX_BUILD.md](docs/TERMUX_BUILD.md) for Termux-specific build instructions and [docs/CLI_QA.md](docs/CLI_QA.md) for the manual TUI QA checklist.

## Quickstart

```sh
# 1. Authenticate via device-code pairing (works on headless boxes & SSH)
openmi login
openmi whoami

# 2. Launch the interactive TUI
openmi

# 3. Resume your previous session
openmi --resume

# 4. Run headlessly in scripts / CI
openmi run "explain src/main.rs" --permission read-only
openmi run "fix compiler warnings" --json --yes

# 5. Inspect models, credits, or self-update
openmi models
openmi credits
openmi update
```

## Interactive TUI Features

- **Layout**: Scrollable transcript, live tool timeline, multiline prompt (`Enter` send, `Shift+Enter` / `Alt+Enter` newline, `↑`/`↓` prompt history), and status bar (`model | mode | credits | cwd | git`).
- **Approval Popup**: In `Ask` mode, mutating tools prompt with `[y]` approve once, `[a]` always this session, or `[n]` deny.
- **Cancel & Quit**: First `Esc` or `Ctrl-C` cancels an active turn (killing child process trees cleanly); second press exits and restores the terminal.
- **Slash Commands**:
  - `/model [id]` — view or switch the active model
  - `/mode [read-only|ask|trusted|full-access]` — view or change permission mode (`full-access` requires typing `I UNDERSTAND FULL ACCESS`)
  - `/diff` — render colored unified diff from the latest checkpoint
  - `/undo [file|run]` — restore exact pre-write bytes (including UTF-8 BOM and CRLF)
  - `/clear` — clear session history
  - `/credits` — refresh credit balance
  - `/mcp [status|restart [server]]` — inspect or restart configured stdio MCP servers
  - `/help` — show command summary
  - `/quit` — exit the TUI
- **`@path` Completion**: Type `@` followed by a partial filename and press `Tab` for `.gitignore`-aware fuzzy completion.
- **Project Instructions**: Place an `OPENMI.md` file in your workspace root to inject project guidelines into the system prompt automatically.
- **Session Persistence**: Sessions are saved automatically under `~/.local/share/openmi/sessions` and restored with `openmi --resume [SESSION_ID]`.

## MCP Support (`v1.0`)

Configure stdio MCP servers globally in `~/.config/openmi/mcp.json` or per-project in `.openmi/mcp.json`:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
      "env": {},
      "allowlist": ["read_file"]
    }
  }
}
```

- Discovered tools are registered dynamically as `mcp__<server>__<tool>`.
- All MCP tools are classified at least `Ask` risk and are **never auto-approved in `Trusted` mode** unless explicitly listed in the server's `allowlist`.
- Inspect servers and tools from the CLI with `openmi mcp list [--json]` or inside the TUI with `/mcp status` and `/mcp restart [server]`.
- Server processes are spawned in managed process groups and terminated cleanly on exit.

## Exit Codes

| Code | Meaning |
|---:|---|
| `0` | Success |
| `1` | General error |
| `2` | CLI usage / invalid arguments |
| `3` | Authentication missing, expired, or revoked |
| `4` | Out of credits (`HTTP 402`) |
| `5` | Agent loop-cap exceeded |
| `130` | Cancelled (`Ctrl-C` / `SIGINT`) |

## Architecture

- `crates/openmi-platform` — cross-platform paths (`AppPaths`), `PathGuard` (symlink / `..` / Unix `O_NOFOLLOW` TOCTOU protection), atomic writes (`0600` private mode), process-group tree kill.
- `crates/openmi-checkpoints` — exact-byte pre-write snapshots and per-file/per-run rollback.
- `crates/openmi-core` — UI-agnostic `AgentSession`, `AgentEvent` stream, `PermissionMode`, SSE proxy client, built-in tools, model/credit catalog.
- `crates/openmi-cli` — `openmi` binary (`login`, `logout`, `whoami`, `models`, `credits`, `run`, `tui`, `update`, `smoke-test`).
- `supabase/functions/cli-auth/` & `supabase/migrations/` — device-code pairing Edge Function and `device_codes` schema.