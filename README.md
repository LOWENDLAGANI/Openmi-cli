# Openmi CLI

`openmi` is an interactive `ratatui` TUI and headless coding agent for **Windows**, **Linux**, **macOS**, and **Termux (Android `aarch64`)**. It uses **rustls-only** TLS (no OpenSSL dependency), workspace `PathGuard` sandboxing, exact-byte checkpoints, and a managed Supabase backend for auth & credits (no bring-your-own-key).

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

### Automatic updates

On every launch (except `openmi update`) the CLI checks GitHub Releases with a 3-second timeout. If a newer version exists it downloads it, verifies the SHA-256 checksum, replaces the binary and re-runs your command. Offline or failed checks are silently skipped. Disable with `OPENMI_NO_AUTO_UPDATE=1`.

## Quickstart

```sh
# 1. Authenticate via device-code pairing (works on headless boxes & SSH)
openmi login
openmi whoami

# 2. Launch the interactive TUI
openmi

# 3. Resume your latest session for the current folder (or pass a session ID)
openmi --resume

# 4. Run headlessly in scripts / CI
openmi run "explain src/main.rs" --permission read-only
openmi run "fix compiler warnings" --json --yes

# 5. Inspect models, credits, or update manually
openmi models
openmi credits
openmi update --check
```

`openmi login --token <JWT> [--refresh-token <TOKEN>]` pastes a Supabase token directly instead of running device-code pairing.

### Global flags (TUI)

`openmi` and `openmi tui` accept `--model <id>` (default `openmi-default`), `--permission <mode>`, `--cwd <dir>`, `--api-url <url>` and `--resume [SESSION_ID]`.

### `openmi run` flags

| Flag | Meaning |
| --- | --- |
| `--model <id>` | Model (default `openmi-default`) |
| `--permission <mode>` | `read-only`, `ask`, `trusted`, `full-access` (default from settings, initially `trusted`) |
| `--cwd <dir>` | Workspace root (default: current directory) |
| `--json` | Stream versioned JSONL events (one `AgentEvent` per line) |
| `-y`, `--yes` | Auto-approve tool prompts in `ask` mode |
| `--max-steps <n>` | Tool-calling step cap (default `16`); exceeding it exits with code `5` |
| `--api-url <url>` | Override backend URL |

In `ask` mode without `--yes`: if stdin is a TTY you are prompted `[y/N/a]`; otherwise the tool is denied with a message on stderr.

### Backend configuration

By default the CLI talks to the production Openmi backend at
`https://umjlmqeghfrfpduhkits.supabase.co`. Resolution order for the API base URL:

1. `--api-url <URL>`
2. `OPENMI_API_URL`
3. The base URL saved with your credentials at `openmi login`
4. The built-in production URL

The Supabase publishable key (sent as the `apikey` header, required by `/auth/v1/*`) is built in
for the production project. Set `OPENMI_SUPABASE_ANON_KEY` to override it when pointing at
another Supabase project.

### Environment variables

| Variable | Effect |
| --- | --- |
| `OPENMI_API_URL` | Backend base URL (see above) |
| `OPENMI_SUPABASE_ANON_KEY` | Override the built-in Supabase publishable key |
| `OPENMI_CREDENTIALS_FILE` | Use a custom credentials file path |
| `OPENMI_NO_AUTO_UPDATE` | Any value except empty/`0`/`false` disables the startup update check |
| `OPENMI_RELEASE_URL` | Override the GitHub Releases JSON endpoint used for updates |
| `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` | Relocate config/data/cache dirs (Unix/Termux) |

## Permission Modes

| Mode | Behavior |
| --- | --- |
| `read-only` | Only `read_file`, `list_dir`, `grep`, and `glob`; writes and commands are blocked |
| `ask` | Asks before `edit_file`, `write_file`, `delete_file`, `move_file`, `run_command` and MCP tools |
| `trusted` | **Default.** Auto-approves structured file tools within PathGuard (`read_file`, `write_file`, `edit_file`, `delete_file`, `move_file`, `list_dir`, `glob`, `grep`). **Note:** `run_command` executes arbitrary shell commands without PathGuard confinement (it requires workspace trust / explicit approval first); MCP tools still ask unless allowlisted |
| `full-access` | No prompts; in the TUI requires typing `I UNDERSTAND FULL ACCESS` |

The default comes from `settings.json` (managed by `/settings`): `auto_approve: true` → `trusted`, `false` → `ask`, `read_only: true` → `read-only`. `--permission` overrides it for one launch.

## Interactive TUI Features

- **Layout**: a header line (`✦ Openmi · <model> · <folder>`, a mode/PLAN badge when active, and credits / active pass on the right), the chat transcript with inline tool activity lines, a multiline prompt box, and a footer with key hints. There is no separate tool pane.
- **Input**: `Enter` sends, `Shift+Enter` / `Alt+Enter` inserts a newline, `↑`/`↓` browse prompt history, `Tab` completes `/` commands or `@path` mentions.
- **Scrolling**: `PageUp`/`PageDown` (5 lines) or `Ctrl`/`Alt` + `↑`/`↓` (3 lines). Mouse capture is disabled so the Termux soft keyboard works.
- **Mode cycling**: `Shift+Tab` cycles `ask → trusted → read-only → ask` (`full-access` only via `/mode full-access`).
- **Approval popup**: shows the tool, a summary and, for file writes/edits (`write_file` / `edit_file`), a diff preview. Choose `[y]` allow once, `[a]` always allow this tool for the session, `[n]` cancel — or move with `↑`/`↓` (`j`/`k`) and press `Enter`.
- **Esc / Ctrl-C**: closes any open popup first (a pending approval is denied). While the agent is working, the first press stops the turn and a second press quits. When idle, press twice to quit.
- **Slash Commands** (type `/` to see a live palette):
  - `/settings` — toggle auto-approve, read-only, and inline tool activity (saved to `settings.json`)
  - `/folder [path]` — pick or switch the project folder (aliases `/project`, `/cd`)
  - `/model [id]` — open the model picker or switch directly
  - `/resume` — pick a previous chat to continue
  - `/new` (alias `/clear`) — start a fresh chat
  - `/init` — create a starter `OPENMI.md` guide tailored to the detected stack
  - `/plan [on|off]` — toggle read-only Plan Mode (explores & proposes a step-by-step plan before editing)
  - `/compact [focus]` — summarize conversation history to free context & save daily tokens
  - `/mode [read-only|ask|trusted|full-access]` — show or change permission mode
  - `/diff` — colored unified diff of the latest checkpointed run
  - `/undo [file|run_id]` — restore exact pre-write bytes (including UTF-8 BOM and CRLF)
  - `/credits` — refresh plan, daily + wallet credits, and pass status
  - `/upgrade` — open the `/pay` page in your browser
  - `/mcp [status|restart [server]]` — inspect or restart configured stdio MCP servers
  - `/help [steps|commands|modes|keys|mcp|cli]` — full guide or one topic
  - `/version` — show version and platform
  - `/quit` (alias `/exit`) — exit the TUI
- **`@path` Completion**: type `@` and part of a filename, then `Tab`, for `.gitignore`-aware fuzzy completion.
- **Project Instructions**: the first non-empty file of `OPENMI.md`, `CLAUDE.md`, `AGENTS.md` in the workspace root is added to the system prompt.
- **Session Persistence**: chats are saved automatically to the sessions directory and restored with `/resume` or `openmi --resume [SESSION_ID]` (without an ID, the latest session for the current folder is preferred).

## Files

| Purpose | Linux / macOS / Termux | Windows |
| --- | --- | --- |
| Credentials (`0600` on Unix) | `~/.config/openmi/credentials.json` | `%APPDATA%\openmi\credentials.json` |
| User settings | `~/.config/openmi/settings.json` | `%APPDATA%\openmi\settings.json` |
| Global MCP config | `~/.config/openmi/mcp.json` | `%APPDATA%\openmi\mcp.json` |
| Saved sessions | `~/.local/share/openmi/sessions/` | `%LOCALAPPDATA%\openmi\data\sessions\` |
| Model catalog cache | `~/.cache/openmi/models.json` | `%LOCALAPPDATA%\openmi\cache\models.json` |
| Project MCP config | `<workspace>/.openmi/mcp.json` | same |

## MCP Support

Configure stdio MCP servers globally (see Files above) or per-project in `.openmi/mcp.json`:

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
- MCP tools are denied in `read-only`, always ask in `ask`, ask in `trusted` unless listed in the server's `allowlist` (`"*"` allows all), and run without prompts in `full-access`.
- Inspect servers and tools with `openmi mcp list [--cwd <dir>] [--json]` or inside the TUI with `/mcp status` and `/mcp restart [server]`.
- Server processes are spawned in managed process groups and terminated on exit.

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

## Repository Layout

- `crates/openmi-platform` — cross-platform paths (`AppPaths`), `PathGuard` (symlink / `..` / Unix `O_NOFOLLOW` TOCTOU protection), atomic writes (`0600` private mode), process-group tree kill.
- `crates/openmi-checkpoints` — exact-byte pre-write snapshots and per-file/per-run rollback.
- `crates/openmi-core` — UI-agnostic `AgentSession`, `AgentEvent` stream, `PermissionMode`, SSE proxy client, built-in tools, MCP client & dynamic `ToolRegistry`, model/credit catalog.
- `crates/openmi-cli` — `openmi` binary (`login`, `logout`, `whoami`, `models`, `credits`, `mcp list`, `run`, `tui`, `update`, `smoke-test`).
- `supabase/` — Edge Functions (`cli-auth`, `models`, `credits`, `chat`, `payments`) and SQL migrations. See [docs/SUPABASE_DEPLOY.md](docs/SUPABASE_DEPLOY.md).
- `auth-web/` — device-login approval and payment web pages; `admin-panel/` — admin UI for backend settings.