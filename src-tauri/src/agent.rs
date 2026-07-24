// Mouse AI Desktop — Local Agent Loop
//
// Turns Mouse AI from "answer one question about the screen" into an agent that
// can DO things: run commands, read/write files, research the web, and drive the
// UI (click/type) to complete a whole task the user described — then report back
// to whichever window they asked from.
//
// Split of responsibility:
//   • The MODEL runs on the backend (Gemini function-calling, /api/agent/step) —
//     it only ever sees the conversation + tool results and decides the next move.
//   • Every TOOL runs HERE, locally, on the user's machine.
//
// The loop mirrors a standard agent turn: ask the model → if it returned
// functionCalls, execute them locally and feed the results back → repeat until
// it returns a plain-text answer (or we hit the iteration cap).

use crate::backend::BackendClient;
use crate::{capture, explorer, input, mcp, mouse};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

const MAX_ITERATIONS: usize = 40;
// A clean HTTP 200 with a genuinely empty candidate (no text, no functionCall) is rare
// but does happen under load — retrying with a short nudge recovers most of these
// instead of ending the whole task on one blank turn (see drive_loop).
const MAX_EMPTY_RESPONSE_RETRIES: usize = 3;
// A single foreground command can't hold the whole task hostage.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_TOOL_OUTPUT: usize = 6000;
const MAX_FILE_READ: usize = 20_000;

/// Emitted to the UI so the user sees each step the agent takes.
#[derive(Clone, serde::Serialize)]
pub struct AgentProgress {
    /// Which window asked — so the right panel renders the live log.
    pub source: String,
    pub kind: String, // "thinking" | "tool" | "tool-result" | "done" | "error" | "confirm"
    pub message: String,
    /// Only set for kind == "confirm" — the id the UI must echo back via
    /// `agent_confirm` when the user clicks Allow/Deny.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

fn emit(app: &AppHandle, source: &str, kind: &str, message: impl Into<String>) {
    let _ = app.emit(
        "agent-progress",
        AgentProgress {
            source: source.to_string(),
            kind: kind.to_string(),
            message: message.into(),
            request_id: None,
        },
    );
}

// ── Screen coordinate mapping ────────────────────────────────────
// The model only ever sees a DOWNSCALED screenshot (kept small so the agent's
// growing conversation history doesn't blow past the backend's request-size
// limit — see capture.rs), but clicks must land on real screen pixels. So the
// model is told to report click coordinates NORMALIZED to a 0-1000 scale
// relative to the screenshot it was just shown, and this maps that back to
// exact global pixels using the captured region's real origin/size. Skipping
// this (i.e. treating the model's numbers as raw screen pixels) is what makes
// clicks land on the wrong element — off by exactly the downscale ratio.
#[derive(Clone, Copy)]
struct ScreenMapping {
    origin_x: i32,
    origin_y: i32,
    native_width: u32,
    native_height: u32,
}

impl ScreenMapping {
    fn from_shot(shot: &capture::AgentShot) -> Self {
        Self {
            origin_x: shot.origin_x,
            origin_y: shot.origin_y,
            native_width: shot.native_width,
            native_height: shot.native_height,
        }
    }

    fn to_real(&self, nx: f64, ny: f64) -> (i32, i32) {
        let nx = nx.clamp(0.0, 1000.0);
        let ny = ny.clamp(0.0, 1000.0);
        let rx = self.origin_x + ((nx / 1000.0) * self.native_width as f64).round() as i32;
        let ry = self.origin_y + ((ny / 1000.0) * self.native_height as f64).round() as i32;
        (rx, ry)
    }
}

// ── Confirmation gate ────────────────────────────────────────────
// The agent can call `request_confirmation` before a sensitive/ambiguous
// action; this pauses that tool call on a channel until the UI reports back
// which button the user clicked (Allow/Deny), via the `agent_confirm` command.
static PENDING_CONFIRMATIONS: OnceLock<Mutex<HashMap<String, oneshot::Sender<bool>>>> = OnceLock::new();

fn confirmations() -> &'static Mutex<HashMap<String, oneshot::Sender<bool>>> {
    PENDING_CONFIRMATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Called by the `agent_confirm` Tauri command when the user clicks a button.
/// Returns false if the request already timed out or doesn't exist.
pub fn resolve_confirmation(request_id: &str, approved: bool) -> bool {
    if let Some(tx) = confirmations().lock().unwrap().remove(request_id) {
        tx.send(approved).is_ok()
    } else {
        false
    }
}

fn system_prompt(autonomous: bool) -> String {
    let mut s = String::from(
        "You are Mouse AI, an autonomous desktop agent running on the user's Windows PC. \
You can actually operate the computer through tools — run commands, read/write files, \
search the web, and control the mouse and keyboard — to complete whatever the user asks.\n\n\
Guidelines:\n\
- The user's request comes with a snapshot of what's currently on their screen (active \
window, the folder/element under their cursor, and a screenshot). Ground your work in that \
context — it's usually the thing they mean by \"this\".\n\
- Break the task into concrete steps and use tools to carry them out. Prefer reading real \
data (run_command, read_file, list_directory, web_search) over guessing.\n\
- COORDINATES: click/double_click/scroll take x and y NORMALIZED to a 0-1000 scale relative to \
the width/height of the most recent screenshot (0,0 = top-left, 1000,1000 = bottom-right) — NOT \
raw pixels, and independent of the screenshot's actual resolution. Always base coordinates on the \
MOST RECENT screenshot only; if you're not looking at a fresh one, call capture_screen again first \
— clicking from a stale or remembered screenshot is the #1 cause of clicking the wrong thing.\n\
- Browser tasks (be precise — don't guess a destination):\n\
  1. Check list_windows first — if a browser is already open, use focus_window to bring it forward \
instead of launching a new one; only fall back to open_url for a brand-new window/tab.\n\
  2. If you already know the exact URL, open_url directly. If you don't, use web_search first and \
open_url the exact result link — never guess or invent a URL, and never click a search result \
before capture_screen confirms it's the one you mean.\n\
  3. If web_search returns 0 results (it can, for very fresh/niche/local queries), don't retry the \
identical query — either rephrase it once with different/fewer keywords, or open_url directly to a \
search engine results page (e.g. https://www.google.com/search?q=... or https://www.bing.com/search?q=...) \
and read the results yourself via capture_screen. Never repeat the exact failed query a third time.\n\
  4. After open_url or focus_window, wait ~2s, then capture_screen before doing anything else — the \
page may still be loading, and acting on stale layout is the #2 cause of wrong clicks.\n\
  5. To fill a form: capture_screen, click the exact field, type_text into it, repeat per field \
(Tab also moves between fields). Verify with capture_screen before submitting (press_key 'enter' \
or click Submit).\n\
  6. To scroll a page (like a normal user would to see more): scroll with a point roughly in the \
center of the page content, then capture_screen to see what's now visible before acting further.\n\
  7. If a page didn't load, looks wrong, or you're unsure you're on the right site, say so and \
capture_screen again rather than clicking blindly — precision matters more than speed.\n\
- Coding/editor tasks (e.g. in Cursor or VS Code): prefer the terminal (run_command) and read_file/\
write_file/edit-style file writes over clicking around the editor UI — it's far more reliable.\n\
- Finding things: use find_files to locate a file anywhere on the PC by name; use run_command with \
Select-String (or the tools) to search inside files.\n\
- Before a genuinely sensitive or ambiguous action — submitting a form with personal/payment info, \
sending a message, making a purchase, deleting/moving files, or anything you're not confident the \
user actually wants — call request_confirmation with a short specific description and WAIT for the \
result before proceeding. If denied, stop that action and ask what they'd like instead.\n\
- When the task is complete, stop calling tools and reply with a clear, concise final answer \
in Markdown describing what you did and the result.\n\
- Be efficient: don't repeat a tool call that already succeeded. If a tool fails twice the same way \
or you notice yourself repeating the same action without progress, stop, try a genuinely different \
approach, or report the blocker as your final answer instead of looping.\n",
    );
    if autonomous {
        s.push_str(
            "\nYou are in autonomous mode: act without asking for confirmation for routine, \
low-risk actions. Still call request_confirmation before sensitive/ambiguous actions (see above), \
and still refuse to run genuinely destructive operations (mass deletion, disk formatting, shutting \
down the machine).",
        );
    }
    s
}

/// Gemini functionDeclarations for every local tool the agent can call.
fn tool_declarations() -> Value {
    json!([
        {
            "name": "run_command",
            "description": "Run a Windows PowerShell command and return its output. Use background=true for long-running/servers so it doesn't block.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The PowerShell command to run." },
                    "background": { "type": "boolean", "description": "Run detached and return immediately." }
                },
                "required": ["command"]
            }
        },
        {
            "name": "read_file",
            "description": "Read a UTF-8 text file from disk and return its contents.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        },
        {
            "name": "write_file",
            "description": "Create or overwrite a text file on disk.",
            "parameters": {
                "type": "object",
                "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
                "required": ["path", "content"]
            }
        },
        {
            "name": "list_directory",
            "description": "List the files and folders in a directory.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        },
        {
            "name": "find_files",
            "description": "Search the PC for files/folders whose name contains a substring (case-insensitive), under a root directory. Use this to locate a file when you don't know its full path.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name_contains": { "type": "string", "description": "Substring to match in file/folder names." },
                    "root": { "type": "string", "description": "Directory to search under. Defaults to the user's home folder." }
                },
                "required": ["name_contains"]
            }
        },
        {
            "name": "web_search",
            "description": "Search the web and return titles, snippets, and URLs.",
            "parameters": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
        },
        {
            "name": "web_fetch",
            "description": "Fetch a URL and return its readable text content.",
            "parameters": { "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }
        },
        {
            "name": "capture_screen",
            "description": "Take a screenshot of the current screen so you can see the present state before acting.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "get_screen_context",
            "description": "Get the active window title, process, cursor position, and the folder/element under the cursor.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "list_windows",
            "description": "List currently open top-level windows (title + process name). Use this to check whether a browser or app is already open before deciding to launch a new one.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "focus_window",
            "description": "Bring an already-open window to the foreground by a case-insensitive substring match on its title or process name (e.g. 'chrome', 'comet', 'notepad'). Prefer this over open_url/open_app when list_windows shows the window already exists — reusing it is more reliable than opening a duplicate.",
            "parameters": {
                "type": "object",
                "properties": { "title_contains": { "type": "string", "description": "Substring to match against window titles/process names." } },
                "required": ["title_contains"]
            }
        },
        {
            "name": "click",
            "description": "Move the mouse to a point and click. x/y are normalized 0-1000 relative to the most recent screenshot's width/height (0,0=top-left, 1000,1000=bottom-right) — always base these on the LATEST screenshot.",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "Horizontal position, 0-1000, normalized to screenshot width." },
                    "y": { "type": "integer", "description": "Vertical position, 0-1000, normalized to screenshot height." },
                    "button": { "type": "string", "enum": ["left", "right"] }
                },
                "required": ["x", "y"]
            }
        },
        {
            "name": "type_text",
            "description": "Type the given text at the current focus (e.g. into a focused form field).",
            "parameters": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
        },
        {
            "name": "press_key",
            "description": "Press a key or combo, e.g. 'enter', 'tab', 'ctrl+c', 'ctrl+shift+t', 'alt+tab'.",
            "parameters": { "type": "object", "properties": { "keys": { "type": "string" } }, "required": ["keys"] }
        },
        {
            "name": "double_click",
            "description": "Double-click the left mouse button at a point (e.g. to open an item). x/y are normalized 0-1000 relative to the most recent screenshot, same as click.",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "0-1000, normalized to screenshot width." },
                    "y": { "type": "integer", "description": "0-1000, normalized to screenshot height." }
                },
                "required": ["x", "y"]
            }
        },
        {
            "name": "scroll",
            "description": "Scroll the mouse wheel at a point. x/y are normalized 0-1000 relative to the most recent screenshot, same as click. Positive amount scrolls up, negative scrolls down (a few notches per call).",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "0-1000, normalized to screenshot width." },
                    "y": { "type": "integer", "description": "0-1000, normalized to screenshot height." },
                    "amount": { "type": "integer", "description": "Notches; negative = down." }
                },
                "required": ["x", "y", "amount"]
            }
        },
        {
            "name": "open_url",
            "description": "Open a URL in the user's default browser (use this to start a browser task before clicking/typing).",
            "parameters": { "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }
        },
        {
            "name": "open_app",
            "description": "Launch an application or open a file/folder by path or name (e.g. 'notepad', 'C:/Users/me/Desktop').",
            "parameters": { "type": "object", "properties": { "target": { "type": "string" } }, "required": ["target"] }
        },
        {
            "name": "wait",
            "description": "Pause briefly to let an app or web page finish loading before the next action.",
            "parameters": { "type": "object", "properties": { "seconds": { "type": "number" } }, "required": ["seconds"] }
        },
        {
            "name": "request_confirmation",
            "description": "Show the user a real Allow/Deny button and WAIT for their decision before proceeding. Call this before sensitive or ambiguous actions: submitting a form with personal/payment info, sending a message, making a purchase, deleting/moving files, or anything you're not confident the user wants.",
            "parameters": {
                "type": "object",
                "properties": { "message": { "type": "string", "description": "A short, specific description of exactly what you're about to do." } },
                "required": ["message"]
            }
        }
    ])
}

/// A tool's result: JSON for the model, plus an optional screenshot to show it.
struct ToolOutcome {
    response: Value,
    image_data_url: Option<String>,
    log: String,
}

impl ToolOutcome {
    fn text(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        ToolOutcome { response: json!({ "result": msg.clone() }), image_data_url: None, log: msg }
    }
    fn err(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        ToolOutcome { response: json!({ "error": msg.clone() }), image_data_url: None, log: format!("error: {}", msg) }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let kept: String = s.chars().take(n).collect();
        format!("{}\n…[truncated, {} chars total]", kept, s.chars().count())
    } else {
        s.to_string()
    }
}

/// Refuse obviously catastrophic shell commands unless the user has opted into
/// running them. This is a coarse safety net, not a sandbox — it blocks the
/// handful of one-liners that could wipe the machine, and lets everything else
/// through (this is the user's own PC, acting on their own request).
fn is_destructive(command: &str) -> Option<&'static str> {
    let c = command.to_lowercase();
    let patterns = [
        ("format ", "disk format"),
        ("remove-item c:\\", "recursive delete of a system drive"),
        ("rd /s", "recursive directory wipe"),
        ("rmdir /s", "recursive directory wipe"),
        ("del /s /q c:\\", "recursive delete of a system drive"),
        ("rm -rf /", "recursive root delete"),
        ("rm -rf ~", "recursive home delete"),
        ("stop-computer", "shutting down the machine"),
        ("shutdown", "shutting down the machine"),
        ("restart-computer", "restarting the machine"),
        ("cipher /w", "disk wipe"),
        ("diskpart", "disk partitioning"),
        ("mkfs", "filesystem format"),
    ];
    patterns.iter().find(|(p, _)| c.contains(p)).map(|(_, label)| *label)
}

/// Build the initial context block describing what's on screen right now, so
/// "this file / this window / what I'm looking at" resolves correctly.
fn initial_context_block(cx: i32, cy: i32) -> String {
    let ctx = mouse::get_cursor_context();
    let element = mouse::get_element_under_cursor(cx, cy);
    let mut block = format!(
        "[CURRENT SCREEN CONTEXT]\nActive app: {}\nActive window: {}\nCursor at: ({}, {})",
        ctx.process_name, ctx.window_title, cx, cy
    );
    let el = [element.role.as_str(), element.name.as_str(), element.value.as_str()]
        .iter()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    if !el.is_empty() {
        block.push_str(&format!("\nElement under cursor: {}", el));
    }
    if let Some(ectx) = explorer::get_context_at(cx, cy) {
        block.push_str("\n\n");
        block.push_str(&explorer::format_context(&ectx));
    }
    block
}

/// Keep only the most recent `keep` screenshots in the conversation, replacing
/// older ones with a short text note. The agent re-sends the whole history each
/// step, so without this the accumulated images quickly exceed the backend's
/// request-body limit (HTTP 413) on any multi-screenshot task.
fn prune_old_images(contents: &mut [Value], keep: usize) {
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for (mi, msg) in contents.iter().enumerate() {
        if let Some(parts) = msg.get("parts").and_then(|p| p.as_array()) {
            for (pi, part) in parts.iter().enumerate() {
                if part.get("inlineData").is_some() {
                    positions.push((mi, pi));
                }
            }
        }
    }
    if positions.len() <= keep {
        return;
    }
    let drop_count = positions.len() - keep;
    for &(mi, pi) in positions.iter().take(drop_count) {
        if let Some(parts) = contents[mi].get_mut("parts").and_then(|p| p.as_array_mut()) {
            parts[pi] = json!({ "text": "[earlier screenshot omitted to keep the request small]" });
        }
    }
}

fn image_part(data_url: &str) -> Option<Value> {
    let (mime, b64) = if let Some(rest) = data_url.strip_prefix("data:image/png;base64,") {
        ("image/png", rest)
    } else if let Some(rest) = data_url.strip_prefix("data:image/jpeg;base64,") {
        ("image/jpeg", rest)
    } else {
        return None;
    };
    Some(json!({ "inlineData": { "mimeType": mime, "data": b64 } }))
}

/// Run one whole agent task to completion. Returns the final answer text.
pub async fn run_task(
    app: &AppHandle,
    backend: &BackendClient,
    task: &str,
    cx: i32,
    cy: i32,
    source: &str,
    autonomous: bool,
) -> Result<String, String> {
    let system = system_prompt(autonomous);

    // Built-in tools, plus any tools exposed by configured MCP servers.
    let mut tool_defs = tool_declarations().as_array().cloned().unwrap_or_default();
    let mcp_configs: Vec<mcp::McpServerConfig> =
        serde_json::from_value(backend.get_mcp_servers().await).unwrap_or_default();
    if !mcp_configs.is_empty() {
        emit(app, source, "thinking", "Connecting to MCP servers…");
    }
    let (mut mcp, mcp_notes) = mcp::McpManager::start(&mcp_configs).await;
    for note in &mcp_notes {
        emit(app, source, "tool-result", note.clone());
    }
    for d in mcp.tool_declarations() {
        tool_defs.push(d);
    }
    let tools = json!(tool_defs);

    // First user turn: the task + a snapshot of the current screen (text context
    // and an initial screenshot) so the model starts grounded in "this".
    let context_block = tokio::task::spawn_blocking(move || initial_context_block(cx, cy))
        .await
        .unwrap_or_default();
    let initial_shot = tokio::task::spawn_blocking(move || capture::capture_agent_window(cx, cy))
        .await
        .ok()
        .and_then(|r| r.ok());

    let mut screen_mapping: Option<ScreenMapping> = initial_shot.as_ref().map(ScreenMapping::from_shot);

    let mut first_parts = vec![json!({
        "text": format!("{}\n\n[TASK]\n{}", context_block, task)
    })];
    if let Some(shot) = &initial_shot {
        if let Some(img) = image_part(&shot.data_url) {
            first_parts.push(img);
        }
    }

    let contents: Vec<Value> = vec![json!({ "role": "user", "parts": first_parts })];

    emit(app, source, "thinking", "Understanding your request…");

    // Drive the loop, then always shut the MCP servers down (whatever the outcome).
    let result = drive_loop(app, backend, &system, &tools, contents, source, &mut mcp, &mut screen_mapping).await;
    mcp.shutdown().await;
    result
}

// Gemini occasionally returns a transient "model overloaded / high demand"
// error (surfaces here as a 502 with that message text) or a plain network
// hiccup. Neither means the task failed — it means "ask again in a moment" —
// so retrying with backoff turns a dead task into a brief pause instead of
// losing all progress and forcing the user to restart from scratch.
const STEP_RETRY_ATTEMPTS: usize = 4;
const STEP_RETRY_BASE_DELAY_SECS: u64 = 3;

fn is_retryable_step_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    [
        "high demand",
        "overloaded",
        "unavailable",
        "please try again later",
        "econnreset",
        "timed out",
        "timeout",
        "reach the thothai backend",
        "(502)",
        "(503)",
        "(500)",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Calls the model with retry-with-backoff on transient errors, so a momentary
/// Gemini overload pauses the task briefly instead of ending it.
async fn agent_step_with_retry(
    app: &AppHandle,
    backend: &BackendClient,
    system: &str,
    contents: &[Value],
    tools: &Value,
    source: &str,
) -> Result<Value, String> {
    let mut attempt = 0usize;
    loop {
        match backend.agent_step(system, json!(contents), tools.clone()).await {
            Ok(v) => return Ok(v),
            Err(e) if attempt < STEP_RETRY_ATTEMPTS && is_retryable_step_error(&e) => {
                attempt += 1;
                let delay = STEP_RETRY_BASE_DELAY_SECS * (1 << (attempt - 1)); // 3s, 6s, 12s, 24s
                emit(
                    app,
                    source,
                    "thinking",
                    format!(
                        "The model is busy ({}). Retrying in {}s… ({}/{})",
                        truncate(&e, 120),
                        delay,
                        attempt,
                        STEP_RETRY_ATTEMPTS
                    ),
                );
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// The core agent turn loop: ask the model, execute any tool calls (built-in or
/// MCP), feed results back, repeat until it answers or we hit the cap.
async fn drive_loop(
    app: &AppHandle,
    backend: &BackendClient,
    system: &str,
    tools: &Value,
    mut contents: Vec<Value>,
    source: &str,
    mcp: &mut mcp::McpManager,
    screen: &mut Option<ScreenMapping>,
) -> Result<String, String> {
    // Anti-loop guard: if the exact same tool calls repeat several iterations
    // in a row (a sign the agent is stuck), inject a stronger nudge to try
    // something different instead of silently spinning until the step cap.
    let mut last_call_signature: Option<String> = None;
    let mut repeat_count: usize = 0;
    let mut empty_response_count: usize = 0;

    for _ in 0..MAX_ITERATIONS {
        // Bound the payload: only the 2 most recent screenshots ride along.
        prune_old_images(&mut contents, 2);
        let step = agent_step_with_retry(app, backend, system, &contents, tools, source).await?;
        let parts = step.get("parts").and_then(|p| p.as_array()).cloned().unwrap_or_default();

        // Split the model's reply into free text and tool calls, and rebuild a
        // clean model turn to echo back (Gemini needs its own functionCalls in
        // the history to match them with our functionResponses).
        let mut assistant_text = String::new();
        let mut calls: Vec<(String, Value)> = Vec::new();
        let mut model_parts: Vec<Value> = Vec::new();

        for part in &parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                if !t.trim().is_empty() {
                    assistant_text.push_str(t);
                    model_parts.push(json!({ "text": t }));
                }
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                model_parts.push(json!({ "functionCall": { "name": name, "args": args } }));
                calls.push((name, args));
            }
        }

        if model_parts.is_empty() {
            if !assistant_text.trim().is_empty() {
                emit(app, source, "done", "Done");
                return Ok(assistant_text);
            }
            // A clean response with no text and no functionCall — Gemini occasionally
            // returns this under load even after the backend's own retry/key-rotation.
            // Nudge it to continue instead of ending the whole task on one blank turn.
            empty_response_count += 1;
            if empty_response_count > MAX_EMPTY_RESPONSE_RETRIES {
                return Err(
                    "The model kept returning empty responses. This can happen under heavy load — \
wait a moment and try again, or try a smaller/more specific task."
                        .to_string(),
                );
            }
            emit(
                app,
                source,
                "thinking",
                format!(
                    "The model returned an empty response — retrying ({}/{})…",
                    empty_response_count, MAX_EMPTY_RESPONSE_RETRIES
                ),
            );
            contents.push(json!({
                "role": "user",
                "parts": [{
                    "text": "[SYSTEM NOTE] Your last turn had no content (no text, no tool call). \
Please continue: call a tool if you need more information, or give a plain-text final answer if \
the task is already done."
                }]
            }));
            continue;
        }
        empty_response_count = 0;
        contents.push(json!({ "role": "model", "parts": model_parts }));

        // No tool calls → this is the final answer.
        if calls.is_empty() {
            emit(app, source, "done", "Done");
            return Ok(assistant_text);
        }

        if !assistant_text.trim().is_empty() {
            emit(app, source, "thinking", truncate(&assistant_text, 400));
        }

        // Anti-loop guard: the same exact batch of tool calls, several times
        // in a row, means the agent is stuck rather than making progress.
        let signature = serde_json::to_string(&calls).unwrap_or_default();
        if last_call_signature.as_deref() == Some(signature.as_str()) {
            repeat_count += 1;
        } else {
            repeat_count = 0;
            last_call_signature = Some(signature);
        }
        let stuck_warning = repeat_count >= 2;

        // Execute every requested tool (built-in or MCP) and collect the
        // responses (+ any images) into one user turn, preserving alternation.
        let mut response_parts: Vec<Value> = Vec::new();
        for (name, args) in calls {
            emit(app, source, "tool", tool_label(&name, &args));
            let outcome = if mcp.owns(&name) {
                match mcp.call(&name, args.clone()).await {
                    Ok(text) => ToolOutcome {
                        response: json!({ "result": truncate(&text, MAX_TOOL_OUTPUT) }),
                        image_data_url: None,
                        log: truncate(&text, 300),
                    },
                    Err(e) => ToolOutcome::err(e),
                }
            } else {
                execute_tool(app, backend, source, &name, &args, screen).await
            };
            emit(app, source, "tool-result", truncate(&outcome.log, 300));
            response_parts.push(json!({
                "functionResponse": { "name": name, "response": outcome.response }
            }));
            if let Some(img) = outcome.image_data_url.as_deref().and_then(image_part) {
                response_parts.push(json!({ "text": "Screenshot after the action:" }));
                response_parts.push(img);
            }
        }
        if stuck_warning {
            emit(app, source, "thinking", "That didn't seem to make progress — nudging the agent to try something different…");
            response_parts.push(json!({
                "text": "[SYSTEM NOTE] You've called the exact same tool(s) with the exact same arguments \
several times in a row without the situation changing. Stop repeating this — either try a genuinely \
different approach (different coordinates after a fresh capture_screen, a different command, a \
different URL) or, if truly stuck, give your final answer now explaining the blocker."
            }));
        }
        contents.push(json!({ "role": "user", "parts": response_parts }));
    }

    Err(format!(
        "Reached the {}-step limit without finishing. Try a smaller task or be more specific.",
        MAX_ITERATIONS
    ))
}

fn tool_label(name: &str, args: &Value) -> String {
    match name {
        "run_command" => format!("$ {}", args.get("command").and_then(|c| c.as_str()).unwrap_or("")),
        "read_file" => format!("Reading {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "write_file" => format!("Writing {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "list_directory" => format!("Listing {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "find_files" => format!("Finding files: {}", args.get("name_contains").and_then(|c| c.as_str()).unwrap_or("")),
        "web_search" => format!("Searching: {}", args.get("query").and_then(|c| c.as_str()).unwrap_or("")),
        "web_fetch" => format!("Fetching {}", args.get("url").and_then(|c| c.as_str()).unwrap_or("")),
        "capture_screen" => "Looking at the screen".to_string(),
        "get_screen_context" => "Reading screen context".to_string(),
        "list_windows" => "Listing open windows".to_string(),
        "focus_window" => format!("Focusing window: {}", args.get("title_contains").and_then(|c| c.as_str()).unwrap_or("")),
        "click" => format!(
            "Clicking ({}, {})",
            args.get("x").and_then(|v| v.as_i64()).unwrap_or(0),
            args.get("y").and_then(|v| v.as_i64()).unwrap_or(0)
        ),
        "double_click" => format!(
            "Double-clicking ({}, {})",
            args.get("x").and_then(|v| v.as_i64()).unwrap_or(0),
            args.get("y").and_then(|v| v.as_i64()).unwrap_or(0)
        ),
        "scroll" => format!("Scrolling {}", args.get("amount").and_then(|v| v.as_i64()).unwrap_or(0)),
        "type_text" => format!("Typing: {}", truncate(args.get("text").and_then(|c| c.as_str()).unwrap_or(""), 60)),
        "press_key" => format!("Pressing {}", args.get("keys").and_then(|c| c.as_str()).unwrap_or("")),
        "open_url" => format!("Opening {}", args.get("url").and_then(|c| c.as_str()).unwrap_or("")),
        "open_app" => format!("Opening {}", args.get("target").and_then(|c| c.as_str()).unwrap_or("")),
        "wait" => format!("Waiting {}s", args.get("seconds").and_then(|v| v.as_f64()).unwrap_or(0.0)),
        other => other.to_string(),
    }
}

async fn execute_tool(
    app: &AppHandle,
    backend: &BackendClient,
    source: &str,
    name: &str,
    args: &Value,
    screen: &mut Option<ScreenMapping>,
) -> ToolOutcome {
    match name {
        "run_command" => run_command_tool(args).await,
        "read_file" => read_file_tool(args),
        "write_file" => write_file_tool(args),
        "list_directory" => list_directory_tool(args),
        "find_files" => find_files_tool(args).await,
        "web_search" => web_search_tool(backend, args).await,
        "web_fetch" => web_fetch_tool(backend, args).await,
        "capture_screen" => capture_screen_tool(screen).await,
        "get_screen_context" => get_screen_context_tool().await,
        "list_windows" => list_windows_tool().await,
        "focus_window" => focus_window_tool(args).await,
        "click" => click_tool(args, screen),
        "double_click" => double_click_tool(args, screen),
        "scroll" => scroll_tool(args, screen),
        "type_text" => type_text_tool(args),
        "press_key" => press_key_tool(args),
        "open_url" => open_target_tool(args, "url"),
        "open_app" => open_target_tool(args, "target"),
        "wait" => wait_tool(args).await,
        "request_confirmation" => request_confirmation_tool(app, source, args).await,
        other => ToolOutcome::err(format!("Unknown tool: {}", other)),
    }
}

async fn run_command_tool(args: &Value) -> ToolOutcome {
    let command = args.get("command").and_then(|c| c.as_str()).unwrap_or("").trim().to_string();
    if command.is_empty() {
        return ToolOutcome::err("no command provided");
    }
    if let Some(label) = is_destructive(&command) {
        return ToolOutcome::err(format!(
            "Refused: this looks like {}. I won't run destructive commands automatically.",
            label
        ));
    }
    let background = args.get("background").and_then(|b| b.as_bool()).unwrap_or(false);
    let cmd = command.clone();

    let result = tokio::task::spawn_blocking(move || run_powershell(&cmd, background)).await;
    match result {
        Ok(Ok(out)) => ToolOutcome {
            response: json!({ "output": truncate(&out, MAX_TOOL_OUTPUT) }),
            image_data_url: None,
            log: if background { format!("started in background: {}", command) } else { truncate(&out, 300) },
        },
        Ok(Err(e)) => ToolOutcome::err(e),
        Err(e) => ToolOutcome::err(format!("task failed: {}", e)),
    }
}

fn run_powershell(command: &str, background: bool) -> Result<String, String> {
    use std::process::{Command, Stdio};

    if background {
        // Detached: fire-and-forget, don't wait for or capture output.
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", command])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to start: {}", e))?;
        return Ok("Command started in the background.".to_string());
    }

    let child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start: {}", e))?;

    // Enforce a wall-clock timeout so one hung command can't stall the agent.
    let out = wait_with_timeout(child, COMMAND_TIMEOUT)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut combined = String::new();
    if !stdout.trim().is_empty() {
        combined.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push_str("\n");
        }
        combined.push_str("[stderr] ");
        combined.push_str(stderr.trim_end());
    }
    if combined.is_empty() {
        combined.push_str("(command completed with no output)");
    }
    Ok(combined)
}

fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    use std::time::Instant;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                return child.wait_with_output().map_err(|e| format!("wait failed: {}", e));
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return Err(format!(
                        "command timed out after {}s (use background=true for long-running commands)",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            Err(e) => return Err(format!("wait failed: {}", e)),
        }
    }
}

fn read_file_tool(args: &Value) -> ToolOutcome {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    if path.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    match std::fs::read_to_string(path) {
        Ok(content) => ToolOutcome {
            response: json!({ "content": truncate(&content, MAX_FILE_READ) }),
            image_data_url: None,
            log: format!("read {} ({} bytes)", path, content.len()),
        },
        Err(e) => ToolOutcome::err(format!("could not read {}: {}", path, e)),
    }
}

fn write_file_tool(args: &Value) -> ToolOutcome {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
    if path.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(path, content) {
        Ok(()) => ToolOutcome::text(format!("Wrote {} ({} bytes).", path, content.len())),
        Err(e) => ToolOutcome::err(format!("could not write {}: {}", path, e)),
    }
}

fn list_directory_tool(args: &Value) -> ToolOutcome {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    if path.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    match std::fs::read_dir(path) {
        Ok(rd) => {
            let mut items: Vec<String> = rd
                .flatten()
                .map(|e| {
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    format!("{}{}", e.file_name().to_string_lossy(), if is_dir { "/" } else { "" })
                })
                .collect();
            items.sort();
            let count = items.len();
            ToolOutcome {
                response: json!({ "entries": items, "count": count }),
                image_data_url: None,
                log: format!("{} items in {}", count, path),
            }
        }
        Err(e) => ToolOutcome::err(format!("could not list {}: {}", path, e)),
    }
}

async fn find_files_tool(args: &Value) -> ToolOutcome {
    let needle = args
        .get("name_contains")
        .and_then(|q| q.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if needle.is_empty() {
        return ToolOutcome::err("name_contains is required");
    }
    let root = args
        .get("root")
        .and_then(|r| r.as_str())
        .filter(|r| !r.trim().is_empty())
        .map(|r| r.to_string())
        .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_else(|| "C:\\".to_string());

    let result = tokio::task::spawn_blocking(move || find_files(&root, &needle)).await;
    match result {
        Ok(matches) => {
            let count = matches.len();
            ToolOutcome {
                response: json!({ "matches": matches, "count": count }),
                image_data_url: None,
                log: format!("{} match(es)", count),
            }
        }
        Err(e) => ToolOutcome::err(format!("search task failed: {}", e)),
    }
}

// Bounded recursive filename search: caps the number of hits and the number of
// directories visited so a search under a huge tree can't run away or hang the
// agent. Skips a few noisy/huge system dirs by name.
fn find_files(root: &str, needle: &str) -> Vec<String> {
    const MAX_MATCHES: usize = 60;
    const MAX_VISITED: usize = 40_000;
    let skip = ["node_modules", "$recycle.bin", "windows", "appdata\\local\\temp", ".git", "target"];

    let mut matches = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    let mut visited = 0usize;

    while let Some(dir) = stack.pop() {
        if matches.len() >= MAX_MATCHES || visited >= MAX_VISITED {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            visited += 1;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_lowercase().contains(needle) {
                matches.push(entry.path().to_string_lossy().to_string());
                if matches.len() >= MAX_MATCHES {
                    break;
                }
            }
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let lower = entry.path().to_string_lossy().to_lowercase();
                if !skip.iter().any(|s| lower.contains(s)) {
                    stack.push(entry.path());
                }
            }
        }
    }
    matches
}

async fn web_search_tool(backend: &BackendClient, args: &Value) -> ToolOutcome {
    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
    if query.is_empty() {
        return ToolOutcome::err("no query provided");
    }
    match backend.agent_search(query).await {
        Ok(v) => {
            let results = v.get("results").cloned().unwrap_or_else(|| json!([]));
            let n = results.as_array().map(|a| a.len()).unwrap_or(0);

            // Snippets from keyless search are thin (often just the title), so
            // fetch the top result's real text to ground the answer. Best-effort
            // and quota-free (the fetch endpoint doesn't call Gemini).
            let mut response = json!({ "results": results });
            if let Some(top_url) = results
                .as_array()
                .and_then(|a| a.first())
                .and_then(|r| r.get("url"))
                .and_then(|u| u.as_str())
            {
                if let Ok(text) = backend.agent_fetch(top_url).await {
                    if !text.trim().is_empty() {
                        response["top_result_url"] = json!(top_url);
                        response["top_result_excerpt"] = json!(truncate(&text, 2500));
                    }
                }
            }

            ToolOutcome {
                response,
                image_data_url: None,
                log: format!("{} results for \"{}\"", n, query),
            }
        }
        Err(e) => ToolOutcome::err(e),
    }
}

async fn web_fetch_tool(backend: &BackendClient, args: &Value) -> ToolOutcome {
    let url = args.get("url").and_then(|u| u.as_str()).unwrap_or("");
    if url.is_empty() {
        return ToolOutcome::err("no url provided");
    }
    match backend.agent_fetch(url).await {
        Ok(text) => ToolOutcome {
            response: json!({ "text": truncate(&text, MAX_TOOL_OUTPUT) }),
            image_data_url: None,
            log: format!("fetched {} ({} chars)", url, text.len()),
        },
        Err(e) => ToolOutcome::err(e),
    }
}

async fn capture_screen_tool(screen: &mut Option<ScreenMapping>) -> ToolOutcome {
    let shot = tokio::task::spawn_blocking(capture::capture_agent_full).await;
    match shot {
        Ok(Ok(shot)) => {
            // Refresh the mapping so the NEXT click/scroll converts against
            // this screenshot, not a stale one.
            *screen = Some(ScreenMapping::from_shot(&shot));
            ToolOutcome {
                response: json!({ "result": "screenshot captured — use normalized 0-1000 x/y relative to this image for the next click/scroll" }),
                image_data_url: Some(shot.data_url),
                log: "captured screenshot".to_string(),
            }
        }
        Ok(Err(e)) => ToolOutcome::err(e),
        Err(e) => ToolOutcome::err(format!("capture task failed: {}", e)),
    }
}

async fn get_screen_context_tool() -> ToolOutcome {
    let outcome = tokio::task::spawn_blocking(|| {
        let (cx, cy) = mouse::get_cursor_position();
        let ctx = mouse::get_cursor_context();
        let element = mouse::get_element_under_cursor(cx, cy);
        let mut response = json!({
            "activeApp": ctx.process_name,
            "activeWindow": ctx.window_title,
            "cursor": { "x": cx, "y": cy },
            "elementName": element.name,
            "elementRole": element.role,
            "elementValue": element.value,
        });
        if let Some(ectx) = explorer::get_context_at(cx, cy) {
            response["folder"] = json!({
                "path": ectx.path,
                "entries": ectx.entries.iter().map(|e| {
                    format!("{}{}", e.name, if e.is_dir { "/" } else { "" })
                }).collect::<Vec<_>>(),
                "tech": ectx.tech_tags,
            });
        }
        response
    })
    .await;

    match outcome {
        Ok(response) => ToolOutcome { response, image_data_url: None, log: "read screen context".to_string() },
        Err(e) => ToolOutcome::err(format!("context task failed: {}", e)),
    }
}

/// One visible top-level window, as seen by `list_windows`/`focus_window`.
struct WindowInfo {
    title: String,
    process_name: String,
}

#[cfg(windows)]
fn enumerate_top_windows() -> Vec<WindowInfo> {
    use winapi::shared::minwindef::{BOOL, LPARAM, TRUE};
    use winapi::shared::windef::HWND;
    use winapi::um::winuser::{
        EnumWindows, GetClassNameW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam as *mut Vec<WindowInfo>);
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return TRUE;
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let actual = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if actual == 0 {
            return TRUE;
        }
        let title = String::from_utf16_lossy(&buf[..actual as usize]).trim().to_string();
        if title.is_empty() {
            return TRUE;
        }
        // Skip the class of invisible/utility "windows" Windows itself creates in bulk
        // (tray icons, IME, tooltips) — they have a title but are noise for this list.
        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
        let class_name = if class_len > 0 {
            String::from_utf16_lossy(&class_buf[..class_len as usize])
        } else {
            String::new()
        };
        if matches!(class_name.as_str(), "Shell_TrayWnd" | "Progman" | "WorkerW" | "IME") {
            return TRUE;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process_name = if pid > 0 { mouse::get_process_name(pid) } else { String::new() };
        out.push(WindowInfo { title, process_name });
        TRUE
    }

    let mut windows: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_proc), &mut windows as *mut Vec<WindowInfo> as LPARAM);
    }
    windows
}

#[cfg(not(windows))]
fn enumerate_top_windows() -> Vec<WindowInfo> {
    Vec::new()
}

#[cfg(windows)]
fn focus_window_by_title(needle: &str) -> Result<String, String> {
    use winapi::shared::minwindef::{BOOL, LPARAM, TRUE};
    use winapi::shared::windef::HWND;
    use winapi::um::processthreadsapi::GetCurrentThreadId;
    use winapi::um::winuser::{
        AttachThreadInput, BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    struct Search<'a> {
        needle: &'a str,
        found: Option<(HWND, String)>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam as *mut Search);
        if search.found.is_some() || IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        let len = GetWindowTextLengthW(hwnd);
        let title = if len > 0 {
            let mut buf = vec![0u16; (len + 1) as usize];
            let actual = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            String::from_utf16_lossy(&buf[..actual.max(0) as usize])
        } else {
            String::new()
        };
        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
        let class_name = if class_len > 0 {
            String::from_utf16_lossy(&class_buf[..class_len as usize])
        } else {
            String::new()
        };
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process_name = if pid > 0 { mouse::get_process_name(pid) } else { String::new() };

        let haystack = format!("{} {}", title, process_name).to_lowercase();
        if !title.trim().is_empty()
            && !matches!(class_name.as_str(), "Shell_TrayWnd" | "Progman" | "WorkerW" | "IME")
            && haystack.contains(search.needle)
        {
            search.found = Some((hwnd, if title.trim().is_empty() { process_name } else { title }));
        }
        TRUE
    }

    let mut search = Search { needle, found: None };
    unsafe {
        EnumWindows(Some(enum_proc), &mut search as *mut Search as LPARAM);
    }

    let (hwnd, label) = search
        .found
        .ok_or_else(|| format!("no open window matches \"{}\" — check list_windows for the exact title", needle))?;

    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        let foreground = GetForegroundWindow();
        let current_thread = GetCurrentThreadId();
        let fg_thread = if !foreground.is_null() {
            GetWindowThreadProcessId(foreground, std::ptr::null_mut())
        } else {
            0
        };
        if fg_thread != 0 && fg_thread != current_thread {
            AttachThreadInput(current_thread, fg_thread, 1);
        }
        BringWindowToTop(hwnd);
        let ok = SetForegroundWindow(hwnd);
        if fg_thread != 0 && fg_thread != current_thread {
            AttachThreadInput(current_thread, fg_thread, 0);
        }
        if ok == 0 {
            return Err(format!(
                "found \"{}\" but Windows blocked bringing it to the foreground — try clicking its taskbar icon instead",
                label
            ));
        }
    }
    Ok(label)
}

#[cfg(not(windows))]
fn focus_window_by_title(_needle: &str) -> Result<String, String> {
    Err("window focus is only supported on Windows".to_string())
}

async fn list_windows_tool() -> ToolOutcome {
    let windows = tokio::task::spawn_blocking(enumerate_top_windows).await.unwrap_or_default();
    let count = windows.len();
    let entries: Vec<Value> = windows
        .iter()
        .take(40)
        .map(|w| json!({ "title": w.title, "process": w.process_name }))
        .collect();
    ToolOutcome {
        response: json!({ "windows": entries, "count": count }),
        image_data_url: None,
        log: format!("{} open window(s)", count),
    }
}

async fn focus_window_tool(args: &Value) -> ToolOutcome {
    let needle = args
        .get("title_contains")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if needle.is_empty() {
        return ToolOutcome::err("title_contains is required");
    }
    let result = tokio::task::spawn_blocking(move || focus_window_by_title(&needle)).await;
    match result {
        Ok(Ok(label)) => ToolOutcome::text(format!("Focused \"{}\".", label)),
        Ok(Err(e)) => ToolOutcome::err(e),
        Err(e) => ToolOutcome::err(format!("focus task failed: {}", e)),
    }
}

/// Reads normalized x/y args and converts them to real screen pixels via the
/// most recent screenshot's mapping. Returns an error (instead of guessing)
/// if there's no mapping yet — the model needs to capture_screen first.
fn resolve_point(args: &Value, screen: &Option<ScreenMapping>) -> Result<(i32, i32), String> {
    let nx = args.get("x").and_then(|v| v.as_f64());
    let ny = args.get("y").and_then(|v| v.as_f64());
    let (nx, ny) = match (nx, ny) {
        (Some(x), Some(y)) => (x, y),
        _ => return Err("requires numeric x and y".to_string()),
    };
    let mapping = screen
        .as_ref()
        .ok_or("no screenshot has been captured yet — call capture_screen first so coordinates have something to map to")?;
    Ok(mapping.to_real(nx, ny))
}

fn click_tool(args: &Value, screen: &Option<ScreenMapping>) -> ToolOutcome {
    let (x, y) = match resolve_point(args, screen) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::err(e),
    };
    let right = args.get("button").and_then(|b| b.as_str()).map(|b| b.eq_ignore_ascii_case("right")).unwrap_or(false);
    input::click(x, y, right);
    ToolOutcome::text(format!("Clicked {} at ({}, {}).", if right { "right" } else { "left" }, x, y))
}

fn double_click_tool(args: &Value, screen: &Option<ScreenMapping>) -> ToolOutcome {
    match resolve_point(args, screen) {
        Ok((x, y)) => {
            input::double_click(x, y);
            ToolOutcome::text(format!("Double-clicked ({}, {}).", x, y))
        }
        Err(e) => ToolOutcome::err(e),
    }
}

fn scroll_tool(args: &Value, screen: &Option<ScreenMapping>) -> ToolOutcome {
    let amount = args.get("amount").and_then(|v| v.as_i64());
    let point = resolve_point(args, screen);
    match (point, amount) {
        (Ok((x, y)), Some(amount)) => {
            input::scroll(x, y, amount as i32);
            ToolOutcome::text(format!("Scrolled {} at ({}, {}).", amount, x, y))
        }
        (Err(e), _) => ToolOutcome::err(e),
        (_, None) => ToolOutcome::err("scroll requires an integer amount"),
    }
}

/// Shows the user a real Allow/Deny button and waits for their click (with a
/// 5-minute timeout, treated as a deny so the task doesn't hang forever).
async fn request_confirmation_tool(app: &AppHandle, source: &str, args: &Value) -> ToolOutcome {
    let message = args
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("Proceed with this action?")
        .to_string();
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    confirmations().lock().unwrap().insert(request_id.clone(), tx);

    let _ = app.emit(
        "agent-progress",
        AgentProgress {
            source: source.to_string(),
            kind: "confirm".to_string(),
            message: message.clone(),
            request_id: Some(request_id.clone()),
        },
    );

    const CONFIRM_TIMEOUT: Duration = Duration::from_secs(300);
    match tokio::time::timeout(CONFIRM_TIMEOUT, rx).await {
        Ok(Ok(true)) => ToolOutcome::text("The user clicked Allow. Proceed."),
        Ok(Ok(false)) => ToolOutcome::text("The user clicked Deny. Do not proceed with this action — ask what they'd like instead, or stop."),
        Ok(Err(_)) => ToolOutcome::err("confirmation channel closed unexpectedly"),
        Err(_) => {
            confirmations().lock().unwrap().remove(&request_id);
            ToolOutcome::text("No response within 5 minutes — treat this as denied and stop this action.")
        }
    }
}

/// Launch a URL / app / file via the shell (ShellExecute semantics), which
/// picks the right default handler — a browser for URLs, Explorer for folders,
/// the associated app for files.
fn open_target_tool(args: &Value, key: &str) -> ToolOutcome {
    let target = args.get(key).and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
    if target.is_empty() {
        return ToolOutcome::err("nothing to open");
    }
    // `cmd /C start "" <target>` uses the shell to resolve URLs (default
    // browser), file/folder paths (associated app / Explorer), and app names
    // on PATH. The empty "" is start's title argument, so a quoted target
    // isn't mistaken for the window title.
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", &target])
        .spawn();
    match result {
        Ok(_) => ToolOutcome::text(format!("Opened {}.", target)),
        Err(e) => ToolOutcome::err(format!("could not open {}: {}", target, e)),
    }
}

async fn wait_tool(args: &Value) -> ToolOutcome {
    let seconds = args.get("seconds").and_then(|s| s.as_f64()).unwrap_or(1.0).clamp(0.0, 15.0);
    tokio::time::sleep(Duration::from_millis((seconds * 1000.0) as u64)).await;
    ToolOutcome::text(format!("Waited {:.1}s.", seconds))
}

fn type_text_tool(args: &Value) -> ToolOutcome {
    let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("");
    if text.is_empty() {
        return ToolOutcome::err("no text provided");
    }
    input::type_text(text);
    ToolOutcome::text(format!("Typed {} characters.", text.chars().count()))
}

fn press_key_tool(args: &Value) -> ToolOutcome {
    let keys = args.get("keys").and_then(|k| k.as_str()).unwrap_or("");
    if keys.is_empty() {
        return ToolOutcome::err("no keys provided");
    }
    match input::press_combo(keys) {
        Ok(()) => ToolOutcome::text(format!("Pressed {}.", keys)),
        Err(e) => ToolOutcome::err(e),
    }
}
