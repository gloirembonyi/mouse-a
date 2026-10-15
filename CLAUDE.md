# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Mouse AI Desktop - a Windows desktop AI assistant built with Tauri 2 (Rust backend + React/TypeScript
frontend). It talks to an external **Thothai backend** (`../backend-Thothai`, a sibling repo, not part
of this codebase) which holds the Gemini API key(s) and does all model inference. This repo only ever
runs *tools* locally (shell commands, file I/O, mouse/keyboard control, screen capture) - the model
itself never runs here.

Four windows are built from `frontend/`: the main chat/IDE window (`index.html`, React), a small
always-on-top hover overlay (`overlay.html`, plain JS, see `frontend/public/overlay.js`) that pops up
near the cursor, the "Mouse AI is working" banner (`shield.html`, plain JS, shown by `shield.rs` for
the length of every local agent run, with Stop), and the in-call copilot bar (`copilot.html`, React, `frontend/src/copilot/`) - a
floating Cluely-style pill + glass panel (Assist / What should I say? / Follow-ups / Recap, live
insights, auto-answer, Solve screen) that `meeting_start` shows over the call. Ctrl+Shift+\ toggles it; while a
call is live, Ctrl+Shift+Enter (suggest) and Ctrl+Shift+S (solve what's on screen) are routed to it instead of the
main window. Ctrl+Alt+Arrows nudge the bar - registered only while it is visible (`set_move_keys` in `meeting.rs`). All Mouse AI windows (main, overlay,
copilot, shield) are excluded from screen capture by default (`WDA_EXCLUDEFROMCAPTURE` via
`set_content_protected`). The switch is `prefs.hide_from_screen_share` (persisted, default on, applied by
`prefs::apply` through `meeting::set_stealth`); `stealth_set` in `meeting.rs` writes it and emits
`stealth-changed`, and the same toggle appears in Settings (DesktopSection) and the copilot bar's menu. The
browser extension's Undercover can open here instead of in a Chrome popup: the `open_copilot` bridge frame
(paired only, `bridge.rs`) shows the copilot bar and emits `copilot-context {url,title,text,selection}` to
it; the bar shows the page as a chip and sends it along with the next question.

## Commands

Run from the repo root unless noted.

```bash
npm install                    # installs the root tauri CLI only; frontend deps install separately
npm run dev                    # = tauri dev - builds Rust, launches Vite dev server, opens the app
npm run build                  # = tauri build - produces installer in src-tauri/target/release/bundle/
```

Frontend-only (from `frontend/`):

```bash
npm run dev                    # vite dev server on :1420 (used by tauri dev via devUrl)
npm run build                  # tsc -b && vite build -> frontend/dist (entry points: main, overlay, copilot, shield)
npm run lint                   # oxlint
```

Rust-only (from `src-tauri/`): standard `cargo build` / `cargo check` work directly.

There are no automated tests in this repo currently.

## Cross-platform rules

The app ships for Windows, macOS and Linux (`.github/workflows/build.yml` builds all three on every push;
a `v*` tag publishes a draft release). Keep it that way:

- Anything Win32 lives behind `#[cfg(windows)]` with a `#[cfg(not(windows))]` counterpart in the same
  file that does the closest honest thing - never a silent no-op that makes a tool "succeed". Today:
  `input.rs` (SendInput vs. `enigo`), `mouse.rs` (UI Automation vs. `osascript`/`xdotool` frontmost
  window), `agent/exec/shell.rs` (PowerShell vs. `bash -c` in its own process group so `kill_tree`
  works), `terminal.rs` (`powershell.exe` vs. `$SHELL -l`), `shield.rs` (GetLastInputInfo vs. "never
  busy"), `uia.rs`/`explorer.rs`/`copilot.rs` (stubs).
- `winapi`/`windows` are `[target.'cfg(windows)'.dependencies]`; `enigo` is the non-Windows one.
  Clipboard is `arboard` everywhere (no PowerShell round trip).
- The prompts never name a shell: `agent/prompt.rs` `environment_block()` adds `OS:` and the
  `shell_rules()` line for the running OS, and the backend's desktop profile tells the model to
  follow `[ENVIRONMENT]`. Tests that drive real PowerShell are `#[cfg(all(test, windows))]`.
- A macOS/Linux cross-check cannot run from a Windows machine (dependency build scripts need the
  target's C compiler); CI is the verification for those targets.

## Architecture

### Split of responsibility: backend does the thinking, this app does the doing

The Gemini model runs entirely on the Thothai backend via HTTP (`/api/agent/step`, `/api/chat`, etc. -
see `src-tauri/src/backend.rs`, which mirrors the browser extension's `shared/backend-client.js`).
This app sends conversation state + tool results and gets back either plain text or a list of
`functionCalls` to execute. There is no Gemini key anywhere in this codebase - the backend rotates
across up to 5 shared keys server-side.

The agent loop lives in `src-tauri/src/agent/` (one file per concern - see `agent/mod.rs` for the
map: `drive.rs` is the loop, `exec/` the tools, `tools/` the declarations and per-task selection,
`ledger.rs`/`checkpoint.rs` the run's memory, `retry.rs` the model-call resilience; every file starts
with `use crate::agent::*` and `mod.rs` re-exports each child `pub(crate)`). Ask the model → if it
returned functionCalls, execute them locally (`run_command`, `read_file`, `write_file`, `find_files`,
`web_search`, `web_fetch`, `capture_screen`, `get_screen_context`, `click`/`double_click`/`scroll`/
`type_text`/`press_key`, `open_url`/`open_app`, `wait`, plus MCP-provided tools) → feed results back →
repeat until a plain-text answer; the step budget starts at 40 and grows while the run makes
progress (up to 120 steps / 30 min). Key details baked into these files:

- **Screen coordinate mapping**: the model only ever sees a downscaled screenshot; click coordinates
  it returns are normalized 0-1000 and must be mapped back to real screen pixels via `ScreenMapping`
  before dispatching a click. Skipping this scaling is the classic "clicks land on the wrong element"
  bug.
- **Confirmation gate**: the agent can call `request_confirmation` before a sensitive action, which
  blocks that tool call on a `oneshot` channel until the UI reports the user's Allow/Deny via the
  `agent_confirm` command (see `PENDING_CONFIRMATIONS` in `agent.rs`).
- **Agent scratch dir**: relative paths from the model resolve into a temp scratch directory
  (`resolve_agent_path`), wiped at the start of every `run_task`, *not* the process cwd - this is
  what stops model-written throwaway files from landing inside this repo's own source tree.
- Genuinely destructive shell commands (disk format, recursive system-drive delete, shutdown, …) are
  refused before ever reaching `run_command`.
- **The system prompt is served by the backend.** `retry.rs` sends `profile: "desktop"` plus a
  `promptContext` (`autonomous`, `toolNames`, `capabilities`, `environment` - built by
  `RunContext::prompt_context` in `drive.rs`); the backend renders
  `backend-Thothai/src/prompts/agent/desktop.js` and answers with `promptVersion`. `agent/prompt.rs`
  is the same prompt as a local fallback for an older backend. `capabilities` tells the server prompt
  which newer behaviours this build has (`structured_run_command`, `type_text_modes`) so it never
  describes a result shape the client does not produce - add to that list when you add one.
- **`run_command` returns a structured result**, not a string: `{ok, stdout, stderr, exit_code,
  timed_out, truncated, duration_ms}` (`exec/shell.rs`, `CommandRun`/`CommandResult`). A timeout is
  data (`timed_out: true`), not an error that loses the output; each stream is capped at 16 KiB
  (4 KiB head + 12 KiB tail with a marker) while the command runs, so a runaway command cannot eat
  memory. `tool_failed` reads `ok`; `response_text` reads stderr then stdout. The test-only
  `run_powershell` wrapper still returns the old `[exit code N]` string shape for the shell tests.
- **`type_text` has modes** (`exec/computer.rs`): `auto` pastes multi-line / long / code text via
  the clipboard (`input::paste_via_clipboard`, clipboard restored) so editors do not auto-indent or
  auto-close it, and types short text with batched keystrokes (`input::type_text_keys`: newline =
  Enter, tab = Tab, ~30x faster than the old per-unit loop); `type` and `paste` force one. Terminals
  always get keystrokes. Where the app exposes accessible text the result carries `verified`.
  `press_key` accepts sequences (`"ctrl+a, delete"`) and F-keys/space/page keys. All SendInput
  calls run on `spawn_blocking` (they used to sleep on the tokio runtime).
- **`open_url`/`open_app` never touch cmd.exe** (`exec/windows.rs`). The old `cmd /C start ""
  <target>` re-parsed the line, so a URL carrying `&calc` ran `calc`. Targets go through
  `tauri_plugin_opener` (ShellExecuteEx, no shell) after validation: http(s)/file/mailto URLs only,
  existing paths, or a bare program token with optional arguments.
- **Verification shots wait for the screen to settle** (`screen.rs::verification_shot`): first
  capture at 250 ms, then every 300 ms until two 48×32 thumbnails match, capped per tool (1.5 s for
  click/type/scroll/keys, 4 s for open/focus) - instead of a fixed 900 ms / 2 s.
- **"continue" resumes** (`checkpoint.rs`): every tool round saves the ledger to
  `%APPDATA%/mouse-ai/runs/<source>.json`; a continue-phrase (or the same task again) from the same
  source within 30 min seeds the new run with a `[RESUMING]` digest. The transcript itself is never
  persisted (replaying old parts with their `thoughtSignature`s 400s). `trusted_apps` is never
  persisted either.
- **Tools are selected per task** (`tools/select.rs`, a port of the extension's `selectTools`):
  a core set plus keyword hints (download, clipboard, notify, create_document, delegate_to_browser
  only when the extension is connected and paired); MCP tools and `use_skill` always. `drive_loop`
  widens to the full catalog once if the model names an un-offered tool (that call is executed
  anyway), on two consecutive errors, a stall, or a completion review.
- The wrap-up turn (`stop_with_report`) offers no tools, and a pending Allow/Deny prompt now
  honours Stop (`confirm.rs` polls `cancel_pending`).

### Live transcription (meeting_stream.rs) and element-level tools (uia.rs)

- **Meeting copilot latency.** `meeting_stream.rs` streams mic + system audio to Deepgram
  over a WebSocket (`tokio-tungstenite` with rustls) using a 2-minute token from the
  backend (`/api/meeting/stt-token`, 503 when `DEEPGRAM_API_KEY` is unset); interim results
  are `meeting-transcript-partial {speaker, text}` (the UI's dashed live line), finals go
  through `meeting::push_entry`. Both channels are sent interleaved (`multichannel=true`)
  so "me"/"them" is exact. While `LiveSession.streaming` is true the chunker only reports
  levels; if the stream drops it falls back to Whisper. The Whisper path itself is faster:
  1.2 s minimum chunk, 260 ms pause window, 5 s cap, up to `MAX_INFLIGHT` (3) chunks
  transcribed concurrently with results appended in cut order (`FLUSH_SEQ`/`FLUSH_PUBLISHED`).
  Assist skips the pre-flush when streaming and includes the sentence still being spoken.
- **UI Automation.** `uia.rs` (windows crate, `Win32_UI_Accessibility`) walks the
  foreground window's tree (`FindAll(TreeScope_Descendants)`, capped) and returns
  name/role/value/automation id/rect/patterns; acts through Invoke/Toggle/SelectionItem/
  ExpandCollapse/Value patterns; always re-finds by name+role near the last rect (COM
  pointers are never cached). Tools `find_elements` / `click_element` / `set_value`
  (`agent/exec/elements.rs`) prefer a real click on the element's own rectangle and fall
  back to patterns or click+select-all+paste. Verified live with
  `cargo test --lib uia -- --ignored --nocapture`. Capability flag `element_tools`.

### Rust module map (`src-tauri/src/`)

| Module | Responsibility |
| --- | --- |
| `lib.rs` | App entry: Tauri builder, system tray, global shortcuts, window setup, invoke_handler registration |
| `commands.rs` | Tauri `#[command]` IPC endpoints exposed to the frontend; owns `AppState` (backend client, ai_active, conversation_id, overlay_pinned) |
| `agent/` | The local agent loop (`drive.rs`), tools (`exec/*`, `tools/*`), ledger + checkpoints, prompt fallback, retry - see `agent/mod.rs` |
| `backend.rs` | HTTP client for the Thothai backend (auth, chat, agent step, conversation history) |
| `mouse.rs` | Cursor position/context, accessible-element inspection under the cursor (Win32 UI Automation) |
| `capture.rs` | Screenshot capture (full screen, region, window-or-around-point) |
| `hover.rs` | Global hover-dwell watcher - shows the overlay's "Ask AI" pill when the mouse pauses over something; `clamp_overlay_into_monitor` is called by `expand_overlay` after every resize so the panel (and its input row) never grows off-screen |
| `shield.rs` | The "working" banner window + the yield-to-user rule: `input.rs` stamps `OUR_LAST_INPUT_TICK` on every SendInput, `user_idle_ms()` compares it with `GetLastInputInfo`, and `drive.rs` calls `wait_for_user_idle` before any UI tool so the agent waits (and says so) while the person is using the mouse or keyboard instead of fighting them. BlockInput was rejected: it needs admin and locks the user out. While any run is active, Esc is a global hotkey that cancels every run (`request_cancel_all`, also behind the banner's Stop and `agent_cancel_all`); keystrokes within 400 ms of the agent's own SendInput are ignored so its own `press_key escape` cannot stop it |
| `input.rs` | Synthesized mouse/keyboard input for the agent's computer-use tools |
| `explorer.rs` | Windows Explorer / file-context helpers used by tools |
| `fs_browser.rs` | File-tree browsing + read/write for the IDE dock's file tree and editor |
| `terminal.rs` | PTY sessions keyed by id (one per terminal tab): spawn/write/resize/kill; output as `pty-output {id,data}` with UTF-8 split handling |
| `git_diff.rs` | Git status/diff for the IDE dock's changes pane |
| `mcp.rs` | MCP client: launches configured servers as stdio subprocesses, speaks JSON-RPC 2.0 (`initialize` → `tools/list` → `tools/call`). Strictly additive/fail-safe - a broken MCP server is skipped, never breaks the core agent loop |
| `bridge.rs` | Localhost WebSocket (127.0.0.1:47821) to the Thothai browser extension - the transport for agent handoffs |
| `link.rs` | Two-way agent link on top of the bridge: pairing code (`%APPDATA%/mouse-ai/bridge.json`), browser→desktop `desktop_task` runs (source `browser:<id>`), the shared task board (`bridge-task` events → Agents view) and the watcher that probes/fails stalled handoffs |
| `meeting.rs` | Meeting copilot: two-sided audio capture → Whisper chunks → transcript; assist actions (answers stream from the backend's `/api/agent/stream` as `meeting-answer-delta`, falling back to `/api/agent/step`); the copilot bar window, stealth, move keys |
| `copilot.rs` | The bar's hands-on tools: "Fix it" runs `agent::run_task` under source `copilot` with transcript + active project as context; `copilot_insert` pastes (clipboard restored) or naturally types text into the last non-Mouse-AI foreground window, tracked by a 250 ms watcher |
| `oauth.rs` | Google OAuth loopback flow (`http://127.0.0.1:8917/callback`) for Sign in with Google; a per-flow `state` must round-trip or the redirect is ignored |
| `auth_sync.rs` | One login across desktop + extension: over a *paired* bridge, `auth_request` / `auth_offer {code}` frames pass a one-time 60 s handoff code (`/api/auth/handoff` → `/api/auth/handoff/redeem`), never a session token. Only a client without a real (non-guest) account adopts an offer |

Session, backend URL, and MCP server config persist to `%APPDATA%/mouse-ai/config.json`.

### Frontend structure (`frontend/src/`)

- `lib/tauri.ts` - the single place that wraps every `invoke()`/`listen()` call to a Rust command.
  When a Rust command's name or shape changes, update it here rather than calling `invoke` directly
  from components.
- `hooks/` - `useAuth`, `useChat`, `useConversations`, `useWorkspace`, `useMeeting` hold the main window's
  stateful logic; `App.tsx` wires them together and switches between the chat, meeting and settings views.
  `useWorkspace` owns the user's **projects** (folders added via the native picker, `tauri-plugin-dialog`,
  persisted in localStorage); the active project drives the IDE dock root and is prefixed onto agent tasks.
- Theming: `index.css` defines dark + light palettes as `--mx-*` variables keyed off `<html data-theme>`, exposed to
  Tailwind via `@theme inline` - use the token utilities (`bg-bg-primary`, `text-text-muted`, …), not raw colors, so
  both themes work. `lib/theme.ts` holds the system/light/dark preference; `lib/editorTheme.ts` themes CodeMirror.
  Shared primitives (`BrandMark`, `Kbd`, `IconButton`, `Switch`, `Menu`) live in `components/ui/`.
- Chat sessions (`useChat`): each chat is an in-memory session; agent runs use source `main:<session-key>`
  so several chats can run at once and keep going while the user is in another chat (sidebar "In progress").
  `send_chat`/`agent_run` only adopt a new backend conversation id if the user is still on the chat that
  started them - keep that compare-and-set when touching `commands.rs`.
- `lib/openFile.ts` is the app-wide channel for "open this file in the editor" / "run this in the terminal";
  absolute file paths in chat answers become clickable through it.
- `components/chat/` - chat UI, including live agent activity rendering (`AgentActivity.tsx`) driven
  by the `agent-progress` event stream from `agent.rs`.
- `components/editor/`, `components/terminal/`, `components/diff/` - the optional "IDE dock"
  (file tree/editor, embedded terminal, git changes pane) backing onto `fs_browser.rs`, `terminal.rs`,
  and `git_diff.rs` respectively.
- `overlay.html` + `public/overlay.js` - deliberately plain JS, not part of the React app, since it's
  a tiny always-on-top popup and doesn't need the React build. How it behaves now:
  - **Routing without a model call.** With the sparkle (Auto) on, `routeFor()` sends questions
    (`?`, question words, web hints) to `ask_about_hover` - with `webSearch` when `needsWeb()` says so -
    and instructions (task verbs) to `agent_run`. Sparkle off = ask-only. The eye button is always a
    plain "explain what's here".
  - **One thread.** Every ask/task is a `.turn` appended to `#agent-log`; follow-ups stack. A reopen
    within 5 minutes continues the thread, a later one starts a fresh thread + `new_conversation`.
  - **@-mentions.** `@screen`, `@window`, `@clipboard`, `@file`, `@web` become chips above the input and
    resolve at send time through `resolve_mention` (commands.rs); text goes to the model as
    `[ATTACHED BY THE USER]`, an image replaces the automatic capture.
  - **Sizing never shrinks mid-thread** (`syncOverlaySize(max, { growOnly })`, one native resize per
    frame, +14px for the root padding) and Rust re-clamps the window into the monitor after each resize.
  - Each run uses source `overlay:<n>`; ✕ cancels a running task before collapsing.
  - `renderMarkdown` keeps numbered lists together across blank lines and preserves item numbers
    (`<li value>`); the test in the session scratchpad exercises the shapes models emit.

### Config

- `src-tauri/tauri.conf.json` defines the windows (`main`, `overlay`, `copilot`, `shield`) and CSP.
- Env vars (loaded from `.env.local`/`.env` at the repo root or `src-tauri/`, see `load_env_files` in
  `lib.rs`): `MOUSE_AI_BACKEND_URL` (override default backend), `MOUSE_AI_GOOGLE_CLIENT_ID` (override
  OAuth client), `MOUSE_AI_GEMINI_MODEL` (passed through to the backend, doesn't add a local key),
  `GROQ_API_KEY` (dev/BYOK only - lets `meeting.rs` call Whisper on Groq directly; production goes
  through the backend's `/api/meeting/transcribe`, then falls back to Gemini), `MOUSE_AI_WHISPER_MODEL`
  (default `whisper-large-v3`).
  Real environment variables always win over the `.env` files.
