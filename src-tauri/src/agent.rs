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
use crate::{capture, explorer, input, mouse};
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const MAX_ITERATIONS: usize = 28;
// A single foreground command can't hold the whole task hostage.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_TOOL_OUTPUT: usize = 6000;
const MAX_FILE_READ: usize = 20_000;

/// Emitted to the UI so the user sees each step the agent takes.
#[derive(Clone, serde::Serialize)]
pub struct AgentProgress {
    /// Which window asked — so the right panel renders the live log.
    pub source: String,
    pub kind: String, // "thinking" | "tool" | "tool-result" | "done" | "error"
    pub message: String,
}

fn emit(app: &AppHandle, source: &str, kind: &str, message: impl Into<String>) {
    let _ = app.emit(
        "agent-progress",
        AgentProgress {
            source: source.to_string(),
            kind: kind.to_string(),
            message: message.into(),
        },
    );
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
- To operate other apps or a browser, use capture_screen to see the current state, then \
click/double_click/type_text/press_key/scroll on the coordinates you see in that screenshot. \
IMPORTANT: coordinates come from the most recent screenshot — always capture_screen right before \
clicking, and capture_screen again after acting to confirm the result before continuing.\n\
- Browser tasks: open_url to launch the page, wait ~2s for it to load, capture_screen, then click \
fields and type_text to fill forms. To fill a form field, click it first, then type_text. Use Tab \
to move between fields and press_key 'enter' to submit.\n\
- Coding/editor tasks (e.g. in Cursor or VS Code): prefer the terminal (run_command) and read_file/\
write_file/edit-style file writes over clicking around the editor UI — it's far more reliable.\n\
- Finding things: use find_files to locate a file anywhere on the PC by name; use run_command with \
Select-String (or the tools) to search inside files.\n\
- When the task is complete, stop calling tools and reply with a clear, concise final answer \
in Markdown describing what you did and the result.\n\
- Be efficient: don't repeat a tool call that already succeeded. If a tool fails twice the same way, \
try a different approach or report the blocker instead of looping.\n",
    );
    if autonomous {
        s.push_str(
            "\nYou are in autonomous mode: act without asking for confirmation. Still refuse to \
run genuinely destructive operations (mass deletion, disk formatting, shutting down the machine).",
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
            "name": "click",
            "description": "Move the mouse to global screen coordinates and click. Use screenshot coordinates.",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
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
            "description": "Double-click the left mouse button at global screen coordinates (e.g. to open an item).",
            "parameters": {
                "type": "object",
                "properties": { "x": { "type": "integer" }, "y": { "type": "integer" } },
                "required": ["x", "y"]
            }
        },
        {
            "name": "scroll",
            "description": "Scroll the mouse wheel at a point. Positive amount scrolls up, negative scrolls down (a few notches per call).",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
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
    let initial_shot = tokio::task::spawn_blocking(move || {
        capture::capture_window_or_around_point(cx, cy, 400)
    })
    .await
    .ok()
    .and_then(|r| r.ok());

    let mut first_parts = vec![json!({
        "text": format!("{}\n\n[TASK]\n{}", context_block, task)
    })];
    if let Some(shot) = &initial_shot {
        if let Some(img) = image_part(shot) {
            first_parts.push(img);
        }
    }

    let contents: Vec<Value> = vec![json!({ "role": "user", "parts": first_parts })];

    emit(app, source, "thinking", "Understanding your request…");

    // Drive the loop, then always shut the MCP servers down (whatever the outcome).
    let result = drive_loop(app, backend, &system, &tools, contents, source, &mut mcp).await;
    mcp.shutdown().await;
    result
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
) -> Result<String, String> {
    for _ in 0..MAX_ITERATIONS {
        let step = backend.agent_step(system, json!(contents), tools.clone()).await?;
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
            return Err("The agent returned an empty response.".to_string());
        }
        contents.push(json!({ "role": "model", "parts": model_parts }));

        // No tool calls → this is the final answer.
        if calls.is_empty() {
            emit(app, source, "done", "Done");
            return Ok(assistant_text);
        }

        if !assistant_text.trim().is_empty() {
            emit(app, source, "thinking", truncate(&assistant_text, 400));
        }

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
                execute_tool(app, backend, &name, &args).await
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
    name: &str,
    args: &Value,
) -> ToolOutcome {
    match name {
        "run_command" => run_command_tool(args).await,
        "read_file" => read_file_tool(args),
        "write_file" => write_file_tool(args),
        "list_directory" => list_directory_tool(args),
        "find_files" => find_files_tool(args).await,
        "web_search" => web_search_tool(backend, args).await,
        "web_fetch" => web_fetch_tool(backend, args).await,
        "capture_screen" => capture_screen_tool().await,
        "get_screen_context" => get_screen_context_tool().await,
        "click" => click_tool(args),
        "double_click" => double_click_tool(args),
        "scroll" => scroll_tool(args),
        "type_text" => type_text_tool(args),
        "press_key" => press_key_tool(args),
        "open_url" => open_target_tool(args, "url"),
        "open_app" => open_target_tool(args, "target"),
        "wait" => wait_tool(args).await,
        other => {
            let _ = app;
            ToolOutcome::err(format!("Unknown tool: {}", other))
        }
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
            ToolOutcome {
                response: json!({ "results": results }),
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

async fn capture_screen_tool() -> ToolOutcome {
    let shot = tokio::task::spawn_blocking(|| capture::capture_full_screen()).await;
    match shot {
        Ok(Ok(data_url)) => ToolOutcome {
            response: json!({ "result": "screenshot captured" }),
            image_data_url: Some(data_url),
            log: "captured screenshot".to_string(),
        },
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

fn click_tool(args: &Value) -> ToolOutcome {
    let x = args.get("x").and_then(|v| v.as_i64());
    let y = args.get("y").and_then(|v| v.as_i64());
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (x as i32, y as i32),
        _ => return ToolOutcome::err("click requires integer x and y"),
    };
    let right = args.get("button").and_then(|b| b.as_str()).map(|b| b.eq_ignore_ascii_case("right")).unwrap_or(false);
    input::click(x, y, right);
    ToolOutcome::text(format!("Clicked {} at ({}, {}).", if right { "right" } else { "left" }, x, y))
}

fn double_click_tool(args: &Value) -> ToolOutcome {
    let x = args.get("x").and_then(|v| v.as_i64());
    let y = args.get("y").and_then(|v| v.as_i64());
    match (x, y) {
        (Some(x), Some(y)) => {
            input::double_click(x as i32, y as i32);
            ToolOutcome::text(format!("Double-clicked ({}, {}).", x, y))
        }
        _ => ToolOutcome::err("double_click requires integer x and y"),
    }
}

fn scroll_tool(args: &Value) -> ToolOutcome {
    let x = args.get("x").and_then(|v| v.as_i64());
    let y = args.get("y").and_then(|v| v.as_i64());
    let amount = args.get("amount").and_then(|v| v.as_i64());
    match (x, y, amount) {
        (Some(x), Some(y), Some(amount)) => {
            input::scroll(x as i32, y as i32, amount as i32);
            ToolOutcome::text(format!("Scrolled {} at ({}, {}).", amount, x, y))
        }
        _ => ToolOutcome::err("scroll requires integer x, y, and amount"),
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
