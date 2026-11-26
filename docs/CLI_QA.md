# Openmi CLI — Manual QA Script (TUI & Workflow)

Run through this checklist on Linux (including `tmux`), macOS, Windows Terminal, Termux (Android), and over SSH at `80x24`.

## 1. Launch & Layout (`80x24`, SSH, Resize)
1. Resize terminal to `80x24`.
2. Run `openmi` (or `openmi tui`).
3. Verify:
   - Transcript pane, Tool timeline pane, multiline Prompt box, and bottom Status bar (`model | mode | credits | cwd | git`) render with zero flicker and no clipping.
   - Resize terminal down to `40x12` and back up to `120x40`; verify clean redraw without panic.

## 2. Multiline Input & Prompt History
1. Type `line 1`, press `Shift+Enter` (or `Alt+Enter`), type `line 2`, press `Enter`.
2. Verify the multiline prompt submits cleanly and streams the assistant response.
3. Press `Up` arrow to recall the previous prompt; press `Down` arrow to return to the blank draft.

## 3. Approval Popup (`Ask` Mode)
1. Trigger a mutating tool call (`write_file` or `run_command`) while in `Ask` mode.
2. Verify the centered yellow **Approval Required** modal appears with `[y] Approve once`, `[a] Always this session`, `[n] Deny`.
3. Test `n` (denies tool cleanly), `y` (approves once), and `a` (subsequent calls to the same tool auto-approve for the rest of the session).

## 4. Cancel & Quit + Panic Recovery
1. While a turn or long command (`sleep 30`) is running, press `Esc` (or `Ctrl-C`) once:
   - Verify the active turn cancels and child processes terminate without exiting the TUI.
2. Press `Esc` (or `Ctrl-C`) again:
   - Verify the TUI exits cleanly and restores normal terminal input/echo/cursor.
3. Verify `PageUp` / `PageDown` scroll long transcripts cleanly.

## 5. Workflow Features (M4)
1. Test slash commands: `/help`, `/model <id>`, `/mode <mode>`, `/diff`, `/undo`, `/clear`, `/credits`, `/quit`.
2. Test `/mode full-access`: verify typed confirmation (`I UNDERSTAND FULL ACCESS`) is required before switching.
3. Type `@` followed by partial filename and press `Tab`: verify fuzzy `@path` completion inserts a workspace-relative path (respecting `.gitignore`).
4. Quit and run `openmi --resume`: verify prior conversation history is restored.
5. Create `OPENMI.md` in workspace root and restart `openmi`: verify project instructions are loaded into system context.
