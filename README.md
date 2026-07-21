# Mouse AI Desktop

A Windows desktop AI assistant powered by the **Thothai backend** (`../backend-Thothai`).
Built with Tauri 2 (Rust + vanilla HTML/JS). Point your mouse at anything on screen and ask the AI about it.

## Features

- **Ask at Cursor** — `Ctrl+Shift+Space` anywhere: an input panel opens at your cursor about whatever you're pointing at (screenshot + accessible element text are sent as context).
- **Hover Mode** — `Ctrl+Shift+Q`: pause the mouse over anything and a floating **✦ Ask AI** pill appears.
- **Screenshot Analyze** — `Ctrl+Shift+S`: captures around the cursor and analyzes it in the main window.
- System tray app; closing the window minimizes to tray.

## Prerequisites

- Rust (stable) + the Tauri 2 prerequisites for Windows (WebView2, MSVC build tools)
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

## Connecting to the Thothai backend

The app talks to the same backend as the Thothai browser extension. Default URL:
`https://backend-thothai.vercel.app` — change it in **Settings → Backend URL**
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

Until that's registered, Google shows `redirect_uri_mismatch` — in that case
the app also accepts a manually pasted access token (or a full redirect URL
containing `access_token=…`) as a fallback.

A different OAuth client can be used via the `MOUSE_AI_GOOGLE_CLIENT_ID` env var.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+Space` | Ask at cursor (anywhere) |
| `Ctrl+Shift+Q` | Toggle hover mode |
| `Ctrl+Shift+S` | Screenshot & analyze |
| `Esc` | Dismiss overlay |
