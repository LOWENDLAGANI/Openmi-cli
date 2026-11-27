# Openmi CLI — Manual QA Script (TUI & Workflow)

## 1. Launch & Layout (`80x24`, SSH, Resize)
1. Resize terminal to `80x24`.
2. Run `openmi` (or `openmi tui`).
3. Verify:
   - Header (`✦ Openmi · <model> · <folder>`, credits on the right), chat area, prompt box and footer hints (`/ commands · Shift+Tab mode · /folder project · Esc ×2 quit`) render without clipping.
   - In the default `trusted` mode no mode badge is shown; after `Shift+Tab` the header shows `Ask` / `Read-only`.
   - Shrink the terminal (below `60` columns the folder is hidden; below `10x6` "Terminal too small" is shown) and grow it back; verify clean redraw without panic.

## 2. Multiline Input & Prompt History
1. Type `line 1`, press `Shift+Enter` (or `Alt+Enter`), type `line 2`, press `Enter`.
2. Verify the multiline prompt submits and the assistant response streams in.
3. Press `Up` to recall the previous prompt; press `Down` to return to the draft.
4. Type `/` and verify the command palette appears; `Up`/`Down` select, `Tab` completes, `Enter` runs.

## 3. Approval Popup (`ask` Mode)
1. Switch to `ask` with `Shift+Tab` (or `/mode ask`, or turn off auto-approve in `/settings`).
2. Trigger `write_file` or `run_command`.
3. Verify the **Permission needed** popup shows the action, summary, a diff preview for writes, and `[y] Allow once`, `[a] Always allow this session`, `[n] Cancel`.
4. Test `n`, `y`, `a` (later calls of the same tool auto-approve this session), and `Up`/`Down` + `Enter`. `Esc` denies.

## 4. Cancel & Quit + Panic Recovery
1. While a turn (e.g. `sleep 30` via `run_command`) is running, press `Esc` (or `Ctrl-C`) once:
   - Verify the turn stops, the footer shows "Stopping... press Esc again to quit", and the child process is killed.
2. Press `Esc` again: verify the TUI exits and restores terminal input/echo/cursor.
3. When idle, verify a single `Esc` only arms quit ("Press Esc again to quit") and any other key disarms it.
4. Verify `PageUp`/`PageDown` and `Ctrl`/`Alt` + `Up`/`Down` scroll long transcripts.

## 5. Workflow Features
1. Test slash commands: `/settings`, `/folder`, `/model`, `/model <id>`, `/resume`, `/new`, `/init`, `/mode <mode>`, `/diff`, `/undo`, `/credits`, `/mcp`, `/help`, `/help keys`, `/version`, `/quit`.
2. `/mode full-access`: verify typed confirmation (`I UNDERSTAND FULL ACCESS`) is required; `Esc` cancels.
3. Type `@` + partial filename and press `Tab`: verify a workspace-relative path is inserted (respecting `.gitignore`).
4. Quit and run `openmi --resume`: verify the prior conversation is restored ("Resumed your previous chat.").
5. Run `/init` (or create `OPENMI.md` / `CLAUDE.md` / `AGENTS.md`) and verify project instructions are loaded.
