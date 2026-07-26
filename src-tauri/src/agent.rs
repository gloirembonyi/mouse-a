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
// A model-requested timeout_seconds is honored up to this — long enough for a
// real build/install, still bounded so a stuck command can't wedge the task
// indefinitely (background:true remains the right tool for anything longer).
const MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_TOOL_OUTPUT: usize = 6000;
const MAX_FILE_READ: usize = 20_000;

// ── Agent scratch space ──────────────────────────────────────────
// Root cause of files like `yt_search.py` turning up inside the app's own
// source tree: run_command/write_file/read_file resolve a relative path
// against whatever the CURRENT PROCESS's cwd happens to be (src-tauri/ under
// `tauri dev`) when the model doesn't pass an absolute one. Rather than
// trust every future prompt revision to remember an absolute path, relative
// paths now resolve here instead — a real OS temp folder, wiped clean at the
// start of every task, so scratch work literally cannot accumulate or land
// in the user's project regardless of what the model does.
fn agent_scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("mouse-ai-agent-scratch");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Wipes the scratch dir back to empty — called once at the start of every
/// `run_task` so a previous run's throwaway files never linger, even if the
/// model never called delete_file on them itself.
fn reset_scratch_dir() {
    let dir = agent_scratch_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
}

/// Absolute paths are used as-is (the model explicitly means a real location
/// — the user's project, Desktop, wherever). A relative path resolves inside
/// the private scratch dir instead of the process's actual cwd.
fn resolve_agent_path(raw: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        agent_scratch_dir().join(p)
    }
}

/// Emitted to the UI so the user sees each step the agent takes.
#[derive(Clone, serde::Serialize)]
pub struct AgentProgress {
    /// Which window asked — so the right panel renders the live log.
    pub source: String,
    pub kind: String, // "plan" | "thinking" | "phase-summary" | "tool" | "tool-result" | "done" | "error" | "confirm"
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
write_file/edit-style file writes over clicking around the editor UI — it's far more reliable. Use \
run_command's cwd to run inside a specific project folder instead of cd-ing in the command string, and \
pass timeout_seconds (up to 300) for anything slower than the ~45s default — an install/build/test run \
that just needs more time is not the same as background=true, which is for long-running/indefinite \
processes (servers, watchers) you don't want to wait on at all.\n\
- Finding things: use find_files to locate a file anywhere on the PC by name; use run_command with \
Select-String (or the tools) to search inside files.\n\
- Tool-first, file-last: before writing any file, check whether a tool you already have does the \
job directly — web_search/web_fetch for research, run_command for a one-off computation or listing, \
read_file/list_directory for inspecting something that already exists. Writing and running a script \
is a LAST resort for when no existing tool can do it, not a default way to \"do research\" or \"check \
something\" — e.g. never write a .py/.js file to search or scrape a site when web_search/web_fetch \
already return that directly.\n\
- Temporary vs. permanent files: a file you write purely as your own means to an end (a scratch \
script, an intermediate data dump, notes you're building an answer from) is temporary working \
material, not something the user asked for — write it with a relative path (it lands in a private \
scratch folder, not the user's project or Desktop) and delete_file it once you've used it, before \
your final answer. Only write to a real absolute path — the user's project, Desktop, a folder they \
named — when the user's request is actually for a persisted file at that location (\"save this as\", \
\"create a document\", \"write the code to my project\"); that's the one case a file should be left \
behind. When in doubt about which one a file is, err toward scratch + cleanup.\n\
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
            "description": "Run a Windows PowerShell command and return its output. Use background=true for long-running/servers so it doesn't block. For anything that takes more than a few seconds (installs, builds), pass timeout_seconds explicitly rather than letting it hit the default and fail — up to 300s foreground, or use background=true for anything longer/indefinite. IMPORTANT: only use this (or write_file) when no built-in tool already does the job — e.g. never write and run a script to search the web or fetch a page, web_search/web_fetch already do that directly.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The PowerShell command to run." },
                    "background": { "type": "boolean", "description": "Run detached and return immediately." },
                    "cwd": { "type": "string", "description": "Working directory to run the command in. Omit it ONLY for your own scratch/throwaway work — it then defaults to a private temp scratch folder, never a real project. Always pass the real absolute folder path when the task is about actual files (a project, the user's Desktop, etc.)." },
                    "timeout_seconds": { "type": "integer", "description": "How long to wait for foreground output before giving up, 1-300 (default 45). Ignored when background=true." }
                },
                "required": ["command"]
            }
        },
        {
            "name": "read_file",
            "description": "Read a UTF-8 text file from disk and return its contents. A relative path resolves inside your private scratch folder — pass an absolute path for a real file the user cares about.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        },
        {
            "name": "write_file",
            "description": "Create or overwrite a text file on disk. Only use this when the task genuinely needs a persisted file — never as a substitute for a tool that already exists (e.g. don't write a script to do what web_search/web_fetch/run_command can do directly). A relative path resolves inside your private scratch folder (fine for your own working files — delete_file it once you're done, unless the user actually wants it kept); pass an absolute path — the user's project, Desktop, wherever they mean — for anything they should end up seeing.",
            "parameters": {
                "type": "object",
                "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
                "required": ["path", "content"]
            }
        },
        {
            "name": "delete_file",
            "description": "Delete a file. Use this to clean up your own scratch files once you've used them and no longer need them — never on a file you haven't verified is your own throwaway work.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        },
        {
            "name": "list_directory",
            "description": "List the files and folders in a directory. A relative path resolves inside your private scratch folder.",
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
            "description": "Get the cursor position, the window under the cursor, the window that ACTUALLY has keyboard focus (these can differ — type_text/press_key always go to the focused one, not necessarily whatever's under the cursor), and the folder/element under the cursor.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "list_windows",
            "description": "List currently open top-level windows (title + process name, with the currently-focused one flagged). Use this to check whether a browser or app is already open before deciding to launch a new one.",
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

// ── Phase rollups (Cursor-style "Explored 12 files, 3 searches +367 -14") ──
//
// The raw step-by-step tool log (see `.agent-step` in the UI) is accurate but
// unreadable for a long run. Between each narration ("thinking") message the
// model sends, this tallies what its tool calls actually did, so a burst of
// 20 read_file/list_directory calls collapses into one line instead of 20.

#[derive(Default, Clone)]
struct PhaseCounts {
    files_touched: u32,
    searches: u32,
    edited: u32,
    commands: u32,
    actions: u32,
    lines_added: u32,
    lines_removed: u32,
    has_diff_stats: bool,
}

impl PhaseCounts {
    fn is_empty(&self) -> bool {
        self.files_touched == 0 && self.searches == 0 && self.edited == 0 && self.commands == 0 && self.actions == 0
    }

    fn record(&mut self, tool_name: &str) {
        match tool_name {
            "read_file" | "list_directory" | "find_files" | "get_screen_context" | "capture_screen"
            | "list_windows" | "focus_window" | "web_fetch" => self.files_touched += 1,
            "web_search" => self.searches += 1,
            "write_file" | "delete_file" => self.edited += 1,
            "run_command" => self.commands += 1,
            _ => self.actions += 1,
        }
    }

    fn record_diff(&mut self, added: u32, removed: u32) {
        self.lines_added += added;
        self.lines_removed += removed;
        self.has_diff_stats = true;
    }

    /// e.g. "Edited 11 files, explored 7 files, 3 searches, ran 2 commands +367 -14"
    fn summarize(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let plural = |n: u32| if n == 1 { "" } else { "s" };
        let mut parts: Vec<String> = Vec::new();
        if self.edited > 0 {
            parts.push(format!("edited {} file{}", self.edited, plural(self.edited)));
        }
        if self.files_touched > 0 {
            parts.push(format!("explored {} file{}", self.files_touched, plural(self.files_touched)));
        }
        if self.searches > 0 {
            parts.push(format!("{} search{}", self.searches, if self.searches == 1 { "" } else { "es" }));
        }
        if self.commands > 0 {
            parts.push(format!("ran {} command{}", self.commands, plural(self.commands)));
        }
        if self.actions > 0 {
            parts.push(format!("{} other action{}", self.actions, plural(self.actions)));
        }
        let mut line = parts.join(", ");
        if let Some(first) = line.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        if self.has_diff_stats {
            line.push_str(&format!(" +{} -{}", self.lines_added, self.lines_removed));
        }
        Some(line)
    }
}

/// Approximate line-level diff stats (added, removed) between old and new file
/// content — write_file always overwrites the whole file rather than patching
/// it, so this is computed after the fact via a longest-common-subsequence
/// pass, same idea as `git diff --numstat`. Returns None for pathologically
/// large files rather than spend seconds on an O(n·m) pass no one asked for.
fn line_diff_stats(old: &str, new: &str) -> Option<(u32, u32)> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines.len().saturating_mul(new_lines.len()) > 4_000_000 {
        return None;
    }
    // Classic O(n·m) LCS length via a rolling two-row DP.
    let mut prev = vec![0u32; new_lines.len() + 1];
    let mut curr = vec![0u32; new_lines.len() + 1];
    for i in 1..=old_lines.len() {
        for j in 1..=new_lines.len() {
            curr[j] = if old_lines[i - 1] == new_lines[j - 1] {
                prev[j - 1] + 1
            } else {
                prev[j].max(curr[j - 1])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    let lcs_len = prev[new_lines.len()];
    let removed = old_lines.len() as u32 - lcs_len;
    let added = new_lines.len() as u32 - lcs_len;
    Some((added, removed))
}

/// Emits the accumulated tool-call tally as one "phase-summary" event (if
/// anything happened) and resets the counter for the next phase.
fn flush_phase(app: &AppHandle, source: &str, phase: &mut PhaseCounts) {
    if let Some(summary) = phase.summarize() {
        emit(app, source, "phase-summary", summary);
    }
    *phase = PhaseCounts::default();
}

/// write_file/delete_file smuggle their diff stats through `ToolOutcome.log`
/// as a trailing `\u{1}DIFF <added> <removed>` marker (rather than adding a
/// field to `ToolOutcome`, which ~35 call sites construct as a plain struct
/// literal) — stripped back out here before the log line reaches the model
/// or the UI, neither of which should ever see the raw marker.
fn extract_diff_marker(log: &str) -> (String, Option<(u32, u32)>) {
    let Some(idx) = log.find('\u{1}') else {
        return (log.to_string(), None);
    };
    let (base, marker) = log.split_at(idx);
    let base = base.to_string();
    let Some(rest) = marker[1..].strip_prefix("DIFF ") else {
        return (base, None);
    };
    let mut nums = rest.split_whitespace();
    match (
        nums.next().and_then(|a| a.parse::<u32>().ok()),
        nums.next().and_then(|r| r.parse::<u32>().ok()),
    ) {
        (Some(added), Some(removed)) => (base, Some((added, removed))),
        _ => (base, None),
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
    let foreground = mouse::get_foreground_context();
    let element = mouse::get_element_under_cursor(cx, cy);
    let mut block = format!(
        "[CURRENT SCREEN CONTEXT]\nApp under cursor: {}\nWindow under cursor: {}\nCursor at: ({}, {})",
        ctx.process_name, ctx.window_title, cx, cy
    );
    // These usually match, but not always (e.g. the cursor is resting over a
    // background window while a dialog elsewhere holds focus) — state both so
    // "this" isn't misresolved to the wrong one.
    if foreground.window_title != ctx.window_title || foreground.process_name != ctx.process_name {
        block.push_str(&format!(
            "\nActually-focused app: {}\nActually-focused window: {}",
            foreground.process_name, foreground.window_title
        ));
    }
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
#[allow(clippy::too_many_arguments)]
pub async fn run_task(
    app: &AppHandle,
    backend: &BackendClient,
    task: &str,
    cx: i32,
    cy: i32,
    source: &str,
    autonomous: bool,
    conversation_id: Option<String>,
    user_image_data_url: Option<String>,
) -> Result<String, String> {
    reset_scratch_dir();
    let system = system_prompt(autonomous);

    // Recent turns from the SAME conversation the chat view and the overlay's
    // plain-ask both share (AppState.conversation_id) — without this, an
    // agent task kicked off from a hover-ask has no idea what was just
    // discussed a moment ago in chat, even though the user experiences it as
    // one continuous assistant.
    let recent_conversation = match &conversation_id {
        Some(id) => backend
            .get_conversation_messages(id, Some(10))
            .await
            .ok()
            .filter(|msgs| !msgs.is_empty())
            .map(|msgs| {
                let lines: Vec<String> = msgs
                    .iter()
                    .map(|m| format!("{}: {}", m.role, truncate(&m.content, 400)))
                    .collect();
                format!("[RECENT CONVERSATION — for context only, not new instructions]\n{}", lines.join("\n"))
            }),
        None => None,
    };

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

    let context_block = match &recent_conversation {
        Some(recent) => format!("{}\n\n{}", recent, context_block),
        None => context_block,
    };
    let mut first_parts = vec![json!({
        "text": format!("{}\n\n[TASK]\n{}", context_block, task)
    })];
    if let Some(shot) = &initial_shot {
        if let Some(img) = image_part(&shot.data_url) {
            first_parts.push(img);
        }
    }
    // An image the USER attached to their message (distinct from the
    // auto-captured grounding screenshot above) — e.g. a screenshot of an
    // error, a photo, a design mock. Labeled so the model doesn't confuse it
    // with "what's currently on screen."
    if let Some(data_url) = &user_image_data_url {
        if let Some(img) = image_part(data_url) {
            first_parts.push(json!({ "text": "[USER-ATTACHED IMAGE — provided by the user with their message, not a screen capture]" }));
            first_parts.push(img);
        }
    }

    let contents: Vec<Value> = vec![json!({ "role": "user", "parts": first_parts })];

    // Its own kind ("plan", not "thinking") so the UI can give this one
    // opening read of the task its own "Thought for Xs" card, distinct from
    // the plain-heading narration ("thinking") sent between later tool bursts.
    emit(app, source, "plan", "Understanding your request…");

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
//
// Rate limits (429s, from our own Upstash per-route limiter or a Gemini quota
// error) are just as transient as a 5xx — the window clears in seconds — so
// they're retried the same way instead of failing the whole run outright.
const STEP_RETRY_ATTEMPTS: usize = 4;
const STEP_RETRY_BASE_DELAY_SECS: u64 = 3;
const STEP_RETRY_MAX_DELAY_SECS: u64 = 30;

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
        "(429)",
        "429",
        "too many requests",
        "rate limit",
        "(502)",
        "(503)",
        "(500)",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Pulls a server-suggested delay out of an error string shaped like
/// `"...[retry-after=7]..."` (see `backend.rs::agent_step`), so a rate-limited
/// step waits exactly as long as the server asked instead of guessing.
fn retry_after_hint_secs(err: &str) -> Option<u64> {
    let start = err.find("[retry-after=")? + "[retry-after=".len();
    let rest = &err[start..];
    let end = rest.find(']')?;
    rest[..end].trim().parse::<u64>().ok()
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
                let backoff = STEP_RETRY_BASE_DELAY_SECS * (1 << (attempt - 1)); // 3s, 6s, 12s, 24s
                let delay = retry_after_hint_secs(&e)
                    .map(|hint| hint.clamp(1, STEP_RETRY_MAX_DELAY_SECS))
                    .unwrap_or(backoff);
                let reason = if e.to_lowercase().contains("429")
                    || e.to_lowercase().contains("rate limit")
                    || e.to_lowercase().contains("too many requests")
                {
                    "Too many requests right now"
                } else {
                    "The model is busy"
                };
                emit(
                    app,
                    source,
                    "thinking",
                    format!(
                        "{} ({}). Retrying in {}s… ({}/{})",
                        reason,
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
    // Tallies tool-call activity between narration messages — flushed as a
    // "phase-summary" event right before the next "thinking" line (or at the
    // end of the run), so the UI can show "Explored 12 files, 3 searches"
    // instead of 12 separate raw tool-call rows.
    let mut phase = PhaseCounts::default();

    for _ in 0..MAX_ITERATIONS {
        // Bound the payload: only the 2 most recent screenshots ride along.
        prune_old_images(&mut contents, 2);
        let step = agent_step_with_retry(app, backend, system, &contents, tools, source).await?;
        let parts = step.get("parts").and_then(|p| p.as_array()).cloned().unwrap_or_default();

        // Split the model's reply into free text and tool calls, and echo the
        // model's OWN parts back verbatim into history (not a rebuilt {name,args}
        // object) — thinking-enabled Gemini models attach a sibling
        // `thoughtSignature` field to functionCall parts that MUST be replayed
        // back unchanged on the next turn, or the API 400s with "Function call is
        // missing a thought_signature". Reconstructing a stripped-down part
        // silently drops that field.
        let mut assistant_text = String::new();
        let mut calls: Vec<(String, Value)> = Vec::new();
        let mut model_parts: Vec<Value> = Vec::new();

        for part in &parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                if !t.trim().is_empty() {
                    assistant_text.push_str(t);
                    model_parts.push(part.clone());
                }
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                model_parts.push(part.clone());
                calls.push((name, args));
            }
        }

        if model_parts.is_empty() {
            if !assistant_text.trim().is_empty() {
                flush_phase(app, source, &mut phase);
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
            flush_phase(app, source, &mut phase);
            emit(app, source, "done", "Done");
            return Ok(assistant_text);
        }

        if !assistant_text.trim().is_empty() {
            // Flush whatever the PREVIOUS burst of tool calls did before
            // showing the model's new narration — so the rollup line reads
            // as "here's what that last phase did" ahead of "here's the next
            // phase," matching the order the work actually happened in.
            flush_phase(app, source, &mut phase);
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
            let (clean_log, diff) = extract_diff_marker(&outcome.log);
            phase.record(&name);
            if let Some((added, removed)) = diff {
                phase.record_diff(added, removed);
            }
            emit(app, source, "tool-result", truncate(&clean_log, 300));
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
        "delete_file" => format!("Deleting {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
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
        "delete_file" => delete_file_tool(args),
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
    // No cwd → the agent's own scratch dir, never the host process's actual
    // cwd (which under `tauri dev` is the app's own source tree — that's how
    // throwaway scripts used to end up committed alongside real project
    // files). A real project/folder task should always pass an explicit cwd.
    let cwd = args
        .get("cwd")
        .and_then(|c| c.as_str())
        .filter(|c| !c.trim().is_empty())
        .map(|c| c.to_string())
        .unwrap_or_else(|| agent_scratch_dir().to_string_lossy().into_owned());
    let cwd = Some(cwd);
    let timeout = args
        .get("timeout_seconds")
        .and_then(|t| t.as_u64())
        .map(Duration::from_secs)
        .map(|d| d.clamp(Duration::from_secs(1), MAX_COMMAND_TIMEOUT))
        .unwrap_or(COMMAND_TIMEOUT);
    let cmd = command.clone();

    let result = tokio::task::spawn_blocking(move || run_powershell(&cmd, background, cwd.as_deref(), timeout)).await;
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

fn run_powershell(command: &str, background: bool, cwd: Option<&str>, timeout: Duration) -> Result<String, String> {
    use std::process::{Command, Stdio};

    if background {
        // Detached: fire-and-forget, don't wait for or capture output.
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", command])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.spawn().map_err(|e| format!("failed to start: {}", e))?;
        return Ok("Command started in the background.".to_string());
    }

    let mut spawn_cmd = Command::new("powershell");
    spawn_cmd
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    if let Some(dir) = cwd {
        spawn_cmd.current_dir(dir);
    }
    let child = spawn_cmd.spawn().map_err(|e| format!("failed to start: {}", e))?;

    // Enforce a wall-clock timeout so one hung command can't stall the agent.
    let out = wait_with_timeout(child, timeout)?;
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
                        "command timed out after {}s (pass a longer timeout_seconds for a slow-but-finite command, or background=true for a long-running/indefinite one)",
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
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    if raw.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    let path = resolve_agent_path(raw);
    match std::fs::read_to_string(&path) {
        Ok(content) => ToolOutcome {
            response: json!({ "content": truncate(&content, MAX_FILE_READ) }),
            image_data_url: None,
            log: format!("read {} ({} bytes)", path.display(), content.len()),
        },
        Err(e) => ToolOutcome::err(format!("could not read {}: {}", path.display(), e)),
    }
}

fn write_file_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
    if raw.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    let path = resolve_agent_path(raw);
    // Captured before the write so the activity log's "+N -M" stat reflects
    // what actually changed — None (file didn't exist yet) means every line
    // of the new content counts as added.
    let old_content = std::fs::read_to_string(&path).ok();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, content) {
        Ok(()) => {
            let scratch_note = if path.starts_with(agent_scratch_dir()) {
                " (private scratch file — delete_file it once you're done with it, unless the user asked to keep it)"
            } else {
                ""
            };
            let msg = format!("Wrote {} ({} bytes).{}", path.display(), content.len(), scratch_note);
            let mut log = msg.clone();
            if let Some((added, removed)) = line_diff_stats(old_content.as_deref().unwrap_or(""), content) {
                log.push_str(&format!("\u{1}DIFF {} {}", added, removed));
            }
            ToolOutcome { response: json!({ "result": msg }), image_data_url: None, log }
        }
        Err(e) => ToolOutcome::err(format!("could not write {}: {}", path.display(), e)),
    }
}

fn delete_file_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    if raw.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    let path = resolve_agent_path(raw);
    // Same "captured before it's gone" idea as write_file_tool, so a delete
    // shows up as "-N" lines removed instead of silently not counting.
    let old_content = std::fs::read_to_string(&path).ok();
    match std::fs::remove_file(&path) {
        Ok(()) => {
            let msg = format!("Deleted {}.", path.display());
            let mut log = msg.clone();
            if let Some(old) = old_content {
                if let Some((_, removed)) = line_diff_stats(&old, "") {
                    log.push_str(&format!("\u{1}DIFF 0 {}", removed));
                }
            }
            ToolOutcome { response: json!({ "result": msg }), image_data_url: None, log }
        }
        Err(e) => ToolOutcome::err(format!("could not delete {}: {}", path.display(), e)),
    }
}

fn list_directory_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    if raw.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    let path = resolve_agent_path(raw);
    match std::fs::read_dir(&path) {
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
                log: format!("{} items in {}", count, path.display()),
            }
        }
        Err(e) => ToolOutcome::err(format!("could not list {}: {}", path.display(), e)),
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
        let foreground = mouse::get_foreground_context();
        let element = mouse::get_element_under_cursor(cx, cy);
        // Reported separately because they can differ (cursor resting over a
        // background window while another has focus) — acting via type_text/
        // press_key always goes to the FOREGROUND window, not whatever's under
        // the cursor, so both need to be visible to avoid acting on the wrong one.
        let mut response = json!({
            "activeApp": ctx.process_name,
            "activeWindow": ctx.window_title,
            "foregroundApp": foreground.process_name,
            "foregroundWindow": foreground.window_title,
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
    foreground: bool,
}

#[cfg(windows)]
fn enumerate_top_windows() -> Vec<WindowInfo> {
    use winapi::shared::minwindef::{BOOL, LPARAM, TRUE};
    use winapi::shared::windef::HWND;
    use winapi::um::winuser::{
        EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible,
    };

    struct Collector {
        windows: Vec<WindowInfo>,
        foreground_hwnd: HWND,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let collector = &mut *(lparam as *mut Collector);
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
        let foreground = hwnd == collector.foreground_hwnd;
        collector.windows.push(WindowInfo { title, process_name, foreground });
        TRUE
    }

    let mut collector = Collector { windows: Vec::new(), foreground_hwnd: unsafe { GetForegroundWindow() } };
    unsafe {
        EnumWindows(Some(enum_proc), &mut collector as *mut Collector as LPARAM);
    }
    collector.windows
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
    let mut windows = tokio::task::spawn_blocking(enumerate_top_windows).await.unwrap_or_default();
    // Surface the actually-focused window first — it's the one type_text/
    // press_key would act on right now, so it's the most decision-relevant entry.
    windows.sort_by_key(|w| !w.foreground);
    let count = windows.len();
    let entries: Vec<Value> = windows
        .iter()
        .take(40)
        .map(|w| json!({ "title": w.title, "process": w.process_name, "focused": w.foreground }))
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
