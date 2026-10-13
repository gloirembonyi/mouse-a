// Mouse AI Desktop - Local Agent Loop
//
// Turns Mouse AI from "answer one question about the screen" into an agent that
// can DO things: run commands, read/write files, research the web, and drive the
// UI (click/type) to complete a whole task the user described - then report back
// to whichever window they asked from.
//
// Split of responsibility:
//   • The MODEL runs on the backend (Gemini function-calling, /api/agent/step) -
//     it only ever sees the conversation + tool results and decides the next move.
//   • Every TOOL runs HERE, locally, on the user's machine.
//
// The loop mirrors a standard agent turn: ask the model → if it returned
// functionCalls, execute them locally and feed the results back → repeat until
// it returns a plain-text answer. Around that: a progress-driven step budget,
// a completion check, loop/stall detection, history compaction and a
// "what got done / what is left" report when a run has to stop.

use crate::backend::BackendClient;
use crate::{capture, explorer, input, mcp, mouse};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

// ── Step budget ──────────────────────────────────────────────────
// A fixed 40-step cap ended long jobs that WERE working mid-way, while a run
// that was stuck burned all 40 steps before stopping. Progress is the right
// signal, not step count: the budget starts at BASE_BUDGET and grows by
// BUDGET_EXTENSION whenever it runs out while recent steps still made progress,
// up to HARD_MAX_STEPS / MAX_RUN_TIME. Stalls and loops stop the run early.
const BASE_BUDGET: usize = 40;
const BUDGET_EXTENSION: usize = 15;
const HARD_MAX_STEPS: usize = 120;
const MAX_RUN_TIME: Duration = Duration::from_secs(30 * 60);
/// Model steps with no progress signal before a "re-think" warning…
const STALL_WARN_STEPS: usize = 6;
/// …and before the run is stopped as stuck.
const STALL_STOP_STEPS: usize = 12;
/// The same call on the same state this many times = a loop (warned)…
const LOOP_REPEAT_WARN: usize = 3;
/// …and this many times with no progress = stopped.
const LOOP_REPEAT_STOP: usize = 6;
/// How many times a "done" answer can be sent back for another look.
const MAX_COMPLETION_REVIEWS: usize = 2;
// ── Context management ───────────────────────────────────────────
/// Screenshots kept in history (they're the main context bloat here).
const KEEP_IMAGES: usize = 2;
/// Tool rounds whose results stay untrimmed.
const KEEP_FULL_TOOL_ROUNDS: usize = 3;
/// Older tool results are cut to this many characters.
const SLIM_TOOL_RESULT_CHARS: usize = 1500;
/// Tool rounds kept verbatim when older ones are folded into a summary…
const COMPACT_KEEP_ROUNDS: usize = 8;
/// …which happens once this many rounds pile up…
const COMPACT_AT_ROUNDS: usize = 18;
/// …or the non-image history text gets this large.
const COMPACT_AT_CHARS: usize = 120_000;
// A clean HTTP 200 with a genuinely empty candidate (no text, no functionCall) is rare
// but does happen under load - retrying with a short nudge recovers most of these
// instead of ending the whole task on one blank turn (see drive_loop).
const MAX_EMPTY_RESPONSE_RETRIES: usize = 3;
// A single foreground command can't hold the whole task hostage.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
// A model-requested timeout_seconds is honored up to this - long enough for a
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
// paths now resolve here instead - a real OS temp folder, wiped clean at the
// start of every task, so scratch work literally cannot accumulate or land
// in the user's project regardless of what the model does.
fn agent_scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("mouse-ai-agent-scratch");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Wipes the scratch dir back to empty - called once at the start of every
/// `run_task` so a previous run's throwaway files never linger, even if the
/// model never called delete_file on them itself.
static ACTIVE_RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct RunGuard;

impl RunGuard {
    fn enter() -> Self {
        if ACTIVE_RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            reset_scratch_dir();
        }
        RunGuard
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        ACTIVE_RUNS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn reset_scratch_dir() {
    let dir = agent_scratch_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
}

/// Absolute paths are used as-is (the model explicitly means a real location
/// - the user's project, Desktop, wherever). A relative path resolves inside
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
    /// Which window asked - so the right panel renders the live log.
    pub source: String,
    pub kind: String, // "plan" | "thinking" | "phase-summary" | "tool" | "tool-result" | "done" | "error" | "confirm"
    pub message: String,
    /// Only set for kind == "confirm" - the id the UI must echo back via
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
// limit - see capture.rs), but clicks must land on real screen pixels. So the
// model is told to report click coordinates NORMALIZED to a 0-1000 scale
// relative to the screenshot it was just shown, and this maps that back to
// exact global pixels using the captured region's real origin/size. Skipping
// this (i.e. treating the model's numbers as raw screen pixels) is what makes
// clicks land on the wrong element - off by exactly the downscale ratio.
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

// ── Cancellation ─────────────────────────────────────────────────
// A long agent run used to be unstoppable short of killing the app - the UI
// showed a spinner for minutes with no way out. The Stop button sets a flag
// here (keyed by the same `source` string that routes progress events), and
// the drive loop checks it between model steps and between tool calls.
static CANCEL_REQUESTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn cancels() -> &'static Mutex<HashSet<String>> {
    CANCEL_REQUESTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Called by the `agent_cancel` Tauri command (the UI's Stop button).
pub fn request_cancel(source: &str) {
    cancels().lock().unwrap().insert(source.to_string());
    // Also cancel any web task this run delegated to the browser extension.
    crate::bridge::bridge().cancel_source(source);
}

// ── Last target window ───────────────────────────────────────────
// The window the agent last clicked or explicitly focused, kept so
// type_text/press_key can hand keyboard focus BACK to it when focus has
// drifted onto Mouse AI's own UI (typically: the user touched the overlay
// mid-run to watch the log). Without this the step fails with a corrective
// error and burns a whole model round-trip on something the app can fix
// itself in milliseconds.
static LAST_TARGET_WINDOW: Mutex<Option<(isize, String)>> = Mutex::new(None);

fn remember_target_window(hwnd: isize, label: &str) {
    if hwnd != 0 {
        *LAST_TARGET_WINDOW.lock().unwrap() = Some((hwnd, label.to_string()));
    }
}

fn last_target_window() -> Option<(isize, String)> {
    LAST_TARGET_WINDOW.lock().unwrap().clone()
}

/// Check-and-consume: returns true at most once per request_cancel call.
fn cancel_requested(source: &str) -> bool {
    cancels().lock().unwrap().remove(source)
}

/// Non-consuming peek - used while waiting between model retries so a Stop
/// cuts the wait short but is still seen (and consumed) by the loop itself.
fn cancel_pending(source: &str) -> bool {
    cancels().lock().unwrap().contains(source)
}

// The prompt is ordered the way a run unfolds - plan, pick tools, act, recover,
// verify, report - because the failure modes it guards against come in the same
// order: no plan (wanders), wrong tool (GUI-clicking what a command does in one
// step), retrying a broken move (burns the budget), and declaring victory without
// looking (the worst one, because the user believes it).
fn system_prompt(autonomous: bool) -> String {
    let mut s = [
        "You are Mouse AI, an autonomous agent running on the user's Windows PC. You complete real jobs by",
        "calling tools: shell commands, file reads/edits, web research, the browser extension, and mouse/",
        "keyboard control. Keep going until the job is actually done and verified - not until you've tried once.",
        "",
        "HOW TO WORK",
        "- The task arrives with a snapshot of the screen (focused window, what's under the cursor, a",
        "  screenshot). \"This\"/\"here\" usually means that. Read real data instead of guessing.",
        "- Plan first: for anything beyond ~3 actions, call update_plan in your FIRST response (together with",
        "  your first real action) with ordered steps and a concrete, checkable done_when (\"npm test passes\",",
        "  \"the note is visible in the list\"). Call it again when you finish a step (completed=N), change",
        "  approach, or learn a fact you'll need later (notes - they survive when old results are trimmed).",
        "- Every tool round carries a run_status (step/budget, your plan, warnings). Read it. Your step budget",
        "  grows while you make progress, so long jobs are fine - but it stops if you loop.",
        "- Independent tool calls can go in one response (e.g. read three files at once).",
        "",
        "CHOOSING TOOLS (most reliable first)",
        "1. Direct tools: read_file, edit_file, write_file, list_directory, search_in_files, find_files,",
        "   web_search, web_fetch, download_file, get_clipboard/set_clipboard, read_app_content.",
        "2. run_command (PowerShell) for anything a command can do: builds, tests, git, installs, queries.",
        "3. delegate_to_browser for interactive WEBSITE work (forms, logged-in pages, multi-step web flows).",
        "4. GUI control (click/type/press_key) only for what has no command/file/web route.",
        "Never write a script to do what a tool already does (no scraper .py when web_fetch exists).",
        "",
        "FILES & PROJECTS",
        "- If the task starts with \"[Project folder: X]\", X is the project: pass it as run_command's cwd and",
        "  build ABSOLUTE paths under it for every file tool. Relative paths go to a private scratch folder.",
        "- Find code with search_in_files (contents) / find_files (names); read_file before edit_file; change",
        "  existing files with edit_file (exact unique snippet); write_file only for new files or full rewrites.",
        "- Scratch vs. real: throwaway files (helper scripts, dumps) use relative paths and get delete_file'd",
        "  before you finish. Only write to real locations when the user wants the file kept there.",
        "",
        "TERMINAL",
        "- run_command runs PowerShell (use PowerShell syntax). Use cwd instead of cd. Output ends with",
        "  [exit code N] when the command failed - read the error and fix the cause; don't re-run it unchanged.",
        "- Slow but finite (install/build/test): pass timeout_seconds (up to 300; default 45).",
        "- Servers/watchers/anything indefinite: background=true. It returns a log file path - read_file that",
        "  log (after a short wait) to confirm it actually started and on which port before relying on it.",
        "",
        "BROWSER & WEB",
        "- Information: web_search, then web_fetch the best result. If a search returns nothing, rephrase once",
        "  with different keywords instead of repeating it.",
        "- Doing things on a website: delegate_to_browser with a self-contained instruction (site/URL and what",
        "  done looks like). Use background=true + check_browser_task to run web and local work in parallel.",
        "- Only if no extension is connected: list_windows/focus_window to reuse an open browser (else",
        "  open_url with an exact URL - never invent one), wait for the page to load, then click/type as below.",
        "",
        "UI AUTOMATION",
        "- click/double_click/scroll take x,y NORMALIZED 0-1000 relative to the MOST RECENT screenshot",
        "  (0,0 top-left; 1000,1000 bottom-right) - not pixels. Never click from a stale screenshot.",
        "- OBSERVE → ACT → VERIFY: every UI action (click, double_click, scroll, type_text, press_key, open_url,",
        "  open_app, focus_window, wait) returns a fresh screenshot. Check it before the next step. If the",
        "  expected change isn't visible, the action did NOT work - diagnose (coordinates? focus? still",
        "  loading?) instead of carrying on.",
        "- Forms: click the field, confirm focus in the screenshot, type_text, confirm your text is in THAT",
        "  field; repeat per field (Tab also works); then submit and verify from its screenshot.",
        "- list_windows shows what's open (browser/office/editor/terminal); read_app_content reads the focused",
        "  app's actual text (cells, documents, lists) - use it instead of squinting at a screenshot.",
        "",
        "SAFETY",
        "- Before sensitive or ambiguous actions (sending messages, payments/personal data, purchases,",
        "  deleting/moving user files) call request_confirmation and wait. If denied, stop that action and",
        "  ask what they'd like instead.",
        "- The first time a UI action targets a new app the user gets an automatic Allow/Deny prompt. If a",
        "  result says it was denied, treat it like a manual deny - don't retry.",
        "- Destructive system commands (formatting, wiping drives, shutdown) are refused; don't try to evade that.",
        "",
        "WHEN SOMETHING FAILS",
        "- Failed results may carry a \"hint\" - follow it. Never repeat a call that failed the same way; change",
        "  something real (path, command, approach). After two failures of one approach, switch approach.",
        "- Before giving up, try a genuinely different route (another tool, a command instead of the GUI, a",
        "  direct URL, web_search for how it's done). If truly blocked, say exactly what blocks it.",
        "",
        "FINISHING",
        "- Check the result against done_when with a fresh observation (re-run the test, read the file back,",
        "  look at the screenshot) before you claim success. Never claim something was saved/sent/fixed that",
        "  you haven't seen confirmed; an honest \"I did X but couldn't confirm Y\" beats a false \"done\".",
        "- Delete your scratch files, then stop calling tools and reply with a concise Markdown answer: what",
        "  you did, the result (with evidence), and anything left for the user.",
    ]
    .join("\n");
    if autonomous {
        s.push_str(
            "\n\nAUTONOMOUS MODE: act without asking for routine, low-risk actions. Still call \
request_confirmation before sensitive/ambiguous actions (see SAFETY), and still never run genuinely \
destructive operations (mass deletion, disk formatting, shutting down the machine).",
        );
    }
    s
}

/// Gemini functionDeclarations for every local tool the agent can call.
fn tool_declarations() -> Value {
    json!([
        {
            "name": "run_command",
            "description": "Run a Windows PowerShell command and return its output. Use background=true for long-running/servers so it doesn't block. For anything that takes more than a few seconds (installs, builds), pass timeout_seconds explicitly rather than letting it hit the default and fail - up to 300s foreground, or use background=true for anything longer/indefinite. IMPORTANT: only use this (or write_file) when no built-in tool already does the job - e.g. never write and run a script to search the web or fetch a page, web_search/web_fetch already do that directly.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The PowerShell command to run." },
                    "background": { "type": "boolean", "description": "Run detached and return immediately." },
                    "cwd": { "type": "string", "description": "Working directory to run the command in. Omit it ONLY for your own scratch/throwaway work - it then defaults to a private temp scratch folder, never a real project. Always pass the real absolute folder path when the task is about actual files (a project, the user's Desktop, etc.)." },
                    "timeout_seconds": { "type": "integer", "description": "How long to wait for foreground output before giving up, 1-300 (default 45). Ignored when background=true." }
                },
                "required": ["command"]
            }
        },
        {
            "name": "read_file",
            "description": "Read a UTF-8 text file from disk and return its contents. A relative path resolves inside your private scratch folder - pass an absolute path for a real file the user cares about.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        },
        {
            "name": "write_file",
            "description": "Create or overwrite a text file on disk. Only use this when the task genuinely needs a persisted file - never as a substitute for a tool that already exists (e.g. don't write a script to do what web_search/web_fetch/run_command can do directly). A relative path resolves inside your private scratch folder (fine for your own working files - delete_file it once you're done, unless the user actually wants it kept); pass an absolute path - the user's project, Desktop, wherever they mean - for anything they should end up seeing.",
            "parameters": {
                "type": "object",
                "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
                "required": ["path", "content"]
            }
        },
        {
            "name": "delete_file",
            "description": "Delete a file. Use this to clean up your own scratch files once you've used them and no longer need them - never on a file you haven't verified is your own throwaway work.",
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
            "description": "Get the cursor position, the window under the cursor, the window that ACTUALLY has keyboard focus (these can differ - type_text/press_key always go to the focused one, not necessarily whatever's under the cursor), and the folder/element under the cursor.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "list_windows",
            "description": "List currently open top-level windows (title + process name + a coarse category - browser/office/editor/terminal/other - with the currently-focused one flagged). Use this to check whether a browser or app is already open (and what kind) before deciding to launch a new one.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "read_app_content",
            "description": "Read visible text/values out of whichever app currently has keyboard focus, via its accessibility tree - generalizes beyond File Explorer (which has its own real directory listing) to spreadsheets, documents, editors, and anything else that exposes accessible content, e.g. Excel cell values, a text editor's document, a list of emails. Use this instead of guessing from a screenshot alone when you need to know what's actually IN an open app.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "focus_window",
            "description": "Bring an already-open window to the foreground by a case-insensitive substring match on its title or process name (e.g. 'chrome', 'comet', 'notepad'). Prefer this over open_url/open_app when list_windows shows the window already exists - reusing it is more reliable than opening a duplicate. The result includes an after-screenshot - confirm the right window is now in front.",
            "parameters": {
                "type": "object",
                "properties": { "title_contains": { "type": "string", "description": "Substring to match against window titles/process names." } },
                "required": ["title_contains"]
            }
        },
        {
            "name": "click",
            "description": "Move the mouse to a point and click. x/y are normalized 0-1000 relative to the most recent screenshot's width/height (0,0=top-left, 1000,1000=bottom-right) - always base these on the LATEST screenshot. The result automatically includes a fresh after-screenshot: inspect it to verify the click did what you intended.",
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
            "description": "Type the given text at the current focus (e.g. into a focused form field). The result automatically includes a fresh after-screenshot: verify YOUR text actually appeared in the RIGHT field before moving on.",
            "parameters": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
        },
        {
            "name": "press_key",
            "description": "Press a key or combo, e.g. 'enter', 'tab', 'ctrl+c', 'ctrl+shift+t', 'alt+tab'. The result automatically includes a fresh after-screenshot - verify the effect from it.",
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
            "description": "Scroll the mouse wheel at a point. x/y are normalized 0-1000 relative to the most recent screenshot, same as click. Positive amount scrolls up, negative scrolls down (a few notches per call). The result includes an after-screenshot showing what's now visible.",
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
            "description": "Open a URL in the user's default browser (use this to start a browser task before clicking/typing). The result includes an after-screenshot taken ~2s later - check whether the page has actually loaded in it before acting.",
            "parameters": { "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }
        },
        {
            "name": "open_app",
            "description": "Launch an application or open a file/folder by path or name (e.g. 'notepad', 'C:/Users/me/Desktop'). The result includes an after-screenshot taken ~2s later - check the app actually opened in it.",
            "parameters": { "type": "object", "properties": { "target": { "type": "string" } }, "required": ["target"] }
        },
        {
            "name": "wait",
            "description": "Pause briefly to let an app or web page finish loading. The result includes a fresh screenshot taken after the wait, so you can see the new state without a separate capture_screen call.",
            "parameters": { "type": "object", "properties": { "seconds": { "type": "number" } }, "required": ["seconds"] }
        },
        {
            "name": "edit_file",
            "description": "Change part of an existing text file by replacing an exact snippet - much faster and safer than rewriting the whole file with write_file. old_text must match the file exactly (including indentation) and be unique unless replace_all is true. Read the file first.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string", "description": "Exact text currently in the file." },
                    "new_text": { "type": "string", "description": "Replacement text." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence instead of requiring a unique match." }
                },
                "required": ["path", "old_text", "new_text"]
            }
        },
        {
            "name": "search_in_files",
            "description": "Search the CONTENTS of text files under a folder (like grep): returns matching lines as path:line: text. Skips node_modules, .git, target, dist and large/binary files. Use it to find where something is defined or used in a project.",
            "parameters": {
                "type": "object",
                "properties": {
                    "folder": { "type": "string", "description": "Absolute folder to search." },
                    "query": { "type": "string", "description": "Text to find (case-insensitive)." },
                    "extension": { "type": "string", "description": "Optional file extension filter, e.g. 'ts' or 'py'." }
                },
                "required": ["folder", "query"]
            }
        },
        {
            "name": "get_clipboard",
            "description": "Read the text currently on the user's clipboard.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "set_clipboard",
            "description": "Put text on the user's clipboard so they can paste it anywhere.",
            "parameters": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
        },
        {
            "name": "download_file",
            "description": "Download a URL straight to a file on disk (images, PDFs, datasets…). Faster and more reliable than doing it through a browser.",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "path": { "type": "string", "description": "Absolute destination file path." }
                },
                "required": ["url", "path"]
            }
        },
        {
            "name": "notify",
            "description": "Show the user a Windows notification - use it when a long task finishes or needs their attention while they may be in another app.",
            "parameters": {
                "type": "object",
                "properties": { "title": { "type": "string" }, "message": { "type": "string" } },
                "required": ["title", "message"]
            }
        },
        {
            "name": "request_confirmation",
            "description": "Show the user a real Allow/Deny button and WAIT for their decision before proceeding. Call this before sensitive or ambiguous actions: submitting a form with personal/payment info, sending a message, making a purchase, deleting/moving files, or anything you're not confident the user wants.",
            "parameters": {
                "type": "object",
                "properties": { "message": { "type": "string", "description": "A short, specific description of exactly what you're about to do." } },
                "required": ["message"]
            }
        },
        {
            "name": "delegate_to_browser",
            "description": "Delegate a WEB task to the connected Thothai browser extension, which drives the page through the DOM (far more reliable than clicking a browser by screen coordinates). Use this for anything on a website: filling and submitting a form, reading/extracting content from a page, searching a site, clicking through a web flow. By default this waits for the browser to finish and returns its result. Set background=true to fire it off and keep working locally, then call check_browser_task later to collect the result.",
            "parameters": {
                "type": "object",
                "properties": {
                    "instruction": { "type": "string", "description": "A clear, self-contained description of what to do on the web, including the target site/URL and what 'done' looks like." },
                    "url": { "type": "string", "description": "Optional starting URL for the browser to open first." },
                    "background": { "type": "boolean", "description": "If true, return a task_id immediately instead of waiting; collect the result later with check_browser_task." }
                },
                "required": ["instruction"]
            }
        },
        {
            "name": "check_browser_task",
            "description": "Wait for and collect the result of a browser task previously started with delegate_to_browser(background=true). Call this when you've finished the local work you could do in parallel and now need the browser's result.",
            "parameters": {
                "type": "object",
                "properties": { "task_id": { "type": "string", "description": "The task_id returned by delegate_to_browser." } },
                "required": ["task_id"]
            }
        },
        {
            // Handled by the loop itself (see `Ledger::apply_plan_update`), never
            // by execute_tool: it touches nothing. It's how the plan, definition
            // of done and key facts survive context compaction, and what the
            // completion check measures a "done" claim against.
            "name": "update_plan",
            "description": "Record your plan and progress (touches nothing on the PC). Call it in your FIRST response for any task needing more than ~3 actions - ordered steps plus a concrete, checkable done_when - then again whenever you finish a step (completed=N), change approach, or learn something you must not forget (notes survive even when old tool results are trimmed). Call it alongside your next real action, not on its own.",
            "parameters": {
                "type": "object",
                "properties": {
                    "plan": { "type": "array", "items": { "type": "string" }, "description": "Ordered steps (send the full list whenever it changes)." },
                    "done_when": { "type": "string", "description": "Concrete, checkable definition of done, e.g. 'npm test exits 0' or 'the new note is visible in the list'." },
                    "completed": { "type": "integer", "description": "How many plan steps are fully done so far." },
                    "notes": { "type": "string", "description": "Key facts/data gathered that you'll need later (paths, ports, values, URLs)." }
                }
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
            | "list_windows" | "focus_window" | "web_fetch" | "read_app_content" => self.files_touched += 1,
            "web_search" | "search_in_files" => self.searches += 1,
            "write_file" | "edit_file" | "delete_file" | "download_file" => self.edited += 1,
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
/// content - write_file always overwrites the whole file rather than patching
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
/// literal) - stripped back out here before the log line reaches the model
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
/// running them. This is a coarse safety net, not a sandbox - it blocks the
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
    // background window while a dialog elsewhere holds focus) - state both so
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
    // Runs can overlap (a browser-requested task alongside a chat task), and
    // they share the scratch dir - only the first concurrent run wipes it.
    let _run_guard = RunGuard::enter();
    // A stale Stop from a previous run must not instantly kill this one, and a
    // window remembered from a previous run is not this task's target.
    let _ = cancels().lock().unwrap().remove(source);
    *LAST_TARGET_WINDOW.lock().unwrap() = None;
    let system = system_prompt(autonomous);

    // Recent turns from the SAME conversation the chat view and the overlay's
    // plain-ask both share (AppState.conversation_id) - without this, an
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
                format!("[RECENT CONVERSATION - for context only, not new instructions]\n{}", lines.join("\n"))
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
    // auto-captured grounding screenshot above) - e.g. a screenshot of an
    // error, a photo, a design mock. Labeled so the model doesn't confuse it
    // with "what's currently on screen."
    if let Some(data_url) = &user_image_data_url {
        if let Some(img) = image_part(data_url) {
            first_parts.push(json!({ "text": "[USER-ATTACHED IMAGE - provided by the user with their message, not a screen capture]" }));
            first_parts.push(img);
        }
    }

    let contents: Vec<Value> = vec![json!({ "role": "user", "parts": first_parts })];

    // Its own kind ("plan", not "thinking") so the UI can give this one
    // opening read of the task its own "Thought for Xs" card, distinct from
    // the plain-heading narration ("thinking") sent between later tool bursts.
    emit(app, source, "plan", "Understanding your request…");

    // While the agent runs, the overlay must never seize keyboard focus - the
    // user watching/scrolling the live log used to pull focus onto Mouse AI's
    // own window, sending the agent's keystrokes into its own UI instead of
    // the target app. WS_EX_NOACTIVATE keeps the overlay visible and clickable
    // (Stop still works) but unfocusable until the run ends.
    let _ = tokio::task::spawn_blocking(|| mouse::set_own_window_no_activate("mouse ai overlay", true)).await;

    // Drive the loop, then always shut the MCP servers down (whatever the outcome).
    let result =
        drive_loop(app, backend, &system, &tools, contents, source, task, &mut mcp, &mut screen_mapping).await;
    mcp.shutdown().await;
    let _ = tokio::task::spawn_blocking(|| mouse::set_own_window_no_activate("mouse ai overlay", false)).await;
    result
}

// ── Model-call resilience ────────────────────────────────────────
// Gemini occasionally returns a transient "model overloaded / high demand"
// error (a 502/503 with that text), the per-route limiter returns a 429, or
// the network hiccups. None of those means the task failed - it means "ask
// again in a moment" - so the step is retried with backoff instead of losing
// all progress. A 413 means the conversation itself got too big: compact it
// hard and retry immediately.
const STEP_RETRY_ATTEMPTS: usize = 4;
const STEP_RETRY_BASE_DELAY_SECS: u64 = 3;
const STEP_RETRY_MAX_DELAY_SECS: u64 = 30;

/// HTTP status out of a backend error shaped like `"Agent step failed (503)…"`.
fn step_error_status(err: &str) -> Option<u16> {
    let start = err.find("failed (")? + "failed (".len();
    let rest = &err[start..];
    let end = rest.find(')')?;
    rest[..end].trim().parse::<u16>().ok()
}

/// A quota/billing 429 ("You exceeded your current quota…") is NOT a transient
/// rate limit - retrying it just delays an inevitable failure.
fn is_quota_exhausted(err: &str) -> bool {
    let lower = err.to_lowercase();
    ["exceeded your current quota", "check your plan and billing", "quota_exceeded", "resource_exhausted"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn is_payload_too_large(err: &str) -> bool {
    if step_error_status(err) == Some(413) {
        return true;
    }
    let lower = err.to_lowercase();
    lower.contains("payload too large") || lower.contains("request entity too large") || lower.contains("body exceeded")
}

fn is_retryable_step_error(err: &str) -> bool {
    if is_quota_exhausted(err) {
        return false;
    }
    let lower = err.to_lowercase();
    if lower.contains("session expired") || lower.contains("not signed in") {
        return false;
    }
    const TRANSIENT_WORDING: [&str; 4] = ["high demand", "overloaded", "unavailable", "please try again later"];
    if let Some(status) = step_error_status(err) {
        if status == 408 || status == 429 || status >= 500 || status == 413 {
            return true;
        }
        // Other 4xx are request problems; only overload wording is worth a retry.
        return TRANSIENT_WORDING.iter().any(|n| lower.contains(n));
    }
    // No status: a network-level failure (see backend.rs friendly_network_error)
    // or a response body cut off mid-flight.
    TRANSIENT_WORDING.iter().any(|n| lower.contains(n))
        || [
            "network error",
            "reach the thothai backend",
            "econnreset",
            "connection",
            "timed out",
            "timeout",
            "too many requests",
            "rate limit",
            "error sending request",
            "failed to parse agent step response",
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

/// Calls the model, riding out transient failures (backoff, honoring
/// retry-after) and compacting the history once if it's too large to send.
/// Returns early (with an error) if the user presses Stop during a wait.
async fn agent_step_with_retry(
    app: &AppHandle,
    backend: &BackendClient,
    system: &str,
    contents: &mut Vec<Value>,
    tools: &Value,
    source: &str,
    ledger: &Ledger,
) -> Result<Value, String> {
    let mut attempt = 0usize;
    let mut compacted_for_size = false;
    loop {
        match backend.agent_step(system, Value::Array(contents.clone()), tools.clone()).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if is_payload_too_large(&e) && !compacted_for_size {
                    compacted_for_size = true;
                    emit(app, source, "thinking", "The conversation got too large - compacting older steps and retrying…");
                    prune_old_images(contents, 1);
                    slim_old_tool_results(contents, 1);
                    compact_history(contents, &ledger.digest(30), true);
                    continue;
                }
                if attempt >= STEP_RETRY_ATTEMPTS || !is_retryable_step_error(&e) {
                    return Err(e);
                }
                attempt += 1;
                let backoff = STEP_RETRY_BASE_DELAY_SECS * (1 << (attempt - 1)); // 3s, 6s, 12s, 24s
                let delay = retry_after_hint_secs(&e)
                    .map(|hint| hint.clamp(1, STEP_RETRY_MAX_DELAY_SECS))
                    .unwrap_or(backoff);
                let lower = e.to_lowercase();
                let reason = if step_error_status(&e) == Some(429)
                    || lower.contains("rate limit")
                    || lower.contains("too many requests")
                {
                    "Too many requests right now"
                } else if step_error_status(&e).is_none() {
                    "Connection problem"
                } else {
                    "The model is busy"
                };
                emit(
                    app,
                    source,
                    "thinking",
                    format!("{} ({}). Retrying in {}s… ({}/{})", reason, truncate(&e, 120), delay, attempt, STEP_RETRY_ATTEMPTS),
                );
                // Sleep in slices so Stop doesn't have to wait out a 24s backoff.
                let until = std::time::Instant::now() + Duration::from_secs(delay);
                while std::time::Instant::now() < until {
                    if cancel_pending(source) {
                        return Err("Stopped".to_string());
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    }
}

// ── Run ledger ───────────────────────────────────────────────────
// The run's memory outside the conversation: plan, definition of done, a
// compact step log, notes and key data. It feeds the run_status every tool
// round carries, the summary that replaces old rounds on compaction, the
// completion check, and the report written when a run has to stop.

const LEDGER_LOG_KEEP: usize = 40;
const LEDGER_FACTS_KEEP: usize = 12;
const LEDGER_NOTES_MAX: usize = 3000;

struct LogEntry {
    n: usize,
    what: String,
    ok: bool,
    note: Option<String>,
}

#[derive(Default)]
struct Ledger {
    plan: Vec<String>,
    done_when: String,
    plan_done: usize,
    notes: String,
    log: std::collections::VecDeque<LogEntry>,
    older_steps: usize,
    facts: std::collections::VecDeque<String>,
    /// Model steps taken so far.
    steps: usize,
    /// Real tool calls (update_plan excluded).
    actions: usize,
    /// Successful state-changing tool calls - the evidence a "done" claim needs.
    effects: usize,
    errors: usize,
}

/// Short clip with a plain ellipsis (unlike `truncate`, which annotates the
/// total length - right for tool output, noisy for one-line labels).
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let kept: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", kept.trim_end())
    } else {
        s.to_string()
    }
}

impl Ledger {
    /// Applies an `update_plan` call. Returns true if it marked more steps done.
    fn apply_plan_update(&mut self, args: &Value) -> bool {
        let before = self.plan_done;
        if let Some(items) = args.get("plan").and_then(|p| p.as_array()) {
            let plan: Vec<String> = items
                .iter()
                .filter_map(|p| p.as_str())
                .map(|p| clip(p.trim(), 200))
                .filter(|p| !p.is_empty())
                .take(20)
                .collect();
            if !plan.is_empty() {
                self.plan = plan;
            }
        }
        if let Some(d) = args.get("done_when").and_then(|d| d.as_str()) {
            if !d.trim().is_empty() {
                self.done_when = clip(d.trim(), 300);
            }
        }
        if let Some(c) = args.get("completed").and_then(|c| c.as_f64()) {
            if c.is_finite() {
                self.plan_done = c.max(0.0).floor() as usize;
            }
        }
        self.plan_done = self.plan_done.min(self.plan.len());
        if let Some(n) = args.get("notes").and_then(|n| n.as_str()) {
            // Notes accumulate (newest last) - models send deltas more often
            // than totals. When over the cap, the OLDEST notes go first.
            let add = n.trim();
            if !add.is_empty() && !self.notes.contains(add) {
                if !self.notes.is_empty() {
                    self.notes.push('\n');
                }
                self.notes.push_str(add);
                let len = self.notes.chars().count();
                if len > LEDGER_NOTES_MAX {
                    self.notes = self.notes.chars().skip(len - LEDGER_NOTES_MAX).collect();
                }
            }
        }
        self.plan_done > before
    }

    fn plan_lines(&self) -> Vec<String> {
        self.plan
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mark = if i < self.plan_done { "✓" } else if i == self.plan_done { "→" } else { "·" };
                format!("{} {}. {}", mark, i + 1, clip(p, 140))
            })
            .collect()
    }

    /// One-line plan for the UI, e.g. "Plan: ✓ Read config → Fix bug · Run tests".
    fn plan_summary_line(&self) -> String {
        let items: Vec<String> = self
            .plan
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mark = if i < self.plan_done { "✓" } else if i == self.plan_done { "→" } else { "·" };
                format!("{} {}", mark, clip(p, 48))
            })
            .collect();
        format!("Plan: {}", items.join(" "))
    }

    fn record(&mut self, what: &str, ok: bool, note: Option<String>) {
        self.log.push_back(LogEntry { n: self.steps, what: clip(what, 90), ok, note });
        while self.log.len() > LEDGER_LOG_KEEP {
            self.log.pop_front();
            self.older_steps += 1;
        }
    }

    fn record_fact(&mut self, text: String) {
        self.facts.push_back(clip(&text, 500));
        while self.facts.len() > LEDGER_FACTS_KEEP {
            self.facts.pop_front();
        }
    }

    /// Human/model-readable digest of the run so far.
    fn digest(&self, max_log: usize) -> String {
        let mut out: Vec<String> = Vec::new();
        if !self.plan.is_empty() {
            out.push(format!("Plan:\n{}", self.plan_lines().join("\n")));
        }
        if !self.done_when.is_empty() {
            out.push(format!("Done when: {}", self.done_when));
        }
        if !self.notes.is_empty() {
            out.push(format!("Notes: {}", clip(&self.notes, 1200)));
        }
        let skip = self.log.len().saturating_sub(max_log);
        let skipped = self.older_steps + skip;
        let recent: Vec<String> = self
            .log
            .iter()
            .skip(skip)
            .map(|e| {
                format!(
                    "{}. {}{}{}",
                    e.n,
                    e.what,
                    if e.ok { "" } else { " - FAILED" },
                    e.note.as_ref().map(|n| format!(" ({})", n)).unwrap_or_default()
                )
            })
            .collect();
        if !recent.is_empty() {
            let head = if skipped > 0 { format!("Steps taken ({} earlier steps not listed):", skipped) } else { "Steps taken:".to_string() };
            out.push(format!("{}\n{}", head, recent.join("\n")));
        }
        if !self.facts.is_empty() {
            let facts: Vec<String> = self.facts.iter().map(|f| format!("- {}", f)).collect();
            out.push(format!("Data collected:\n{}", facts.join("\n")));
        }
        out.join("\n\n")
    }
}

/// Tools whose success means something on the PC (or a website) actually
/// changed - the evidence a "created / fixed / sent" claim needs behind it.
fn is_effect_tool(name: &str) -> bool {
    matches!(
        name,
        "write_file"
            | "edit_file"
            | "delete_file"
            | "download_file"
            | "run_command"
            | "click"
            | "double_click"
            | "type_text"
            | "press_key"
            | "open_url"
            | "open_app"
            | "set_clipboard"
            | "delegate_to_browser"
            | "check_browser_task"
    )
}

/// Tools whose value is the information they return (new info = progress).
fn is_data_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "list_directory"
            | "find_files"
            | "search_in_files"
            | "web_search"
            | "web_fetch"
            | "read_app_content"
            | "get_screen_context"
            | "list_windows"
            | "get_clipboard"
            | "delegate_to_browser"
            | "check_browser_task"
            | "run_command"
    )
}

/// Legitimately repetitive tools - left to the stall limit, never counted as a loop.
fn is_loop_exempt(name: &str) -> bool {
    matches!(name, "wait" | "check_browser_task" | "request_confirmation" | "update_plan")
}

/// The text a tool reported (error message, command output or result).
fn response_text(response: &Value) -> String {
    for key in ["error", "output", "result"] {
        if let Some(v) = response.get(key) {
            return v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
        }
    }
    response.to_string()
}

/// A tool "failed" if it returned an error - or, for run_command, if the
/// command itself exited non-zero (run_powershell appends `[exit code N]`).
fn tool_failed(name: &str, response: &Value) -> bool {
    if response.get("error").is_some() {
        return true;
    }
    if name == "run_command" {
        if let Some(out) = response.get("output").and_then(|o| o.as_str()) {
            let tail = out.trim_end();
            if let Some(idx) = tail.rfind("[exit code ") {
                return tail.ends_with(']') && !tail[idx..].starts_with("[exit code 0]");
            }
        }
    }
    false
}

/// Recovery advice attached to a failed result, keyed off the error's shape.
/// The model reads it right where it reads the failure - which is where advice
/// changes what it does next.
fn recovery_hint(name: &str, error: &str) -> Option<&'static str> {
    let e = error.to_lowercase();
    if e.contains("refused: this looks like") {
        return Some("This command is blocked as destructive. Use a narrower command that targets exactly the files meant, or ask the user to do it themselves.");
    }
    if e.contains("is not recognized as the name of a cmdlet")
        || e.contains("is not recognized as an internal or external command")
        || e.contains("commandnotfoundexception")
    {
        return Some("That program isn't installed or isn't on PATH. Check with `Get-Command <name>` / `where.exe <name>`, call it by its full path, or use an alternative (npx <tool>, py -m <module>, a winget install if the user wants it).");
    }
    if e.contains("timed out") {
        return Some("It ran past the timeout. For a slow-but-finite command pass a larger timeout_seconds (max 300); for servers/watchers use background=true and read_file the log it writes. It may have partly run - check the current state before re-running.");
    }
    if e.contains("cwd \"") && e.contains("doesn't exist") {
        return Some("Pass an existing ABSOLUTE folder as cwd - the project folder from the task's [Project folder: …] prefix, or verify the path with list_directory first.");
    }
    if e.contains("old_text was not found") {
        return Some("read_file the file again and copy old_text exactly from it (indentation included) - or use a smaller snippet that is unique in the file.");
    }
    if e.contains("old_text appears") {
        return Some("Include more surrounding lines so old_text is unique, or set replace_all=true if every occurrence should change.");
    }
    if e.contains("being used by another process") || e.contains("os error 32") {
        return Some("The file is locked by another process (often a running server, build or editor). Stop that process or work on a copy.");
    }
    if e.contains("access is denied") || e.contains("permission denied") || e.contains("os error 5") || e.contains("unauthorizedaccess") {
        return Some("Permission problem: the file may be read-only, open elsewhere, or need admin rights. Use a user-writable location or tell the user what needs elevated access.");
    }
    if e.contains("cannot find path")
        || e.contains("cannot find the path")
        || e.contains("cannot find the file")
        || e.contains("no such file")
        || e.contains("os error 2")
        || e.contains("os error 3")
        || e.contains("does not exist")
    {
        return Some("That path doesn't exist. Check the parent folder with list_directory, or locate the file with find_files (by name) / search_in_files (by content). Use absolute paths under the project folder - relative paths resolve to your private scratch folder.");
    }
    if e.contains("no browser extension is connected") {
        return Some("No browser extension is connected. For read-only info use web_search/web_fetch; to act on a site use open_url, then click/type and verify each step from the screenshots.");
    }
    if e.contains("unknown tool") {
        return Some("Use only the tools you were given.");
    }
    match name {
        "run_command" => Some("The command failed. Read the error output, fix the cause (missing dependency, wrong folder, PowerShell vs. bash syntax, bad arguments) and then re-run - don't repeat it unchanged."),
        "web_search" | "web_fetch" => Some("Try once with a different query or URL. If a site blocks fetching, delegate_to_browser can read it in a real browser."),
        "download_file" => Some("Check the URL is a direct file link (web_search for the right one), or use run_command with curl.exe -L as a fallback."),
        "click" | "double_click" | "scroll" | "type_text" | "press_key" | "focus_window" => Some("Take a fresh capture_screen, re-locate the target, or reach the same result another way (keyboard shortcut, focus_window, a command, a direct URL)."),
        _ => None,
    }
}

// ── Completion check ─────────────────────────────────────────────
// "Done!" used to be accepted at face value - a model whose last command
// failed, or that never actually wrote/clicked anything, could still close the
// task with a success message. Before accepting a final answer it's held to
// evidence (at most MAX_COMPLETION_REVIEWS times, only while budget remains).

const SUCCESS_WORDS: &[&str] = &[
    "done", "complete", "completed", "finished", "success", "successful", "successfully", "sent", "posted",
    "published", "added", "saved", "submitted", "created", "updated", "filled", "booked", "ordered", "scheduled",
    "replied", "uploaded", "all set", "installed", "fixed", "edited", "wrote", "written", "moved", "renamed",
    "deleted", "removed", "downloaded", "configured", "deployed", "committed", "resolved", "is now", "are now",
];
const FAILURE_WORDS: &[&str] = &[
    "couldn't", "could not", "couldnt", "unable", "failed", "cannot", "can't", "cant", "blocked", "not able",
    "wasn't able", "was not able", "didn't work", "did not work", "impossible", "no way to", "wasn't possible",
];
const ACTION_WORDS: &[&str] = &[
    "send", "post", "reply", "respond", "fill", "submit", "add", "create", "book", "order", "buy", "click",
    "type", "write", "upload", "delete", "remove", "sign up", "register", "apply", "schedule", "subscribe",
    "publish", "save", "update", "change", "set up", "setup", "enable", "disable", "install", "uninstall",
    "move", "rename", "edit", "fix", "run", "build", "start", "launch", "download", "copy", "commit", "deploy",
    "configure", "implement", "refactor", "make", "generate", "open", "close", "organize", "clean", "convert",
];

/// Lowercased, punctuation-free, space-padded text for whole-word phrase checks.
fn normalize_words(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push(' ');
    let mut last_space = true;
    for ch in s.chars() {
        let c = if ch == '’' { '\'' } else { ch };
        if c.is_alphanumeric() || c == '\'' {
            out.extend(c.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    if !last_space {
        out.push(' ');
    }
    out
}

fn has_any_phrase(normalized: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| normalized.contains(&format!(" {} ", p)))
}

/// The task text without the UI's "[Project folder: …]" prefix.
fn strip_project_prefix(task: &str) -> &str {
    let t = task.trim_start();
    if t.starts_with("[Project folder:") {
        if let Some(end) = t.find(']') {
            return t[end + 1..].trim_start();
        }
    }
    t
}

/// The project folder from a task shaped like
/// "[Project folder: C:\proj - work inside this folder …]\n\n<task>".
fn project_folder_from_task(task: &str) -> Option<String> {
    let rest = task.trim_start().strip_prefix("[Project folder:")?;
    let end = rest.find(']')?;
    let inner = &rest[..end];
    // The UI follows the folder with " — work inside…" (em dash or plain
    // hyphen). Cut at that phrase, not at any dash: folder names can
    // contain " - " themselves ("Project - Copy").
    let folder = [" \u{2014} work inside", " - work inside", " \u{2014} ", " -- "]
        .iter()
        .find_map(|sep| inner.find(sep).map(|i| &inner[..i]))
        .unwrap_or(inner)
        .trim();
    if folder.is_empty() {
        None
    } else {
        Some(folder.to_string())
    }
}

/// Decide whether a final answer should be accepted or the run sent back to
/// work. Returns an empty list to accept, or the concrete concerns.
fn completion_concerns(text: &str, ledger: &Ledger, task: &str, last_round_failed: bool) -> Vec<String> {
    let mut concerns = Vec::new();
    // The verdict is in the opening of an answer ("I've fixed…", "I couldn't…");
    // scanning the whole thing would match words inside quoted output.
    let head: String = text.chars().take(400).collect();
    let head = normalize_words(&head);
    let admits_failure = has_any_phrase(&head, FAILURE_WORDS);
    let claims_success = has_any_phrase(&head, SUCCESS_WORDS) && !admits_failure;

    if admits_failure {
        if ledger.steps < 15 && ledger.errors > 0 {
            concerns.push(
                "You're giving up fairly early. Before reporting failure, make sure you tried a genuinely \
different route: a different command or tool, checking the path/prerequisites, the terminal instead of the \
GUI, a direct URL, or web_search for how it's normally done."
                    .to_string(),
            );
        }
        return concerns;
    }
    if last_round_failed {
        concerns.push(
            "Your most recent tool call FAILED (error or non-zero exit code) and nothing since shows it was \
fixed - the last thing you tried did not work."
                .to_string(),
        );
    }
    if claims_success {
        let task_words = normalize_words(strip_project_prefix(task));
        if has_any_phrase(&task_words, ACTION_WORDS) && ledger.effects == 0 {
            concerns.push(
                "You're reporting the task as done, but no state-changing action (write/edit a file, a \
successful command, a click/typing, a browser task…) has succeeded in this run. If the task genuinely needed \
no change, say so explicitly; otherwise do the work."
                    .to_string(),
            );
        }
        if !ledger.plan.is_empty() && ledger.plan_done < ledger.plan.len() {
            let left: Vec<String> =
                ledger.plan.iter().skip(ledger.plan_done).map(|p| format!("\"{}\"", clip(p, 80))).collect();
            concerns.push(format!(
                "Your own plan still has {} unfinished step(s): {}. If they're actually done, verify and call \
update_plan with completed={}.",
                ledger.plan.len() - ledger.plan_done,
                left.join(", "),
                ledger.plan.len()
            ));
        }
    }
    concerns
}

// ── Progress & loop detection ────────────────────────────────────

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// A tiny grayscale thumbnail of a screenshot - enough to tell "the screen
/// changed" from "nothing happened" without keeping the image around.
const THUMB_W: u32 = 48;
const THUMB_H: u32 = 32;

fn screen_thumbnail(data_url: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let b64 = data_url.split_once(',')?.1;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let thumb = img.resize_exact(THUMB_W, THUMB_H, image::imageops::FilterType::Triangle).to_luma8();
    Some(thumb.into_raw())
}

/// Clock ticks and a blinking caret touch a cell or two; real changes touch more.
fn screens_differ(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return true;
    }
    a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 8).count() >= 3
}

#[derive(Default)]
struct ProgressTracker {
    /// Bumped whenever the world visibly changes; part of the loop key, so the
    /// same call after a real change isn't counted as a repeat.
    state_version: u64,
    call_counts: HashMap<String, usize>,
    seen_results: HashSet<u64>,
    last_screen: Option<Vec<u8>>,
    stall_steps: usize,
    last_progress_step: usize,
}

impl ProgressTracker {
    /// Counts this call against the current state; returns how many times the
    /// identical call has now been made on this state (0 for exempt tools).
    fn note_call(&mut self, name: &str, args: &Value) -> usize {
        if is_loop_exempt(name) {
            return 0;
        }
        let key = format!("{}:{}@{}", name, args, self.state_version);
        let n = self.call_counts.entry(key).or_insert(0);
        *n += 1;
        *n
    }

    /// Progress from a successful result: new data, or a state change that
    /// hasn't happened before.
    fn observe_result(&mut self, name: &str, args: &Value, response: &Value, is_mcp: bool) -> bool {
        let mut clean = response.clone();
        if let Some(obj) = clean.as_object_mut() {
            obj.remove("run_status");
            obj.remove("verification");
        }
        let progressed = if is_data_tool(name) || is_mcp {
            self.seen_results.insert(hash_str(&format!("{}:{}", name, clean)))
        } else if matches!(name, "write_file" | "edit_file" | "delete_file" | "download_file" | "set_clipboard") {
            self.seen_results.insert(hash_str(&format!("{}:{}", name, args)))
        } else {
            false
        };
        if progressed && (is_effect_tool(name) || is_mcp) {
            self.state_version += 1;
        }
        progressed
    }

    /// Returns true if this screenshot differs from the previous one.
    fn observe_screen(&mut self, thumb: Vec<u8>) -> bool {
        let changed = match &self.last_screen {
            Some(prev) => screens_differ(prev, &thumb),
            None => true,
        };
        if changed {
            self.state_version += 1;
        }
        self.last_screen = Some(thumb);
        changed
    }

    fn end_round(&mut self, progressed: bool, step: usize) {
        if progressed {
            self.stall_steps = 0;
            self.last_progress_step = step;
        } else {
            self.stall_steps += 1;
        }
    }

    fn worst_repeat(&self) -> usize {
        self.call_counts.values().copied().max().unwrap_or(0)
    }
}

// ── History management ───────────────────────────────────────────
// Gemini function calling needs each user turn of functionResponses to follow
// the model turn that issued those functionCalls. Everything here removes or
// trims whole rounds so that pairing stays valid.

fn is_tool_round(content: &Value) -> bool {
    content.get("role").and_then(|r| r.as_str()) == Some("user")
        && content
            .get("parts")
            .and_then(|p| p.as_array())
            .is_some_and(|parts| parts.iter().any(|p| p.get("functionResponse").is_some()))
}

fn tool_round_indices(contents: &[Value]) -> Vec<usize> {
    contents.iter().enumerate().filter(|(_, c)| is_tool_round(c)).map(|(i, _)| i).collect()
}

/// Size of the history without images (those are bounded separately).
fn history_text_chars(contents: &[Value]) -> usize {
    contents
        .iter()
        .filter_map(|c| c.get("parts").and_then(|p| p.as_array()))
        .flatten()
        .filter(|p| p.get("inlineData").is_none())
        .map(|p| p.to_string().len())
        .sum()
}

/// Adds a text note to the conversation as the user, merging into the last
/// user turn when there is one so roles keep alternating.
fn push_user_text(contents: &mut Vec<Value>, text: &str) {
    if let Some(last) = contents.last_mut() {
        if last.get("role").and_then(|r| r.as_str()) == Some("user") {
            if let Some(parts) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                parts.push(json!({ "text": text }));
                return;
            }
        }
    }
    contents.push(json!({ "role": "user", "parts": [{ "text": text }] }));
}

/// Only the newest round carries a run_status; older copies are noise.
fn strip_run_status(contents: &mut [Value]) {
    for c in contents.iter_mut() {
        if let Some(parts) = c.get_mut("parts").and_then(|p| p.as_array_mut()) {
            for p in parts.iter_mut() {
                if let Some(resp) = p.pointer_mut("/functionResponse/response").and_then(|r| r.as_object_mut()) {
                    resp.remove("run_status");
                }
            }
        }
    }
}

/// Trim tool results older than the last `keep_full` rounds to a preview -
/// a 20k-char file read from 10 steps ago doesn't need to ride along in full.
fn slim_old_tool_results(contents: &mut [Value], keep_full: usize) {
    let rounds = tool_round_indices(contents);
    let cutoff = rounds.len().saturating_sub(keep_full);
    for &idx in rounds.iter().take(cutoff) {
        let Some(parts) = contents[idx].get_mut("parts").and_then(|p| p.as_array_mut()) else { continue };
        for p in parts.iter_mut() {
            let Some(resp) = p.pointer_mut("/functionResponse/response") else { continue };
            if let Some(obj) = resp.as_object_mut() {
                obj.remove("verification");
                obj.remove("run_status");
                if obj.contains_key("trimmed") {
                    continue;
                }
            }
            let serialized = resp.to_string();
            if serialized.len() <= SLIM_TOOL_RESULT_CHARS {
                continue;
            }
            let slim = match resp.get("error") {
                Some(err) => json!({
                    "error": clip(&err.as_str().map(|s| s.to_string()).unwrap_or_else(|| err.to_string()), 600),
                    "trimmed": true
                }),
                None => json!({
                    "result_preview": clip(&response_text(resp), SLIM_TOOL_RESULT_CHARS),
                    "trimmed": "older result shortened to save space - re-run the tool if you need it in full"
                }),
            };
            *resp = slim;
        }
    }
}

const COMPACTION_MARKER: &str = "[EARLIER WORK - compacted";

/// Fold every tool round except the most recent ones into one summary. Rounds
/// are removed as (model call → tool response) pairs from the front, so the
/// rest stays a valid function-calling transcript; the summary rides on the
/// first user turn (the task), so roles still alternate. Returns true if it
/// compacted.
fn compact_history(contents: &mut Vec<Value>, digest: &str, force: bool) -> bool {
    let rounds = tool_round_indices(contents);
    let keep = if force { (COMPACT_KEEP_ROUNDS / 2).max(2) } else { COMPACT_KEEP_ROUNDS };
    if rounds.len() <= keep {
        return false;
    }
    if !force && rounds.len() < COMPACT_AT_ROUNDS && history_text_chars(contents) < COMPACT_AT_CHARS {
        return false;
    }
    // Cut at the model turn that issued the first round we keep.
    let mut cut = rounds[rounds.len() - keep] - 1;
    while cut > 1 && contents[cut].get("role").and_then(|r| r.as_str()) != Some("model") {
        cut -= 1;
    }
    if cut <= 1 || contents[cut].get("role").and_then(|r| r.as_str()) != Some("model") {
        return false;
    }
    contents.drain(1..cut);
    let summary = format!(
        "{} to save space. This is the authoritative record of what already happened; do not redo \
completed steps.]\n{}",
        COMPACTION_MARKER, digest
    );
    if let Some(parts) = contents[0].get_mut("parts").and_then(|p| p.as_array_mut()) {
        parts.retain(|p| !p.get("text").and_then(|t| t.as_str()).is_some_and(|t| t.starts_with(COMPACTION_MARKER)));
        parts.push(json!({ "text": summary }));
    }
    true
}

/// The run_status attached to the last result of each tool round: the model
/// sees where the run stands at the moment it decides the next move.
fn run_status(ledger: &Ledger, budget: usize, can_extend: bool, project: Option<&str>, warning: Option<String>) -> Value {
    let mut status = serde_json::Map::new();
    status.insert(
        "step".into(),
        json!(format!(
            "{} of {}{}",
            ledger.steps,
            budget,
            if can_extend { " (extends while you make progress)" } else { " (final budget)" }
        )),
    );
    if ledger.plan.is_empty() {
        status.insert("plan".into(), json!("no plan recorded - call update_plan if this takes more than a few steps"));
    } else {
        status.insert("plan".into(), json!(ledger.plan_lines()));
    }
    if !ledger.done_when.is_empty() {
        status.insert("done_when".into(), json!(ledger.done_when));
    }
    if !ledger.notes.is_empty() {
        status.insert("notes".into(), json!(clip(&ledger.notes, 400)));
    }
    if let Some(p) = project {
        status.insert("project_folder".into(), json!(p));
    }
    if let Some(w) = warning {
        status.insert("warning".into(), json!(w));
    }
    Value::Object(status)
}

/// Fallback report when the model can't write one itself.
fn ledger_report(ledger: &Ledger, reason: &str) -> String {
    let digest = ledger.digest(12);
    let mut text = format!("I stopped before finishing - {}.", reason);
    if !digest.is_empty() {
        text.push_str("\n\n");
        text.push_str(&digest);
    }
    text.push_str(CONTINUE_HINT);
    text
}

const CONTINUE_HINT: &str = "\n\n_(Say \"continue\" to pick up from here.)_";

/// Stop the run with a clear account of what happened: the model gets one
/// no-tools turn to write it (it knows the nuance); if that fails, the ledger
/// writes it.
#[allow(clippy::too_many_arguments)]
async fn stop_with_report(
    app: &AppHandle,
    backend: &BackendClient,
    system: &str,
    tools: &Value,
    contents: &mut Vec<Value>,
    source: &str,
    ledger: &Ledger,
    phase: &mut PhaseCounts,
    reason: &str,
) -> String {
    flush_phase(app, source, phase);
    emit(app, source, "thinking", format!("Wrapping up: {}", reason));
    push_user_text(
        contents,
        &format!(
            "[SYSTEM] The run is stopping now: {}. Do NOT call any tools. Reply in plain text (Markdown) \
with: (1) what you completed, with the evidence you saw; (2) what is still left to do; (3) any data or \
answers the user asked for. Be specific and brief.",
            reason
        ),
    );
    prune_old_images(contents, 1);
    let text = match agent_step_with_retry(app, backend, system, contents, tools, source, ledger).await {
        Ok(step) => step
            .get("parts")
            .and_then(|p| p.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .map(|t| t.trim())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    emit(app, source, "done", "Stopped before finishing");
    if text.is_empty() {
        ledger_report(ledger, reason)
    } else {
        format!("{}{}", text, CONTINUE_HINT)
    }
}

fn stopped_message(mid_burst: bool) -> String {
    if mid_burst {
        "⏹ Stopped at your request. I've halted the task - later steps were not run.".to_string()
    } else {
        "⏹ Stopped at your request. I've halted the task - nothing further was done.".to_string()
    }
}

/// The core agent turn loop: ask the model, execute any tool calls (built-in or
/// MCP), feed results back, repeat until it answers - with a progress-driven
/// step budget, a completion check, loop/stall detection and history
/// compaction (see the sections above).
#[allow(clippy::too_many_arguments)]
async fn drive_loop(
    app: &AppHandle,
    backend: &BackendClient,
    system: &str,
    tools: &Value,
    mut contents: Vec<Value>,
    source: &str,
    task: &str,
    mcp: &mut mcp::McpManager,
    screen: &mut Option<ScreenMapping>,
) -> Result<String, String> {
    let started = std::time::Instant::now();
    let project = project_folder_from_task(task);
    let mut ledger = Ledger::default();
    let mut progress = ProgressTracker::default();
    let mut budget = BASE_BUDGET;
    let mut empty_response_count: usize = 0;
    let mut completion_reviews: usize = 0;
    let mut last_round_failed = false;
    let mut consecutive_errors: usize = 0;
    let mut last_failed_call: Option<String> = None;
    let mut repeated_failures: usize = 0;
    // Tallies tool-call activity between narration messages - flushed as a
    // "phase-summary" event right before the next "thinking" line (or at the
    // end of the run), so the UI can show "Explored 12 files, 3 searches"
    // instead of 12 separate raw tool-call rows.
    let mut phase = PhaseCounts::default();
    // Processes the user has already approved Mouse AI controlling THIS run -
    // see `guard_target_app`. Empty at the start of every task, never persisted.
    let mut trusted_apps: HashSet<String> = HashSet::new();
    let mut iteration: usize = 0;

    loop {
        if cancel_requested(source) {
            flush_phase(app, source, &mut phase);
            emit(app, source, "done", "Stopped");
            return Ok(stopped_message(false));
        }

        // ── Budget: extend while progressing, stop when stuck or out of time.
        let time_left = started.elapsed() < MAX_RUN_TIME;
        if iteration >= budget || !time_left {
            let recent_progress = ledger.steps.saturating_sub(progress.last_progress_step) < BUDGET_EXTENSION;
            if time_left && recent_progress && budget < HARD_MAX_STEPS {
                budget = (budget + BUDGET_EXTENSION).min(HARD_MAX_STEPS);
                emit(app, source, "thinking", format!("Still making progress - continuing (step budget now {})", budget));
            } else {
                let why = if !time_left {
                    format!("it has been running for {} minutes", MAX_RUN_TIME.as_secs() / 60)
                } else if budget >= HARD_MAX_STEPS {
                    format!("it reached the maximum of {} steps", HARD_MAX_STEPS)
                } else {
                    format!(
                        "the last {} steps made no visible progress",
                        ledger.steps.saturating_sub(progress.last_progress_step)
                    )
                };
                let report =
                    stop_with_report(app, backend, system, tools, &mut contents, source, &ledger, &mut phase, &why).await;
                return Ok(report);
            }
        }
        iteration += 1;
        ledger.steps = iteration;

        prune_old_images(&mut contents, KEEP_IMAGES);
        let step = match agent_step_with_retry(app, backend, system, &mut contents, tools, source, &ledger).await {
            Ok(step) => step,
            Err(e) => {
                if cancel_requested(source) {
                    flush_phase(app, source, &mut phase);
                    emit(app, source, "done", "Stopped");
                    return Ok(stopped_message(false));
                }
                // Nothing done yet: surface the error as before. Otherwise keep
                // the work - report it so "continue" can pick it up.
                if ledger.actions == 0 {
                    return Err(e);
                }
                flush_phase(app, source, &mut phase);
                emit(app, source, "error", truncate(&e, 200));
                emit(app, source, "done", "Stopped before finishing");
                return Ok(ledger_report(&ledger, &format!("the AI service kept failing ({})", clip(&e, 160))));
            }
        };
        let parts = step.get("parts").and_then(|p| p.as_array()).cloned().unwrap_or_default();

        // Split the model's reply into free text and tool calls, and echo the
        // model's OWN parts back verbatim into history (not a rebuilt {name,args}
        // object) - thinking-enabled Gemini models attach a sibling
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
            // A clean response with no text and no functionCall - Gemini occasionally
            // returns this under load even after the backend's own retry/key-rotation.
            // Nudge it to continue instead of ending the whole task on one blank turn.
            empty_response_count += 1;
            if empty_response_count > MAX_EMPTY_RESPONSE_RETRIES {
                if ledger.actions == 0 {
                    return Err(
                        "The model kept returning empty responses. This can happen under heavy load - \
wait a moment and try again, or try a smaller/more specific task."
                            .to_string(),
                    );
                }
                flush_phase(app, source, &mut phase);
                emit(app, source, "done", "Stopped before finishing");
                return Ok(ledger_report(&ledger, "the AI kept returning empty responses"));
            }
            emit(
                app,
                source,
                "thinking",
                format!(
                    "The model returned an empty response - retrying ({}/{})…",
                    empty_response_count, MAX_EMPTY_RESPONSE_RETRIES
                ),
            );
            push_user_text(
                &mut contents,
                "[SYSTEM NOTE] Your last turn had no content (no text, no tool call). Please continue: call \
a tool if you need more information, or give a plain-text final answer if the task is already done.",
            );
            continue;
        }
        empty_response_count = 0;
        contents.push(json!({ "role": "model", "parts": model_parts }));

        // ── No tool calls → a final answer, held to evidence first.
        if calls.is_empty() {
            let concerns = completion_concerns(&assistant_text, &ledger, task, last_round_failed);
            if !concerns.is_empty() && completion_reviews < MAX_COMPLETION_REVIEWS && iteration + 1 < budget {
                completion_reviews += 1;
                flush_phase(app, source, &mut phase);
                emit(app, source, "thinking", "Checking the result before finishing…");
                let mut note = String::from("[SYSTEM] Completion check before you finish.\n");
                if !ledger.done_when.is_empty() {
                    note.push_str(&format!("Definition of done: {}\n", ledger.done_when));
                }
                note.push_str(&format!("Concerns:\n- {}\n\n", concerns.join("\n- ")));
                note.push_str(
                    "If the goal really is met, confirm it with a fresh observation (re-run the check, \
read_file the result, capture_screen) and then give your final answer. If it isn't met, keep working now - \
you still have steps left. If it truly can't be done, say exactly what blocks it.",
                );
                push_user_text(&mut contents, &note);
                continue;
            }
            flush_phase(app, source, &mut phase);
            emit(app, source, "done", "Done");
            return Ok(assistant_text);
        }

        if !assistant_text.trim().is_empty() {
            // Flush whatever the PREVIOUS burst of tool calls did before
            // showing the model's new narration - so the rollup line reads
            // as "here's what that last phase did" ahead of "here's the next
            // phase," matching the order the work actually happened in.
            flush_phase(app, source, &mut phase);
            emit(app, source, "thinking", truncate(&assistant_text, 400));
        }

        // ── Execute every requested tool (built-in, MCP, or loop-internal) and
        // collect the responses (+ any images) into one user turn.
        strip_run_status(&mut contents);
        let mut response_parts: Vec<Value> = Vec::new();
        let mut progressed = false;
        let mut round_failed = false;
        let mut loop_warning: Option<String> = None;
        for (name, args) in calls {
            // Stop promptly mid-burst too - don't run the remaining tool calls
            // of this turn after the user has already clicked Stop.
            if cancel_requested(source) {
                flush_phase(app, source, &mut phase);
                emit(app, source, "done", "Stopped");
                return Ok(stopped_message(true));
            }

            if name == "update_plan" {
                if ledger.apply_plan_update(&args) {
                    progressed = true;
                }
                if !ledger.plan.is_empty() {
                    emit(app, source, "plan", ledger.plan_summary_line());
                }
                ledger.record("updated plan", true, None);
                let mut resp = json!({ "ok": true, "plan": ledger.plan_lines() });
                if !ledger.done_when.is_empty() {
                    resp["done_when"] = json!(ledger.done_when);
                }
                response_parts.push(json!({ "functionResponse": { "name": name, "response": resp } }));
                continue;
            }

            ledger.actions += 1;
            let label = tool_label(&name, &args);
            emit(app, source, "tool", label.clone());
            let repeats = progress.note_call(&name, &args);
            let is_mcp = mcp.owns(&name);
            let mut outcome = if is_mcp {
                match mcp.call(&name, args.clone()).await {
                    Ok(text) => ToolOutcome {
                        response: json!({ "result": truncate(&text, MAX_TOOL_OUTPUT) }),
                        image_data_url: None,
                        log: truncate(&text, 300),
                    },
                    Err(e) => ToolOutcome::err(e),
                }
            } else {
                let mut outcome =
                    execute_tool(app, backend, source, &name, &args, screen, &mut trusted_apps).await;
                // Observe-act-verify: UI actions get a fresh screenshot attached
                // automatically so the model SEES the result instead of assuming it.
                attach_verification_shot(&name, &mut outcome, screen).await;
                outcome
            };
            if !outcome.response.is_object() {
                outcome.response = json!({ "result": outcome.response });
            }

            // Did the screen visibly change? (Progress for UI work.)
            if let Some(url) = outcome.image_data_url.clone() {
                if let Ok(Some(thumb)) = tokio::task::spawn_blocking(move || screen_thumbnail(&url)).await {
                    if progress.observe_screen(thumb) {
                        progressed = true;
                    }
                }
            }

            if tool_failed(&name, &outcome.response) {
                round_failed = true;
                ledger.errors += 1;
                consecutive_errors += 1;
                let failure = response_text(&outcome.response);
                // Identical call, identical failure, again: the model isn't
                // adapting, it's retrying. Say so where it can't be missed.
                let fingerprint = format!("{}:{}", name, args);
                if last_failed_call.as_deref() == Some(fingerprint.as_str()) {
                    repeated_failures += 1;
                    outcome.response["repeat_warning"] = json!(format!(
                        "You've now called {} with exactly these arguments {} times and it failed the same way. \
Repeating it won't work - change something real or take a different route.",
                        name,
                        repeated_failures + 1
                    ));
                } else {
                    repeated_failures = 0;
                }
                last_failed_call = Some(fingerprint);
                if let Some(hint) = recovery_hint(&name, &failure) {
                    outcome.response["hint"] = json!(hint);
                }
                let first_line = failure.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                ledger.record(&label, false, Some(clip(&first_line, 120)));
            } else {
                consecutive_errors = 0;
                repeated_failures = 0;
                last_failed_call = None;
                if is_effect_tool(&name) || is_mcp {
                    ledger.effects += 1;
                }
                if progress.observe_result(&name, &args, &outcome.response, is_mcp) {
                    progressed = true;
                }
                // Data worth keeping past compaction.
                let keep_fact = is_mcp
                    || matches!(
                        name.as_str(),
                        "web_search" | "web_fetch" | "delegate_to_browser" | "check_browser_task" | "read_app_content"
                    );
                if keep_fact {
                    ledger.record_fact(format!("[step {}] {}: {}", ledger.steps, clip(&label, 60), response_text(&outcome.response)));
                }
                ledger.record(&label, true, None);
            }

            if repeats >= LOOP_REPEAT_WARN {
                let warning = format!(
                    "You've done {} with the same arguments {} times without anything changing in between. \
It isn't working - stop repeating it and take a different route.",
                    name, repeats
                );
                outcome.response["loop_warning"] = json!(warning.clone());
                loop_warning = Some(warning);
            }

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
        last_round_failed = round_failed;
        progress.end_round(progressed, ledger.steps);

        // run_status rides on the last result of the round.
        let can_extend = budget < HARD_MAX_STEPS && started.elapsed() < MAX_RUN_TIME;
        let warning = if progress.stall_steps >= STALL_WARN_STEPS {
            Some(format!(
                "No progress in the last {} steps (no new data, no visible change). Re-think: re-check the \
state, re-plan, and try a genuinely different route.",
                progress.stall_steps
            ))
        } else {
            loop_warning
        };
        let status = run_status(&ledger, budget, can_extend, project.as_deref(), warning);
        if let Some(last) = response_parts.iter_mut().rev().find(|p| p.get("functionResponse").is_some()) {
            if let Some(obj) = last.pointer_mut("/functionResponse/response").and_then(|r| r.as_object_mut()) {
                obj.insert("run_status".into(), status);
            }
        }
        contents.push(json!({ "role": "user", "parts": response_parts }));

        // After several failures in a row, force a step back.
        if consecutive_errors >= 4 {
            consecutive_errors = 0;
            emit(app, source, "thinking", "Several steps failed in a row - rethinking the approach…");
            push_user_text(
                &mut contents,
                "[SYSTEM NOTE] Several tool calls in a row failed. Stop retrying this approach: re-check the \
current state, reconsider the plan, and try a genuinely different route. If the task truly can't be done, \
finish now with a plain-text explanation of what's blocking it.",
            );
        }

        // A hard loop (the identical call on an unchanged state, warned about
        // and done again anyway) won't resolve itself; neither will a long stall.
        if !progressed && progress.worst_repeat() >= LOOP_REPEAT_STOP {
            let report = stop_with_report(
                app,
                backend,
                system,
                tools,
                &mut contents,
                source,
                &ledger,
                &mut phase,
                "it kept repeating the same action without anything changing",
            )
            .await;
            return Ok(report);
        }
        if progress.stall_steps >= STALL_STOP_STEPS {
            let why = format!("the last {} steps made no visible progress (it looks stuck)", progress.stall_steps);
            let report =
                stop_with_report(app, backend, system, tools, &mut contents, source, &ledger, &mut phase, &why).await;
            return Ok(report);
        }

        // Graceful finish: when the budget is nearly spent and won't be
        // extended, tell the model to verify and report instead of being cut off.
        let will_extend =
            ledger.steps.saturating_sub(progress.last_progress_step) < BUDGET_EXTENSION && can_extend;
        if budget.saturating_sub(ledger.steps) == 2 && !will_extend {
            push_user_text(
                &mut contents,
                "[SYSTEM NOTE] You're almost out of steps. Do at most one more essential action, then verify \
the outcome and give your final plain-text answer - state clearly what you completed and what is left.",
            );
        }

        slim_old_tool_results(&mut contents, KEEP_FULL_TOOL_ROUNDS);
        if compact_history(&mut contents, &ledger.digest(30), false) {
            emit(app, source, "thinking", "Compacting older steps to keep the context small…");
        }
    }
}

fn tool_label(name: &str, args: &Value) -> String {
    match name {
        "run_command" => format!("$ {}", args.get("command").and_then(|c| c.as_str()).unwrap_or("")),
        "read_file" => format!("Reading {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "edit_file" => format!("Editing {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "search_in_files" => format!("Searching for \"{}\"", truncate(args.get("query").and_then(|c| c.as_str()).unwrap_or(""), 60)),
        "get_clipboard" => "Reading the clipboard".to_string(),
        "set_clipboard" => "Copying to the clipboard".to_string(),
        "download_file" => format!("Downloading to {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "notify" => "Notifying you".to_string(),
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
        "read_app_content" => "Reading app content".to_string(),
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
        "delegate_to_browser" => format!(
            "🌐 Browser: {}",
            truncate(args.get("instruction").and_then(|c| c.as_str()).unwrap_or(""), 70)
        ),
        "check_browser_task" => "Collecting browser result".to_string(),
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
    trusted: &mut HashSet<String>,
) -> ToolOutcome {
    match name {
        "run_command" => run_command_tool(args).await,
        "read_file" => read_file_tool(args),
        "write_file" => write_file_tool(args),
        "edit_file" => edit_file_tool(args),
        "search_in_files" => search_in_files_tool(args).await,
        "get_clipboard" => clipboard_tool(None).await,
        "set_clipboard" => clipboard_tool(Some(args.get("text").and_then(|t| t.as_str()).unwrap_or(""))).await,
        "download_file" => download_file_tool(args).await,
        "notify" => notify_tool(app, args),
        "delete_file" => delete_file_tool(args),
        "list_directory" => list_directory_tool(args),
        "find_files" => find_files_tool(args).await,
        "web_search" => web_search_tool(backend, args).await,
        "web_fetch" => web_fetch_tool(backend, args).await,
        "capture_screen" => capture_screen_tool(screen).await,
        "get_screen_context" => get_screen_context_tool().await,
        "list_windows" => list_windows_tool().await,
        "focus_window" => focus_window_tool(args).await,
        "read_app_content" => read_app_content_tool().await,
        "click" => click_tool(app, source, trusted, args, screen).await,
        "double_click" => double_click_tool(app, source, trusted, args, screen).await,
        "scroll" => scroll_tool(app, source, trusted, args, screen).await,
        "type_text" => type_text_tool(app, source, trusted, args).await,
        "press_key" => press_key_tool(app, source, trusted, args).await,
        "open_url" => open_target_tool(args, "url"),
        "open_app" => open_target_tool(args, "target"),
        "wait" => wait_tool(args).await,
        "request_confirmation" => request_confirmation_tool(app, source, args).await,
        "delegate_to_browser" => delegate_to_browser_tool(source, args).await,
        "check_browser_task" => check_browser_task_tool(args).await,
        other => ToolOutcome::err(format!("Unknown tool: {}", other)),
    }
}

// ── Observe-act-verify: automatic post-action screenshots ────────
// The root cause of the agent confidently claiming "I created the note!" when
// nothing was saved: UI actions used to return only a text receipt ("Typed 22
// characters."), so unless the model REMEMBERED to call capture_screen it was
// flying blind and assumed success. Production browser/computer-use agents
// (Manus, Claude computer use, OpenAI CUA) all close this loop by observing
// after every act - so now every action that changes what's on screen gets a
// fresh screenshot attached to its result automatically, after a short settle
// delay. The screenshot also refreshes the coordinate mapping, keeping the
// "coordinates are relative to the most recent screenshot" contract intact.

/// Settle delay before the verification shot: instant for input synthesis,
/// longer for actions that open/switch windows or load pages.
fn post_action_delay_ms(tool_name: &str) -> Option<u64> {
    match tool_name {
        "click" | "double_click" | "scroll" | "type_text" | "press_key" => Some(900),
        "open_url" | "open_app" | "focus_window" => Some(2000),
        // After an explicit wait the whole point is to look again.
        "wait" => Some(0),
        _ => None,
    }
}

/// Capture the screen for verification and refresh the coordinate mapping.
async fn verification_shot(screen: &mut Option<ScreenMapping>, delay_ms: u64) -> Option<String> {
    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    let shot = tokio::task::spawn_blocking(capture::capture_agent_full).await.ok()?.ok()?;
    *screen = Some(ScreenMapping::from_shot(&shot));
    Some(shot.data_url)
}

/// Attach an automatic verification screenshot to a UI action's outcome (only
/// when the action itself succeeded and didn't already produce an image).
async fn attach_verification_shot(
    name: &str,
    outcome: &mut ToolOutcome,
    screen: &mut Option<ScreenMapping>,
) {
    if outcome.image_data_url.is_some() || outcome.response.get("error").is_some() {
        return;
    }
    let Some(delay) = post_action_delay_ms(name) else { return };
    if let Some(data_url) = verification_shot(screen, delay).await {
        outcome.image_data_url = Some(data_url);
        if let Some(obj) = outcome.response.as_object_mut() {
            obj.insert(
                "verification".to_string(),
                json!(
                    "A fresh screenshot of the screen AFTER this action is attached. Inspect it to \
confirm the action actually had the intended effect before moving on - if the expected change \
isn't visible, the action did NOT work. Base any next click/scroll coordinates on this screenshot."
                ),
            );
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
    // No cwd → the agent's own scratch dir, never the host process's actual
    // cwd (which under `tauri dev` is the app's own source tree - that's how
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

/// UTF-8 in/out and no progress bars: PowerShell otherwise emits output in
/// the console's OEM code page (garbling non-English text) and progress
/// records that are pure noise in a captured log.
fn powershell_prelude(command: &str) -> String {
    format!(
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; $OutputEncoding=[Text.Encoding]::UTF8; $ProgressPreference='SilentlyContinue'; {}",
        command
    )
}

fn hidden_powershell(command: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &powershell_prelude(command)]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW - no console flash per command
    }
    cmd
}

fn run_powershell(command: &str, background: bool, cwd: Option<&str>, timeout: Duration) -> Result<String, String> {
    use std::process::Stdio;

    if let Some(dir) = cwd {
        if !std::path::Path::new(dir).is_dir() {
            return Err(format!("cwd \"{}\" doesn't exist or isn't a folder.", dir));
        }
    }

    if background {
        // Detached, but its output goes to a log file the agent can read_file
        // later (e.g. to see whether a dev server actually started).
        let log = agent_scratch_dir().join(format!("bg-{}.log", uuid::Uuid::new_v4().simple()));
        let _ = std::fs::create_dir_all(agent_scratch_dir());
        let file = std::fs::File::create(&log).map_err(|e| format!("couldn't create log: {}", e))?;
        let err_file = file.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = hidden_powershell(command);
        cmd.stdout(Stdio::from(file)).stderr(Stdio::from(err_file)).stdin(Stdio::null());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let child = cmd.spawn().map_err(|e| format!("failed to start: {}", e))?;
        return Ok(format!(
            "Started in the background (pid {}). Output is being written to {} - read_file it to check progress.",
            child.id(),
            log.display()
        ));
    }

    let mut spawn_cmd = hidden_powershell(command);
    spawn_cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    if let Some(dir) = cwd {
        spawn_cmd.current_dir(dir);
    }
    let child = spawn_cmd.spawn().map_err(|e| format!("failed to start: {}", e))?;

    // Enforce a wall-clock timeout so one hung command can't stall the agent.
    let (status, stdout, stderr, timed_out) = wait_with_timeout(child, timeout)?;
    let mut combined = String::new();
    if !stdout.trim().is_empty() {
        combined.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str("[stderr] ");
        combined.push_str(stderr.trim_end());
    }
    if timed_out {
        return Err(format!(
            "command timed out after {}s (pass a longer timeout_seconds for a slow-but-finite command, or background=true for a long-running/indefinite one). Output so far:\n{}",
            timeout.as_secs(),
            if combined.is_empty() { "(none)".to_string() } else { truncate(&combined, 4000) }
        ));
    }
    if combined.is_empty() {
        combined.push_str("(command completed with no output)");
    }
    // The model can't tell a failure from a success without this.
    match status.and_then(|s| s.code()) {
        Some(0) | None => {}
        Some(code) => combined.push_str(&format!("\n[exit code {}]", code)),
    }
    Ok(combined)
}

/// Waits for the child while DRAINING its pipes on background threads.
/// Reading only after exit (the old way) deadlocks as soon as a command
/// prints more than the pipe buffer (npm install, builds, big listings):
/// the child blocks writing, never exits, and "times out".
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<(Option<std::process::ExitStatus>, String, String, bool), String> {
    use std::io::Read;
    use std::time::Instant;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out_t = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err_t = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));

    let start = Instant::now();
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(60)),
            Err(e) => return Err(format!("wait failed: {}", e)),
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    Ok((status, stdout, stderr, timed_out))
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
    // what actually changed - None (file didn't exist yet) means every line
    // of the new content counts as added.
    let old_content = std::fs::read_to_string(&path).ok();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, content) {
        Ok(()) => {
            let scratch_note = if path.starts_with(agent_scratch_dir()) {
                " (private scratch file - delete_file it once you're done with it, unless the user asked to keep it)"
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
                response: json!({ "result": "screenshot captured - use normalized 0-1000 x/y relative to this image for the next click/scroll" }),
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
        // background window while another has focus) - acting via type_text/
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
        // (tray icons, IME, tooltips) - they have a title but are noise for this list.
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
        .ok_or_else(|| format!("no open window matches \"{}\" - check list_windows for the exact title", needle))?;

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
                "found \"{}\" but Windows blocked bringing it to the foreground - try clicking its taskbar icon instead",
                label
            ));
        }
    }
    remember_target_window(hwnd as isize, &label);
    Ok(label)
}

#[cfg(not(windows))]
fn focus_window_by_title(_needle: &str) -> Result<String, String> {
    Err("window focus is only supported on Windows".to_string())
}

async fn list_windows_tool() -> ToolOutcome {
    let mut windows = tokio::task::spawn_blocking(enumerate_top_windows).await.unwrap_or_default();
    // Surface the actually-focused window first - it's the one type_text/
    // press_key would act on right now, so it's the most decision-relevant entry.
    windows.sort_by_key(|w| !w.foreground);
    let count = windows.len();
    let entries: Vec<Value> = windows
        .iter()
        .take(40)
        .map(|w| json!({
            "title": w.title,
            "process": w.process_name,
            "category": classify_process(&w.process_name),
            "focused": w.foreground,
        }))
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
/// if there's no mapping yet - the model needs to capture_screen first.
fn resolve_point(args: &Value, screen: &Option<ScreenMapping>) -> Result<(i32, i32), String> {
    let nx = args.get("x").and_then(|v| v.as_f64());
    let ny = args.get("y").and_then(|v| v.as_f64());
    let (nx, ny) = match (nx, ny) {
        (Some(x), Some(y)) => (x, y),
        _ => return Err("requires numeric x and y".to_string()),
    };
    let mapping = screen
        .as_ref()
        .ok_or("no screenshot has been captured yet - call capture_screen first so coordinates have something to map to")?;
    Ok(mapping.to_real(nx, ny))
}

/// The agent's own always-on-top overlay floats above whatever app it's
/// driving - a click aimed "at the page" can actually land on Mouse AI's own
/// panel, and typing right after the user pressed Enter often goes into Mouse
/// AI's own input box. Both look like success to the model while doing nothing
/// to the target app (the classic "I created the note!" when nothing saved).
/// These return a corrective error instead of letting that happen.
async fn reject_own_window_at(x: i32, y: i32) -> Option<ToolOutcome> {
    let own = tokio::task::spawn_blocking(move || mouse::is_own_process_at(x, y))
        .await
        .unwrap_or(false);
    if own {
        Some(ToolOutcome::err(
            "those coordinates land on Mouse AI's OWN floating panel, which is covering that part \
of the target app - the action was NOT performed. Aim at a point on the target app itself; if the \
element you need is hidden behind the panel, pick the same element elsewhere on screen, scroll it \
into a visible spot, or focus_window the target app and work from a fresh screenshot.",
        ))
    } else {
        None
    }
}

/// Keyboard-focus gate for type_text/press_key. When focus is on Mouse AI's
/// own window (the user touched the overlay mid-run), first try to hand focus
/// back to the last window the agent clicked or focused - only if that isn't
/// possible does the step fail with the corrective error. Returns an optional
/// note describing the auto-recovery for the tool's success message.
async fn ensure_target_focused() -> Result<Option<String>, Box<ToolOutcome>> {
    let own = tokio::task::spawn_blocking(mouse::is_own_process_foreground)
        .await
        .unwrap_or(false);
    if !own {
        return Ok(None);
    }
    if let Some((hwnd, label)) = last_target_window() {
        let refocused = tokio::task::spawn_blocking(move || mouse::focus_hwnd(hwnd)).await;
        if matches!(refocused, Ok(Ok(()))) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let still_own = tokio::task::spawn_blocking(mouse::is_own_process_foreground)
                .await
                .unwrap_or(true);
            if !still_own {
                return Ok(Some(format!(
                    "focus had drifted to Mouse AI's own window - auto-refocused \"{}\" first",
                    label
                )));
            }
        }
    }
    Err(Box::new(ToolOutcome::err(
        "keyboard focus is currently on Mouse AI's OWN window, so the keystrokes would go into \
the assistant's UI instead of the target app - nothing was typed. First click on the target app or \
focus_window it, verify focus from the attached screenshot, then type.",
    )))
}

/// Resolves whatever process/window sits at a real screen point, off the
/// blocking thread pool (the underlying MSAA/Win32 calls aren't async).
async fn context_at_point_async(x: i32, y: i32) -> mouse::CursorContext {
    tokio::task::spawn_blocking(move || mouse::context_at_point(x, y))
        .await
        .unwrap_or(mouse::CursorContext {
            x,
            y,
            window_title: String::new(),
            process_name: String::new(),
            class_name: String::new(),
        })
}

async fn click_tool(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    args: &Value,
    screen: &Option<ScreenMapping>,
) -> ToolOutcome {
    let (x, y) = match resolve_point(args, screen) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::err(e),
    };
    if let Some(rejected) = reject_own_window_at(x, y).await {
        return rejected;
    }
    let target = context_at_point_async(x, y).await;
    if let Err(e) = guard_target_app(app, source, trusted, &target.process_name, &target.window_title).await {
        return ToolOutcome::err(e);
    }
    let hwnd = tokio::task::spawn_blocking(move || mouse::target_hwnd_at_point(x, y)).await.unwrap_or(0);
    remember_target_window(hwnd, &target.window_title);
    let right = args.get("button").and_then(|b| b.as_str()).map(|b| b.eq_ignore_ascii_case("right")).unwrap_or(false);
    input::click(x, y, right);
    ToolOutcome::text(format!("Clicked {} at ({}, {}).", if right { "right" } else { "left" }, x, y))
}

async fn double_click_tool(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    args: &Value,
    screen: &Option<ScreenMapping>,
) -> ToolOutcome {
    let (x, y) = match resolve_point(args, screen) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::err(e),
    };
    if let Some(rejected) = reject_own_window_at(x, y).await {
        return rejected;
    }
    let target = context_at_point_async(x, y).await;
    if let Err(e) = guard_target_app(app, source, trusted, &target.process_name, &target.window_title).await {
        return ToolOutcome::err(e);
    }
    let hwnd = tokio::task::spawn_blocking(move || mouse::target_hwnd_at_point(x, y)).await.unwrap_or(0);
    remember_target_window(hwnd, &target.window_title);
    input::double_click(x, y);
    ToolOutcome::text(format!("Double-clicked ({}, {}).", x, y))
}

async fn scroll_tool(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    args: &Value,
    screen: &Option<ScreenMapping>,
) -> ToolOutcome {
    let amount = match args.get("amount").and_then(|v| v.as_i64()) {
        Some(a) => a,
        None => return ToolOutcome::err("scroll requires an integer amount"),
    };
    let (x, y) = match resolve_point(args, screen) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::err(e),
    };
    if let Some(rejected) = reject_own_window_at(x, y).await {
        return rejected;
    }
    let target = context_at_point_async(x, y).await;
    if let Err(e) = guard_target_app(app, source, trusted, &target.process_name, &target.window_title).await {
        return ToolOutcome::err(e);
    }
    input::scroll(x, y, amount as i32);
    ToolOutcome::text(format!("Scrolled {} at ({}, {}).", amount, x, y))
}

/// Shows the user a real Allow/Deny button and waits for their click (with a
/// 5-minute timeout, treated as a deny so the task doesn't hang forever).
/// Shared by the model-invoked `request_confirmation` tool and the automatic
/// per-app gate in `guard_target_app`.
pub(crate) async fn confirm(app: &AppHandle, source: &str, message: &str) -> bool {
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    confirmations().lock().unwrap().insert(request_id.clone(), tx);

    let _ = app.emit(
        "agent-progress",
        AgentProgress {
            source: source.to_string(),
            kind: "confirm".to_string(),
            message: message.to_string(),
            request_id: Some(request_id.clone()),
        },
    );

    const CONFIRM_TIMEOUT: Duration = Duration::from_secs(300);
    match tokio::time::timeout(CONFIRM_TIMEOUT, rx).await {
        Ok(Ok(approved)) => approved,
        Ok(Err(_)) => false,
        Err(_) => {
            confirmations().lock().unwrap().remove(&request_id);
            false
        }
    }
}

async fn request_confirmation_tool(app: &AppHandle, source: &str, args: &Value) -> ToolOutcome {
    let message = args
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("Proceed with this action?")
        .to_string();
    if confirm(app, source, &message).await {
        ToolOutcome::text("The user clicked Allow. Proceed.")
    } else {
        ToolOutcome::text(
            "The user clicked Deny (or didn't respond within 5 minutes). Do not proceed with this \
action - ask what they'd like instead, or stop.",
        )
    }
}

/// Hand a web task to the connected browser extension over the local bridge.
/// Inline by default (waits for the result); background=true returns a task_id
/// so the desktop agent can keep working and collect it later via
/// check_browser_task - that's how desktop + browser run in parallel.
async fn delegate_to_browser_tool(source: &str, args: &Value) -> ToolOutcome {
    let instruction = args.get("instruction").and_then(|v| v.as_str()).unwrap_or("").trim();
    if instruction.is_empty() {
        return ToolOutcome::err("delegate_to_browser needs an 'instruction' describing the web task.");
    }
    // A task the browser handed us must not bounce back to the browser -
    // that's how two cooperating agents end up in an endless loop.
    if source.starts_with("browser:") {
        return ToolOutcome::err("This task came from the browser agent - do the desktop part and return the result instead of delegating back to the browser.");
    }
    let background = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut options = json!({});
    if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
        if !url.trim().is_empty() {
            options["url"] = json!(url.trim());
        }
    }

    let b = crate::bridge::bridge();
    if background {
        match b.start_task(source, instruction, options) {
            Ok(id) => ToolOutcome::text(format!(
                "Started the browser task in the background (task_id: {}). Continue with other \
local work, then call check_browser_task with this id to get the result.",
                id
            )),
            Err(e) => ToolOutcome::err(e),
        }
    } else {
        match b.run_task(source, instruction, options).await {
            Ok(result) => ToolOutcome::text(format!("Browser task result:\n{}", result)),
            Err(e) => ToolOutcome::err(e),
        }
    }
}

/// Collect the result of a background browser task started earlier.
async fn check_browser_task_tool(args: &Value) -> ToolOutcome {
    let id = args.get("task_id").and_then(|v| v.as_str()).unwrap_or("").trim();
    if id.is_empty() {
        return ToolOutcome::err("check_browser_task needs the 'task_id' from delegate_to_browser.");
    }
    match crate::bridge::bridge().await_task(id, 600).await {
        Ok(result) => ToolOutcome::text(format!("Browser task result:\n{}", result)),
        Err(e) => ToolOutcome::err(e),
    }
}

/// Automatic safety net, independent of the model's own judgment: the first
/// time in a run that an input-affecting tool (click/double_click/scroll/
/// type_text/press_key) is about to act on a given process, this pauses and
/// shows the user a real Allow/Deny prompt - exactly like `request_confirmation`,
/// but the model doesn't have to remember to call it, and can't skip it either.
/// Approval is remembered for the rest of THIS run only (not persisted), so a
/// single task touching one app in a loop only interrupts once.
async fn guard_target_app(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    process_name: &str,
    window_title: &str,
) -> Result<(), String> {
    let key = process_name.trim().to_lowercase();
    if key.is_empty() || trusted.contains(&key) {
        return Ok(());
    }
    let label = if window_title.trim().is_empty() {
        process_name.to_string()
    } else {
        format!("{} ({})", window_title, process_name)
    };
    let message = format!("First action on \"{}\" this task - let Mouse AI control it?", label);
    if confirm(app, source, &message).await {
        trusted.insert(key);
        Ok(())
    } else {
        Err(format!(
            "Denied: the user has not approved Mouse AI controlling \"{}\" for this task.",
            process_name
        ))
    }
}

/// Launch a URL / app / file via the shell (ShellExecute semantics), which
/// picks the right default handler - a browser for URLs, Explorer for folders,
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

/// type_text/press_key act on whatever has keyboard FOCUS, not whatever's
/// under the cursor - the foreground window is the correct thing to gate on.
async fn foreground_context_async() -> mouse::CursorContext {
    tokio::task::spawn_blocking(mouse::get_foreground_context)
        .await
        .unwrap_or(mouse::CursorContext {
            x: 0,
            y: 0,
            window_title: String::new(),
            process_name: String::new(),
            class_name: String::new(),
        })
}

async fn type_text_tool(app: &AppHandle, source: &str, trusted: &mut HashSet<String>, args: &Value) -> ToolOutcome {
    let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("");
    if text.is_empty() {
        return ToolOutcome::err("no text provided");
    }
    let focus_note = match ensure_target_focused().await {
        Ok(note) => note,
        Err(rejected) => return *rejected,
    };
    let fg = foreground_context_async().await;
    if let Err(e) = guard_target_app(app, source, trusted, &fg.process_name, &fg.window_title).await {
        return ToolOutcome::err(e);
    }
    input::type_text(text);
    match focus_note {
        Some(note) => ToolOutcome::text(format!("Typed {} characters ({}).", text.chars().count(), note)),
        None => ToolOutcome::text(format!("Typed {} characters.", text.chars().count())),
    }
}

async fn press_key_tool(app: &AppHandle, source: &str, trusted: &mut HashSet<String>, args: &Value) -> ToolOutcome {
    let keys = args.get("keys").and_then(|k| k.as_str()).unwrap_or("");
    if keys.is_empty() {
        return ToolOutcome::err("no keys provided");
    }
    let focus_note = match ensure_target_focused().await {
        Ok(note) => note,
        Err(rejected) => return *rejected,
    };
    let fg = foreground_context_async().await;
    if let Err(e) = guard_target_app(app, source, trusted, &fg.process_name, &fg.window_title).await {
        return ToolOutcome::err(e);
    }
    match input::press_combo(keys) {
        Ok(()) => match focus_note {
            Some(note) => ToolOutcome::text(format!("Pressed {} ({}).", keys, note)),
            None => ToolOutcome::text(format!("Pressed {}.", keys)),
        },
        Err(e) => ToolOutcome::err(e),
    }
}

async fn read_app_content_tool() -> ToolOutcome {
    let outcome = tokio::task::spawn_blocking(|| {
        let fg = mouse::get_foreground_context();
        let nodes = mouse::foreground_window_rect()
            .map(|(x, y, w, h)| mouse::sample_window_content(x, y, w, h))
            .unwrap_or_default();
        (fg, nodes)
    })
    .await;

    let (fg, nodes) = match outcome {
        Ok(v) => v,
        Err(e) => return ToolOutcome::err(format!("content read task failed: {}", e)),
    };

    if nodes.is_empty() {
        return ToolOutcome::text(format!(
            "No readable accessible content found in \"{}\" ({}) - it may not expose accessible \
content, could be empty, or you may need to click into it first.",
            fg.window_title, fg.process_name
        ));
    }

    let mut text = format!("[APP CONTENT - {} ({})]\n", fg.window_title, fg.process_name);
    for n in &nodes {
        let line = [n.role.as_str(), n.name.as_str(), n.value.as_str()]
            .iter()
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(": ");
        if !line.trim().is_empty() {
            text.push_str("- ");
            text.push_str(&line);
            text.push('\n');
        }
    }
    let count = nodes.len();
    ToolOutcome {
        response: json!({ "result": truncate(&text, MAX_TOOL_OUTPUT) }),
        image_data_url: None,
        log: format!("read {} content node(s) from {}", count, fg.process_name),
    }
}

/// Coarse app category from its process name - good enough for "should I
/// reuse this window or open a new one", not a real app registry.
fn classify_process(process_name: &str) -> &'static str {
    let p = process_name.to_lowercase();
    let browsers = [
        "chrome.exe", "msedge.exe", "firefox.exe", "comet.exe", "brave.exe", "opera.exe",
        "opera_gx.exe", "vivaldi.exe", "iexplore.exe",
    ];
    let office = [
        "excel.exe", "winword.exe", "powerpnt.exe", "outlook.exe", "onenote.exe", "access.exe",
        "msaccess.exe",
    ];
    let editors = [
        "code.exe", "cursor.exe", "notepad.exe", "notepad++.exe", "sublime_text.exe", "vim.exe",
        "gvim.exe", "devenv.exe", "idea64.exe", "pycharm64.exe",
    ];
    let terminals = ["powershell.exe", "pwsh.exe", "cmd.exe", "windowsterminal.exe", "conhost.exe"];
    if browsers.contains(&p.as_str()) {
        "browser"
    } else if office.contains(&p.as_str()) {
        "office"
    } else if editors.contains(&p.as_str()) {
        "editor"
    } else if terminals.contains(&p.as_str()) {
        "terminal"
    } else {
        "other"
    }
}


// ── Productivity tools: precise edits, content search, clipboard, downloads ──

fn edit_file_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let old_text = args.get("old_text").and_then(|p| p.as_str()).unwrap_or("");
    let new_text = args.get("new_text").and_then(|p| p.as_str()).unwrap_or("");
    let replace_all = args.get("replace_all").and_then(|p| p.as_bool()).unwrap_or(false);
    if raw.is_empty() || old_text.is_empty() {
        return ToolOutcome::err("edit_file needs 'path' and a non-empty 'old_text'");
    }
    let path = resolve_agent_path(raw);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return ToolOutcome::err(format!("could not read {}: {}", path.display(), e)),
    };
    // Files saved on Windows often use CRLF while the model writes LF.
    let (old_norm, new_norm) = if content.contains("\r\n") && !old_text.contains("\r\n") {
        (old_text.replace('\n', "\r\n"), new_text.replace('\n', "\r\n"))
    } else {
        (old_text.to_string(), new_text.to_string())
    };
    let count = content.matches(&old_norm).count();
    if count == 0 {
        return ToolOutcome::err(format!(
            "old_text was not found in {} - read_file it again and copy the snippet exactly (whitespace matters).",
            path.display()
        ));
    }
    if count > 1 && !replace_all {
        return ToolOutcome::err(format!(
            "old_text appears {} times in {} - include more surrounding lines to make it unique, or set replace_all.",
            count,
            path.display()
        ));
    }
    let updated = if replace_all { content.replace(&old_norm, &new_norm) } else { content.replacen(&old_norm, &new_norm, 1) };
    match std::fs::write(&path, &updated) {
        Ok(()) => {
            let n = if replace_all { count } else { 1 };
            let msg = format!("Edited {} ({} replacement{}).", path.display(), n, if n == 1 { "" } else { "s" });
            let mut log = msg.clone();
            if let Some((added, removed)) = line_diff_stats(&content, &updated) {
                log.push_str(&format!("\u{1}DIFF {} {}", added, removed));
            }
            ToolOutcome { response: json!({ "result": msg }), image_data_url: None, log }
        }
        Err(e) => ToolOutcome::err(format!("could not write {}: {}", path.display(), e)),
    }
}

async fn search_in_files_tool(args: &Value) -> ToolOutcome {
    let folder = args.get("folder").and_then(|p| p.as_str()).unwrap_or("").trim().to_string();
    let query = args.get("query").and_then(|p| p.as_str()).unwrap_or("").trim().to_lowercase();
    let ext = args
        .get("extension")
        .and_then(|p| p.as_str())
        .map(|e| e.trim().trim_start_matches("*.").trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty());
    if folder.is_empty() || query.is_empty() {
        return ToolOutcome::err("search_in_files needs 'folder' and 'query'");
    }
    let root = resolve_agent_path(&folder);
    let result = tokio::task::spawn_blocking(move || {
        const SKIP: [&str; 8] = ["node_modules", ".git", "target", "dist", "build", ".next", "__pycache__", ".venv"];
        const MAX_HITS: usize = 80;
        const MAX_FILES: usize = 20_000;
        let mut hits = Vec::new();
        let mut visited = 0usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let path = entry.path();
                let Ok(ft) = entry.file_type() else { continue };
                let name = entry.file_name().to_string_lossy().to_string();
                if ft.is_dir() {
                    if !SKIP.contains(&name.as_str()) {
                        stack.push(path);
                    }
                    continue;
                }
                visited += 1;
                if visited > MAX_FILES || hits.len() >= MAX_HITS {
                    return (hits, true);
                }
                if let Some(want) = &ext {
                    if path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() != Some(want.as_str()) {
                        continue;
                    }
                }
                if entry.metadata().map(|m| m.len() > 1_500_000).unwrap_or(true) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue }; // binary / non-UTF-8
                for (i, line) in text.lines().enumerate() {
                    if line.to_lowercase().contains(&query) {
                        let shown: String = line.trim().chars().take(200).collect();
                        hits.push(format!("{}:{}: {}", path.display(), i + 1, shown));
                        if hits.len() >= MAX_HITS {
                            break;
                        }
                    }
                }
            }
        }
        (hits, false)
    })
    .await;
    match result {
        Ok((hits, capped)) if hits.is_empty() => ToolOutcome::text(if capped {
            "No matches in the files scanned - narrow the folder or add an extension filter."
        } else {
            "No matches."
        }),
        Ok((hits, capped)) => {
            let mut out = hits.join("\n");
            if capped {
                out.push_str("\n…(stopped early - narrow the search for more)");
            }
            ToolOutcome {
                log: format!("{} match{}", hits.len(), if hits.len() == 1 { "" } else { "es" }),
                response: json!({ "result": out }),
                image_data_url: None,
            }
        }
        Err(e) => ToolOutcome::err(format!("search failed: {}", e)),
    }
}

/// None = read the clipboard; Some(text) = write it. Goes through PowerShell's
/// Get-/Set-Clipboard (no extra native dependency), with the window hidden.
async fn clipboard_tool(write: Option<&str>) -> ToolOutcome {
    use tokio::io::AsyncWriteExt;
    let script = match write {
        None => "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-Clipboard -Raw",
        Some(_) => "[Console]::InputEncoding=[Text.Encoding]::UTF8; Set-Clipboard -Value ([Console]::In.ReadToEnd())",
    };
    let mut cmd = tokio::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ToolOutcome::err(format!("clipboard unavailable: {}", e)),
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(text) = write {
            let _ = stdin.write_all(text.as_bytes()).await;
        }
        drop(stdin);
    }
    match tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await {
        Ok(Ok(out)) if out.status.success() => match write {
            None => {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                if text.trim().is_empty() {
                    ToolOutcome::text("The clipboard is empty (or holds an image/files, not text).")
                } else {
                    ToolOutcome::text(truncate(&text, MAX_FILE_READ))
                }
            }
            Some(text) => ToolOutcome::text(format!("Copied {} characters to the clipboard.", text.chars().count())),
        },
        Ok(Ok(out)) => ToolOutcome::err(format!("clipboard error: {}", String::from_utf8_lossy(&out.stderr).trim())),
        Ok(Err(e)) => ToolOutcome::err(format!("clipboard error: {}", e)),
        Err(_) => ToolOutcome::err("clipboard timed out"),
    }
}

async fn download_file_tool(args: &Value) -> ToolOutcome {
    const MAX_BYTES: usize = 300 * 1024 * 1024;
    let url = args.get("url").and_then(|p| p.as_str()).unwrap_or("").trim();
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("").trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) || raw.is_empty() {
        return ToolOutcome::err("download_file needs an http(s) 'url' and a destination 'path'");
    }
    let path = resolve_agent_path(raw);
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(300)).build() {
        Ok(c) => c,
        Err(e) => return ToolOutcome::err(e.to_string()),
    };
    let resp = match client.get(url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => return ToolOutcome::err(format!("download failed: HTTP {}", r.status())),
        Err(e) => return ToolOutcome::err(format!("download failed: {}", e)),
    };
    if resp.content_length().map(|n| n as usize > MAX_BYTES).unwrap_or(false) {
        return ToolOutcome::err("file is larger than 300 MB - use run_command (e.g. curl) if you really need it");
    }
    let bytes = match resp.bytes().await {
        Ok(b) if b.len() <= MAX_BYTES => b,
        Ok(_) => return ToolOutcome::err("file is larger than 300 MB"),
        Err(e) => return ToolOutcome::err(format!("download failed: {}", e)),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, &bytes) {
        Ok(()) => ToolOutcome::text(format!("Saved {} ({} KB).", path.display(), bytes.len() / 1024)),
        Err(e) => ToolOutcome::err(format!("could not write {}: {}", path.display(), e)),
    }
}

fn notify_tool(app: &AppHandle, args: &Value) -> ToolOutcome {
    use tauri_plugin_notification::NotificationExt;
    let title = args.get("title").and_then(|p| p.as_str()).unwrap_or("Mouse AI");
    let message = args.get("message").and_then(|p| p.as_str()).unwrap_or("");
    match app.notification().builder().title(title).body(message).show() {
        Ok(()) => ToolOutcome::text("Notification shown."),
        Err(e) => ToolOutcome::err(format!("could not show a notification: {}", e)),
    }
}

#[cfg(test)]
mod run_command_tests {
    use super::*;

    /// More output than a pipe buffer must not deadlock (it used to hang
    /// until the timeout).
    #[test]
    fn large_output_does_not_deadlock() {
        let out = run_powershell("1..20000 | ForEach-Object { \"line $_ of output\" }", false, None, Duration::from_secs(60))
            .expect("command should finish");
        assert!(out.contains("line 20000 of output"), "tail missing: {}", &out[out.len().saturating_sub(200)..]);
    }

    #[test]
    fn reports_nonzero_exit_code() {
        let out = run_powershell("Write-Output 'oops'; exit 3", false, None, Duration::from_secs(30)).unwrap();
        assert!(out.contains("[exit code 3]"), "{}", out);
    }

    #[test]
    fn keeps_non_ascii_output() {
        let out = run_powershell("Write-Output 'Muraho - café ✓'", false, None, Duration::from_secs(30)).unwrap();
        assert!(out.contains("café ✓"), "{}", out);
    }
}

#[cfg(test)]
mod agent_loop_tests {
    use super::*;

    fn model_call(name: &str, n: usize) -> Value {
        json!({ "role": "model", "parts": [{ "functionCall": { "name": name, "args": { "i": n } }, "thoughtSignature": "sig" }] })
    }

    fn tool_response(name: &str, body: &str) -> Value {
        json!({ "role": "user", "parts": [{ "functionResponse": { "name": name, "response": { "result": body } } }] })
    }

    fn history(rounds: usize, body: &str) -> Vec<Value> {
        let mut contents = vec![json!({ "role": "user", "parts": [{ "text": "[TASK]\nfix the build" }] })];
        for i in 0..rounds {
            contents.push(model_call("read_file", i));
            contents.push(tool_response("read_file", body));
        }
        contents
    }

    /// Every functionResponse turn must directly follow the model turn that
    /// issued the same number of functionCalls, and roles must alternate.
    fn assert_valid_transcript(contents: &[Value]) {
        assert_eq!(contents[0]["role"], "user");
        for i in 1..contents.len() {
            assert_ne!(contents[i]["role"], contents[i - 1]["role"], "roles must alternate at {}", i);
            if is_tool_round(&contents[i]) {
                let prev = &contents[i - 1];
                assert_eq!(prev["role"], "model");
                let calls = prev["parts"].as_array().unwrap().iter().filter(|p| p.get("functionCall").is_some()).count();
                let resps =
                    contents[i]["parts"].as_array().unwrap().iter().filter(|p| p.get("functionResponse").is_some()).count();
                assert_eq!(calls, resps, "call/response count mismatch at {}", i);
            }
        }
    }

    #[test]
    fn compaction_keeps_pairs_valid_and_recent_rounds() {
        let mut contents = history(20, "ok");
        assert!(compact_history(&mut contents, "Plan:\n✓ 1. read", false));
        assert_valid_transcript(&contents);
        assert_eq!(tool_round_indices(&contents).len(), COMPACT_KEEP_ROUNDS);
        // The newest round survived verbatim, thought signature included.
        let last_model = &contents[contents.len() - 2];
        assert_eq!(last_model["parts"][0]["functionCall"]["args"]["i"], 19);
        assert_eq!(last_model["parts"][0]["thoughtSignature"], "sig");
        let summary = contents[0]["parts"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string();
        assert!(summary.starts_with(COMPACTION_MARKER) && summary.contains("✓ 1. read"));
        // The task itself is still the first part.
        assert_eq!(contents[0]["parts"][0]["text"], "[TASK]\nfix the build");

        // A second compaction replaces the summary instead of stacking another.
        for i in 20..32 {
            contents.push(model_call("read_file", i));
            contents.push(tool_response("read_file", "ok"));
        }
        assert!(compact_history(&mut contents, "second digest", false));
        assert_valid_transcript(&contents);
        let markers = contents[0]["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["text"].as_str().is_some_and(|t| t.starts_with(COMPACTION_MARKER)))
            .count();
        assert_eq!(markers, 1);
    }

    #[test]
    fn compaction_skips_small_histories_unless_forced() {
        let mut small = history(10, "ok");
        assert!(!compact_history(&mut small, "d", false));
        assert_eq!(small.len(), 21);
        assert!(compact_history(&mut small, "d", true));
        assert_valid_transcript(&small);
        assert_eq!(tool_round_indices(&small).len(), (COMPACT_KEEP_ROUNDS / 2).max(2));
    }

    #[test]
    fn compaction_triggers_on_size_and_keeps_text_turns_valid() {
        let big = "x".repeat(COMPACT_AT_CHARS / 9);
        let mut contents = history(10, &big);
        // A completion-check exchange (model text, then user text) between rounds.
        contents.insert(9, json!({ "role": "model", "parts": [{ "text": "All done!" }] }));
        contents.insert(10, json!({ "role": "user", "parts": [{ "text": "[SYSTEM] Completion check" }] }));
        assert_valid_transcript(&contents);
        assert!(compact_history(&mut contents, "d", false));
        assert_valid_transcript(&contents);
        assert_eq!(tool_round_indices(&contents).len(), COMPACT_KEEP_ROUNDS);
    }

    #[test]
    fn slimming_trims_old_rounds_only_and_is_idempotent() {
        let big = "y".repeat(5000);
        let mut contents = history(6, &big);
        slim_old_tool_results(&mut contents, 2);
        let rounds = tool_round_indices(&contents);
        let first = &contents[rounds[0]]["parts"][0]["functionResponse"]["response"];
        assert!(first.get("trimmed").is_some());
        assert!(first["result_preview"].as_str().unwrap().chars().count() <= SLIM_TOOL_RESULT_CHARS);
        let last = &contents[*rounds.last().unwrap()]["parts"][0]["functionResponse"]["response"];
        assert_eq!(last["result"].as_str().unwrap().len(), 5000);
        let snapshot = contents.clone();
        slim_old_tool_results(&mut contents, 2);
        assert_eq!(snapshot, contents);
    }

    #[test]
    fn run_status_is_stripped_and_text_merges_into_user_turn() {
        let mut contents = history(2, "ok");
        contents[2]["parts"][0]["functionResponse"]["response"]["run_status"] = json!({ "step": "1 of 40" });
        strip_run_status(&mut contents);
        assert!(contents[2]["parts"][0]["functionResponse"]["response"].get("run_status").is_none());

        push_user_text(&mut contents, "note");
        assert_eq!(contents.len(), 5);
        assert_eq!(contents[4]["parts"].as_array().unwrap().len(), 2);
        contents.push(json!({ "role": "model", "parts": [{ "text": "hi" }] }));
        push_user_text(&mut contents, "note 2");
        assert_eq!(contents.len(), 7);
        assert_valid_transcript(&contents);
    }

    #[test]
    fn loop_detection_counts_repeats_on_unchanged_state() {
        let mut p = ProgressTracker::default();
        let args = json!({ "path": "C:/a.txt" });
        assert_eq!(p.note_call("read_file", &args), 1);
        assert!(p.observe_result("read_file", &args, &json!({ "result": "same" }), false));
        assert_eq!(p.note_call("read_file", &args), 2);
        assert!(!p.observe_result("read_file", &args, &json!({ "result": "same" }), false));
        assert_eq!(p.note_call("read_file", &args), 3);
        // A real change (an edit) resets the count for the next identical read.
        let edit = json!({ "path": "C:/a.txt", "old_text": "a", "new_text": "b" });
        p.note_call("edit_file", &edit);
        assert!(p.observe_result("edit_file", &edit, &json!({ "result": "Edited" }), false));
        assert_eq!(p.note_call("read_file", &args), 1);
        assert_eq!(p.worst_repeat(), 3);
        // Waiting is legitimately repetitive.
        assert_eq!(p.note_call("wait", &json!({ "seconds": 2 })), 0);
        assert_eq!(p.note_call("wait", &json!({ "seconds": 2 })), 0);
    }

    #[test]
    fn stall_counter_and_screen_changes() {
        let mut p = ProgressTracker::default();
        p.end_round(false, 1);
        p.end_round(false, 2);
        assert_eq!(p.stall_steps, 2);
        p.end_round(true, 3);
        assert_eq!((p.stall_steps, p.last_progress_step), (0, 3));

        let a = vec![100u8; (THUMB_W * THUMB_H) as usize];
        let mut b = a.clone();
        b[0] = 200; // a clock tick
        assert!(!screens_differ(&a, &b));
        b[10] = 200;
        b[20] = 0;
        assert!(screens_differ(&a, &b));
        assert!(p.observe_screen(a.clone()));
        assert!(!p.observe_screen(a.clone()));
    }

    #[test]
    fn screen_thumbnail_decodes_a_data_url() {
        use base64::Engine;
        let img = image::RgbImage::from_pixel(64, 40, image::Rgb([10, 200, 30]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png));
        let thumb = screen_thumbnail(&url).expect("thumbnail");
        assert_eq!(thumb.len(), (THUMB_W * THUMB_H) as usize);
        assert!(screen_thumbnail("data:image/png;base64,!!!").is_none());
    }

    fn ledger_with(effects: usize, errors: usize, steps: usize) -> Ledger {
        Ledger { effects, errors, steps, ..Ledger::default() }
    }

    #[test]
    fn completion_check_flags_unbacked_success_claims() {
        let task = "[Project folder: C:\\proj — work inside this folder and use absolute paths under it for files and shell commands.]\n\nFix the failing test";
        let c = completion_concerns("Done! I fixed the test.", &ledger_with(0, 0, 5), task, false);
        assert_eq!(c.len(), 1, "{:?}", c);
        assert!(completion_concerns("Done! I fixed the test.", &ledger_with(2, 0, 5), task, false).is_empty());
        // Last call failed → concern even with effects.
        assert_eq!(completion_concerns("Fixed it.", &ledger_with(2, 1, 5), task, true).len(), 1);
    }

    #[test]
    fn completion_check_accepts_plain_answers() {
        let task = "[Project folder: C:\\proj — work inside this folder.]\n\nwhat does main.rs do?";
        assert!(completion_concerns("main.rs is done in three stages: …", &ledger_with(0, 0, 3), task, false).is_empty());
        assert!(completion_concerns("It's a weather app.", &ledger_with(0, 0, 1), "what is on my screen?", false).is_empty());
    }

    #[test]
    fn completion_check_flags_unfinished_plan_and_early_give_up() {
        let mut l = ledger_with(3, 0, 8);
        l.apply_plan_update(&json!({ "plan": ["edit", "test", "commit"], "completed": 1 }));
        let c = completion_concerns("All set — the change is saved.", &l, "update the readme", false);
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("2 unfinished"), "{}", c[0]);

        let c = completion_concerns("I couldn't install it.", &ledger_with(0, 2, 4), "install ffmpeg", false);
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("giving up"));
        // Honest failure after a long attempt is accepted.
        assert!(completion_concerns("I couldn't install it.", &ledger_with(0, 6, 30), "install ffmpeg", false).is_empty());
    }

    #[test]
    fn plan_updates_track_progress_and_notes() {
        let mut l = Ledger::default();
        assert!(!l.apply_plan_update(&json!({ "plan": ["a", "b", "c"], "done_when": "tests pass" })));
        assert!(l.apply_plan_update(&json!({ "completed": 1, "notes": "port 5173" })));
        assert!(!l.apply_plan_update(&json!({ "completed": 1, "notes": "port 5173" })));
        assert!(l.apply_plan_update(&json!({ "completed": 99 })));
        assert_eq!(l.plan_done, 3);
        assert_eq!(l.notes, "port 5173");
        l.plan_done = 1;
        assert_eq!(l.plan_summary_line(), "Plan: ✓ a → b · c");
        assert_eq!(l.plan_lines()[1], "→ 2. b");
        let digest = l.digest(10);
        assert!(digest.contains("Done when: tests pass") && digest.contains("port 5173"));
    }

    #[test]
    fn project_folder_is_parsed_from_the_task_prefix() {
        let t = "[Project folder: C:\\Users\\me\\app — work inside this folder and use absolute paths.]\n\nrun tests";
        assert_eq!(project_folder_from_task(t).as_deref(), Some("C:\\Users\\me\\app"));
        assert_eq!(strip_project_prefix(t), "run tests");
        assert_eq!(project_folder_from_task("[Project folder: D:/x]\nhi").as_deref(), Some("D:/x"));
        assert_eq!(project_folder_from_task("run tests"), None);
    }

    #[test]
    fn tool_failure_detection() {
        assert!(tool_failed("read_file", &json!({ "error": "nope" })));
        assert!(tool_failed("run_command", &json!({ "output": "oops\n[exit code 3]" })));
        assert!(!tool_failed("run_command", &json!({ "output": "all good" })));
        assert!(!tool_failed("read_file", &json!({ "result": "[exit code 3]" })));
    }

    #[test]
    fn recovery_hints_match_common_failures() {
        let h = recovery_hint("run_command", "'pnpm' is not recognized as the name of a cmdlet, function…\n[exit code 1]").unwrap();
        assert!(h.contains("PATH"));
        assert!(recovery_hint("run_command", "command timed out after 45s").unwrap().contains("background=true"));
        assert!(recovery_hint("edit_file", "old_text was not found in C:/a.rs").unwrap().contains("read_file"));
        assert!(recovery_hint("read_file", "could not read C:/x: The system cannot find the file specified. (os error 2)")
            .unwrap()
            .contains("list_directory"));
        assert!(recovery_hint("list_windows", "something odd").is_none());
    }

    #[test]
    fn model_errors_are_classified() {
        assert_eq!(step_error_status("Agent step failed (503): busy"), Some(503));
        assert!(is_retryable_step_error("Agent step failed (503): busy"));
        assert!(is_retryable_step_error("Agent step failed (429) [retry-after=5]: slow down"));
        assert_eq!(retry_after_hint_secs("Agent step failed (429) [retry-after=5]: x"), Some(5));
        assert!(is_retryable_step_error("Agent step failed (408): timeout"));
        assert!(!is_retryable_step_error("Agent step failed (400): bad request"));
        assert!(!is_retryable_step_error("Agent step failed (429): You exceeded your current quota"));
        assert!(is_retryable_step_error("Can't reach the Thothai backend. Check your internet…"));
        assert!(is_retryable_step_error("Network error: error sending request for url"));
        assert!(!is_retryable_step_error("Session expired — please try again."));
        assert!(is_payload_too_large("Agent step failed (413): Payload Too Large"));
        assert!(!is_payload_too_large("Agent step failed (500): oops"));
    }
}

#[cfg(test)]
mod project_prefix_tests {
    use super::project_folder_from_task;

    #[test]
    fn parses_hyphen_em_dash_and_dashed_folder_names() {
        let hyphen = r"[Project folder: C:\code\app - work inside this folder and use absolute paths.]

fix it";
        assert_eq!(project_folder_from_task(hyphen).as_deref(), Some(r"C:\code\app"));
        let em = "[Project folder: C:/code/app \u{2014} work inside this folder.]\n\nfix it";
        assert_eq!(project_folder_from_task(em).as_deref(), Some("C:/code/app"));
        let dashed = r"[Project folder: C:\code\My App - Copy - work inside this folder.]";
        assert_eq!(project_folder_from_task(dashed).as_deref(), Some(r"C:\code\My App - Copy"));
    }
}
