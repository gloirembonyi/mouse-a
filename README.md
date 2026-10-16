# Kudu Desktop

Kudu's desktop AI assistant for Windows, macOS and Linux, running on **Kudu Core** (the backend, `../backend-Thothai`).
Built with Tauri 2 (Rust + React/TypeScript). Point your mouse at anything on screen and ask the AI about it.

## Features

- **Agent mode** - describe a task and Kudu actually does it on your PC: runs terminal
  commands, reads/writes/searches files, researches the web, and drives the mouse & keyboard
  (click, type, scroll) to operate apps and fill browser forms - reporting a live activity log
  and the result back in the window you asked from. Toggle **🤖 Agent** in the main window, or the
  **🤖** button in the cursor overlay.
- **Ask at Cursor** - `Ctrl+Shift+Space` anywhere: an input panel opens at your cursor about whatever you're pointing at (screenshot + accessible element text are sent as context).
- **Kudu Lens** - `Ctrl+Shift+Q`: pause the mouse over anything and a floating **✦ Ask Kudu** pill (the Kudu Lens pill) appears.
- **Screenshot Analyze** - `Ctrl+Shift+S`: captures around the cursor and analyzes it in the main window.
- **MCP support** - add external [MCP](https://modelcontextprotocol.io) servers in Settings; their
  tools are added to the agent automatically.
- System tray app; closing the window minimizes to tray.

## Agent tools

The agent decides which of these to use per step (the model runs on the backend; every tool runs
locally on your machine):

`run_command` (PowerShell, foreground or background) · `read_file` · `write_file` · `list_directory` ·
`find_files` (search the whole PC by name) · `web_search` · `web_fetch` · `capture_screen` ·
`get_screen_context` · `click` / `double_click` / `scroll` / `type_text` / `press_key`
(computer-use) · `open_url` / `open_app` · `wait` · plus any tools from configured MCP servers.

Genuinely destructive shell commands (disk format, recursive system-drive delete, shutdown, …) are
refused automatically.

## Gemini API keys

There is no Gemini key configuration in this app at all - every chat/agent call goes through the
Kudu Core, which holds the only Gemini key(s) and rotates across up to 5 shared keys
(`GEMINI_API_KEY`, `GEMINI_API_KEY_2`..`_5` on the backend) when one is rate-limited or overloaded.
See `backend-Thothai/src/lib/gemini-request.js`. To raise the effective rate limit, add more shared
keys on the backend deployment - there's nothing to configure here.

## Platform support

Every feature works on Windows. On macOS and Linux the app builds and runs with the differences below;
the GitHub Actions workflow (`.github/workflows/build.yml`) builds installers for all three on every push
and attaches them to a draft release when you push a `v*` tag.

| Feature | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Chat, agent tools (files, shell, web, MCP), Kudu Lens overlay, bridge to the browser extension | yes | yes | yes |
| Shell for `run_command` / terminal | PowerShell 5.1 | bash (`$SHELL` in the terminal) | bash (`$SHELL` in the terminal) |
| Mouse/keyboard control (click, type, press_key) | SendInput | CGEvent via `enigo` - grant **Accessibility** | X11 via `enigo` - needs an X11 session (Wayland: limited) |
| Screenshots (`capture_screen`, Kudu Lens context) | yes | grant **Screen Recording** | yes (X11) |
| Element-level tools (`find_elements`, `click_element`, `set_value`), window under the cursor | UI Automation | frontmost app/title only (`osascript`) | frontmost app/title only (`xdotool`) |
| Kudu Pilot: microphone | yes | grant **Microphone** | yes |
| Kudu Pilot: the other participants (system audio) | WASAPI loopback | needs a virtual device (e.g. BlackHole) selected as the input | needs a PulseAudio/PipeWire monitor selected as the input |
| Hide windows from screen share | yes | yes | no (not supported by the platform) |
| Global hotkeys, tray, Esc-to-stop | yes | yes | yes |

macOS asks for each permission the first time it is needed (System Settings > Privacy & Security).
Linux builds need the Tauri system libraries plus `libasound2-dev`, `libx11-dev`, `libxtst-dev`,
`libxkbcommon-dev`, `libxdo-dev` and the `xdotool` package at runtime (see the workflow for the exact list).

## Prerequisites

- Rust (stable) + the Tauri 2 prerequisites for your OS (Windows: WebView2 + MSVC build tools;
  macOS: Xcode command line tools; Linux: the packages listed above)
- Node.js (only for the `tauri` CLI)

## Run in development

```bash
npm install
npm run dev        # = tauri dev (builds Rust, opens the app)
```

## Build an installer

```bash
npm run build      # = tauri build → src-tauri/target/release/bundle/
```

## Connecting to Kudu Core

The app talks to the same backend (Kudu Core) as the Kudu Connect browser extension. Default URL:
`https://backend-thothai.vercel.app` - change it in **Settings → Backend URL**
(use **Test connection** to verify), or set the `MOUSE_AI_BACKEND_URL` env var.
Session + URL persist in `%APPDATA%/mouse-ai/config.json`.

## Google sign-in

**Sign in with Google** opens your browser on Google's consent screen and
catches the redirect on `http://127.0.0.1:8917/callback`.

**One-time setup** (same Google Cloud project as the extension): open the
*Web application* OAuth client (the one used for `GOOGLE_OAUTH_WEB_CLIENT_ID`
in the extension's `.env`) and add this to **Authorized redirect URIs**:

```
http://127.0.0.1:8917/callback
```

Until that's registered, Google shows `redirect_uri_mismatch` - in that case
the app also accepts a manually pasted access token (or a full redirect URL
containing `access_token=…`) as a fallback.

A different OAuth client can be used via the `MOUSE_AI_GOOGLE_CLIENT_ID` env var.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+Space` | Ask at cursor (anywhere) |
| `Ctrl+Shift+Q` | Toggle Kudu Lens |
| `Ctrl+Shift+S` | Screenshot & analyze |
| `Esc` | Dismiss overlay |
