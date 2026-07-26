# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Mouse AI Desktop — a Windows desktop AI assistant built with Tauri 2 (Rust backend + React/TypeScript
frontend). It talks to an external **Thothai backend** (`../backend-Thothai`, a sibling repo, not part
of this codebase) which holds the Gemini API key(s) and does all model inference. This repo only ever
runs *tools* locally (shell commands, file I/O, mouse/keyboard control, screen capture) — the model
itself never runs here.

Two windows are built from `frontend/`: the main chat/IDE window (`index.html`, React) and a small
always-on-top hover overlay (`overlay.html`, plain JS, see `frontend/public/overlay.js`) that pops up
near the cursor.

## Commands

Run from the repo root unless noted.

```bash
npm install                    # installs the root tauri CLI only; frontend deps install separately
npm run dev                    # = tauri dev — builds Rust, launches Vite dev server, opens the app
npm run build                  # = tauri build — produces installer in src-tauri/target/release/bundle/
```

Frontend-only (from `frontend/`):

```bash
npm run dev                    # vite dev server on :1420 (used by tauri dev via devUrl)
npm run build                  # tsc -b && vite build -> frontend/dist (two entry points: main, overlay)
npm run lint                   # oxlint
```

Rust-only (from `src-tauri/`): standard `cargo build` / `cargo check` work directly.

There are no automated tests in this repo currently.

## Architecture

### Split of responsibility: backend does the thinking, this app does the doing

The Gemini model runs entirely on the Thothai backend via HTTP (`/api/agent/step`, `/api/chat`, etc. —
see `src-tauri/src/backend.rs`, which mirrors the browser extension's `shared/backend-client.js`).
This app sends conversation state + tool results and gets back either plain text or a list of
`functionCalls` to execute. There is no Gemini key anywhere in this codebase — the backend rotates
across up to 5 shared keys server-side.

The agent loop lives in `src-tauri/src/agent.rs`: ask the model → if it returned functionCalls,
execute them locally (`run_command`, `read_file`, `write_file`, `find_files`, `web_search`,
`web_fetch`, `capture_screen`, `get_screen_context`, `click`/`double_click`/`scroll`/`type_text`/
`press_key`, `open_url`/`open_app`, `wait`, plus MCP-provided tools) → feed results back → repeat
until a plain-text answer or `MAX_ITERATIONS` (40) is hit. Key details baked into this file:

- **Screen coordinate mapping**: the model only ever sees a downscaled screenshot; click coordinates
  it returns are normalized 0-1000 and must be mapped back to real screen pixels via `ScreenMapping`
  before dispatching a click. Skipping this scaling is the classic "clicks land on the wrong element"
  bug.
- **Confirmation gate**: the agent can call `request_confirmation` before a sensitive action, which
  blocks that tool call on a `oneshot` channel until the UI reports the user's Allow/Deny via the
  `agent_confirm` command (see `PENDING_CONFIRMATIONS` in `agent.rs`).
- **Agent scratch dir**: relative paths from the model resolve into a temp scratch directory
  (`resolve_agent_path`), wiped at the start of every `run_task`, *not* the process cwd — this is
  what stops model-written throwaway files from landing inside this repo's own source tree.
- Genuinely destructive shell commands (disk format, recursive system-drive delete, shutdown, …) are
  refused before ever reaching `run_command`.

### Rust module map (`src-tauri/src/`)

| Module | Responsibility |
| --- | --- |
| `lib.rs` | App entry: Tauri builder, system tray, global shortcuts, window setup, invoke_handler registration |
| `commands.rs` | Tauri `#[command]` IPC endpoints exposed to the frontend; owns `AppState` (backend client, ai_active, conversation_id, overlay_pinned) |
| `agent.rs` | The local agent loop and every built-in tool implementation |
| `backend.rs` | HTTP client for the Thothai backend (auth, chat, agent step, conversation history) |
| `mouse.rs` | Cursor position/context, accessible-element inspection under the cursor (Win32 UI Automation) |
| `capture.rs` | Screenshot capture (full screen, region, window-or-around-point) |
| `hover.rs` | Global hover-dwell watcher — shows the overlay's "Ask AI" pill when the mouse pauses over something |
| `input.rs` | Synthesized mouse/keyboard input for the agent's computer-use tools |
| `explorer.rs` | Windows Explorer / file-context helpers used by tools |
| `fs_browser.rs` | File-tree browsing + read/write for the IDE dock's file tree and editor |
| `terminal.rs` | PTY spawn/write/resize backing the IDE dock's embedded terminal |
| `git_diff.rs` | Git status/diff for the IDE dock's changes pane |
| `mcp.rs` | MCP client: launches configured servers as stdio subprocesses, speaks JSON-RPC 2.0 (`initialize` → `tools/list` → `tools/call`). Strictly additive/fail-safe — a broken MCP server is skipped, never breaks the core agent loop |
| `oauth.rs` | Google OAuth loopback flow (`http://127.0.0.1:8917/callback`) for Sign in with Google |

Session, backend URL, and MCP server config persist to `%APPDATA%/mouse-ai/config.json`.

### Frontend structure (`frontend/src/`)

- `lib/tauri.ts` — the single place that wraps every `invoke()`/`listen()` call to a Rust command.
  When a Rust command's name or shape changes, update it here rather than calling `invoke` directly
  from components.
- `hooks/` — `useAuth`, `useChat`, `useConversations`, `useWorkspace` hold the main window's stateful
  logic; `App.tsx` wires them together and switches between the chat and settings views.
- `components/chat/` — chat UI, including live agent activity rendering (`AgentActivity.tsx`) driven
  by the `agent-progress` event stream from `agent.rs`.
- `components/editor/`, `components/terminal/`, `components/diff/` — the optional "IDE dock"
  (file tree/editor, embedded terminal, git changes pane) backing onto `fs_browser.rs`, `terminal.rs`,
  and `git_diff.rs` respectively.
- `overlay.html` + `public/overlay.js` — deliberately plain JS, not part of the React app, since it's
  a tiny always-on-top popup and doesn't need the React build.

### Config

- `src-tauri/tauri.conf.json` defines the two windows (`main`, `overlay`) and CSP.
- Env vars (loaded from `.env.local`/`.env` at the repo root or `src-tauri/`, see `load_env_files` in
  `lib.rs`): `MOUSE_AI_BACKEND_URL` (override default backend), `MOUSE_AI_GOOGLE_CLIENT_ID` (override
  OAuth client), `MOUSE_AI_GEMINI_MODEL` (passed through to the backend, doesn't add a local key).
  Real environment variables always win over the `.env` files.
