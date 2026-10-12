// Mouse AI Desktop — Agent link: pairing, the shared task board, and the watcher.
//
// bridge.rs is the transport (the localhost WebSocket to the browser
// extension). This module is what turns it into ONE system of two agents:
//
//   • Both directions. The desktop agent hands web work to the browser agent
//     (`delegate_to_browser`), and the browser agent hands PC work — terminal,
//     files, apps — to the desktop agent (`desktop_task` frames). A task that
//     arrived from one side may not delegate back to it, so they can't ping-pong.
//
//   • Pairing. Running shell commands on request is powerful, so tasks FROM the
//     browser are only accepted from a connection that presented this app's
//     pairing code (Settings → Browser link) — merely being a browser
//     extension, or a local process, is not enough. The user can also switch
//     incoming tasks off entirely.
//
//   • The watcher. Every cross-agent task is a record on a shared board
//     (`bridge-task` events → the Agents panel): who asked, every step, and a
//     status. A background loop notices when a task goes quiet, marks it
//     "stalled", asks the other side what it's doing (`status_request`), and
//     fails it if the other side has lost it or stops answering — so neither
//     agent hangs for ten minutes on a task that died with a restarted browser.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Listener, Manager};

use crate::bridge::bridge;

/// No progress for this long → "stalled" + ask the other side.
const STALL_AFTER_MS: i64 = 45_000;
/// Re-ask a stalled task at most this often.
const REPROBE_EVERY_MS: i64 = 30_000;
/// A browser task silent this long (with no answer to our probes) is failed.
const GIVE_UP_AFTER_MS: i64 = 300_000;
const MAX_STEPS: usize = 80;
const KEEP_FINISHED: usize = 30;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── Pairing config ───────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct LinkConfig {
    pair_code: String,
    accept_tasks: bool,
}

fn config_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("mouse-ai").join("bridge.json"))
}

fn new_code() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
    format!("{}-{}-{}", &hex[0..4], &hex[4..8], &hex[8..12])
}

fn config() -> &'static Mutex<LinkConfig> {
    static CFG: OnceLock<Mutex<LinkConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let loaded = config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<LinkConfig>(&s).ok())
            .filter(|c| !c.pair_code.trim().is_empty());
        let cfg = loaded.unwrap_or_else(|| {
            let c = LinkConfig { pair_code: new_code(), accept_tasks: true };
            save_config(&c);
            c
        });
        Mutex::new(cfg)
    })
}

fn save_config(cfg: &LinkConfig) {
    if let Some(p) = config_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serde_json::to_string_pretty(cfg).unwrap_or_default());
    }
}

/// Constant-time-ish comparison; codes are short, but there's no reason to
/// leak how many leading characters matched.
pub fn code_matches(candidate: &str) -> bool {
    let expected = config().lock().unwrap().pair_code.clone();
    let a = candidate.trim().to_uppercase();
    if a.len() != expected.len() {
        return false;
    }
    a.bytes().zip(expected.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn accepts_tasks() -> bool {
    config().lock().unwrap().accept_tasks
}

// ── Task board ───────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct Step {
    pub kind: String,
    pub message: String,
    pub at_ms: i64,
}

#[derive(Serialize, Clone)]
pub struct TaskRecord {
    pub id: String,
    /// "to_browser" (desktop asked the browser) | "from_browser" (browser asked us)
    pub direction: String,
    pub title: String,
    /// queued | running | waiting (for the user's Allow/Deny) | stalled | done | failed | cancelled
    pub status: String,
    pub steps: Vec<Step>,
    pub started_ms: i64,
    pub updated_ms: i64,
    pub result: Option<String>,
    pub error: Option<String>,
    #[serde(skip)]
    last_probe_ms: i64,
}

fn board() -> &'static Mutex<Vec<TaskRecord>> {
    static BOARD: OnceLock<Mutex<Vec<TaskRecord>>> = OnceLock::new();
    BOARD.get_or_init(|| Mutex::new(Vec::new()))
}

fn app_handle() -> &'static OnceLock<AppHandle> {
    static APP: OnceLock<AppHandle> = OnceLock::new();
    &APP
}

fn emit_task(rec: &TaskRecord) {
    if let Some(app) = app_handle().get() {
        let _ = app.emit("bridge-task", rec);
    }
}

fn is_open(status: &str) -> bool {
    matches!(status, "queued" | "running" | "waiting" | "stalled")
}

fn short(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

pub fn task_started(id: &str, direction: &str, title: &str, status: &str) {
    let now = now_ms();
    let rec = TaskRecord {
        id: id.to_string(),
        direction: direction.to_string(),
        title: short(title, 160),
        status: status.to_string(),
        steps: Vec::new(),
        started_ms: now,
        updated_ms: now,
        result: None,
        error: None,
        last_probe_ms: 0,
    };
    let mut b = board().lock().unwrap();
    b.retain(|t| t.id != id);
    b.push(rec.clone());
    // Drop the oldest finished records beyond the retention cap.
    let finished = b.iter().filter(|t| !is_open(&t.status)).count();
    if finished > KEEP_FINISHED {
        let mut to_drop = finished - KEEP_FINISHED;
        b.retain(|t| {
            if to_drop > 0 && !is_open(&t.status) {
                to_drop -= 1;
                false
            } else {
                true
            }
        });
    }
    drop(b);
    emit_task(&rec);
}

fn update<F: FnOnce(&mut TaskRecord)>(id: &str, f: F) -> Option<TaskRecord> {
    let mut b = board().lock().unwrap();
    let rec = b.iter_mut().find(|t| t.id == id)?;
    f(rec);
    let out = rec.clone();
    drop(b);
    emit_task(&out);
    Some(out)
}

/// Any sign of life: record the step and clear a "stalled" flag.
pub fn task_step(id: &str, kind: &str, message: &str) {
    update(id, |t| {
        if !is_open(&t.status) {
            return;
        }
        t.updated_ms = now_ms();
        t.status = if kind == "confirm" { "waiting".into() } else { "running".into() };
        if !message.trim().is_empty() {
            t.steps.push(Step { kind: kind.to_string(), message: short(message, 400), at_ms: t.updated_ms });
            if t.steps.len() > MAX_STEPS {
                let extra = t.steps.len() - MAX_STEPS;
                t.steps.drain(0..extra);
            }
        }
    });
}

pub fn task_finished(id: &str, result: Result<&str, &str>) {
    update(id, |t| {
        if !is_open(&t.status) {
            return;
        }
        t.updated_ms = now_ms();
        match result {
            Ok(r) => {
                t.status = "done".into();
                t.result = Some(short(r, 800));
            }
            Err(e) if e == "cancelled" => t.status = "cancelled".into(),
            Err(e) => {
                t.status = "failed".into();
                t.error = Some(short(e, 400));
            }
        }
    });
}

fn last_step(id: &str) -> Option<(String, String)> {
    let b = board().lock().unwrap();
    let t = b.iter().find(|t| t.id == id)?;
    Some((t.status.clone(), t.steps.last().map(|s| s.message.clone()).unwrap_or_default()))
}

// ── Incoming tasks (browser → desktop) ───────────────────────────

fn inbound_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Runs a task the browser agent handed us, through the normal desktop agent
/// loop (same tools, same confirmation gate — its Allow/Deny prompts appear in
/// the Agents panel). One at a time: incoming tasks queue behind each other.
pub fn handle_desktop_task(v: &Value, paired: bool) {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    if id.is_empty() {
        return;
    }
    let task = v.get("task").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let reject = |msg: &str| {
        bridge().send_json(json!({ "v": 1, "type": "desktop_result", "id": id, "ok": false, "error": msg }));
    };
    if !paired {
        reject("This browser isn't paired with Mouse AI. Copy the pairing code from Mouse AI → Settings → Browser link into the extension's settings.");
        return;
    }
    if !accepts_tasks() {
        reject("Mouse AI is set to not accept tasks from the browser (Settings → Browser link).");
        return;
    }
    if task.is_empty() {
        reject("Empty task.");
        return;
    }
    let Some(app) = app_handle().get().cloned() else {
        reject("The desktop app isn't ready yet.");
        return;
    };
    let cwd = v
        .get("options")
        .and_then(|o| o.get("cwd"))
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    task_started(&id, "from_browser", &task, "queued");
    bridge().send_json(json!({ "v": 1, "type": "desktop_accepted", "id": id }));

    tauri::async_runtime::spawn(async move {
        let _turn = inbound_lock().lock().await;
        task_step(&id, "plan", "Starting on the desktop");
        let source = format!("browser:{}", id);
        let mut prompt = String::from(
            "[This task was handed to you by the Thothai BROWSER agent, which is working on a larger job in the user's browser. \
Do the PC-side part (terminal, files, folders, apps), then answer with a concise result the browser agent can use \
(paths written, command output, values found). Do not delegate back to the browser.]\n\n",
        );
        if let Some(dir) = &cwd {
            prompt.push_str(&format!("[Working folder: {} — use absolute paths under it.]\n\n", dir));
        }
        prompt.push_str(&task);

        let backend = app.state::<crate::commands::AppState>().backend.clone();
        let (cx, cy) = crate::mouse::get_cursor_position();
        let result = crate::agent::run_task(&app, &backend, &prompt, cx, cy, &source, true, None, None).await;
        match result {
            Ok(text) => {
                task_finished(&id, Ok(&text));
                bridge().send_json(json!({ "v": 1, "type": "desktop_result", "id": id, "ok": true, "result": text }));
            }
            Err(e) => {
                task_finished(&id, Err(&e));
                bridge().send_json(json!({ "v": 1, "type": "desktop_result", "id": id, "ok": false, "error": e }));
            }
        }
    });
}

pub fn handle_desktop_cancel(v: &Value) {
    if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
        crate::agent::request_cancel(&format!("browser:{}", id));
        task_step(id, "tool-result", "Cancel requested by the browser");
    }
}

/// The browser asks how one of OUR runs (a desktop_task it sent) is doing.
pub fn handle_status_request(v: &Value) {
    let Some(id) = v.get("id").and_then(|x| x.as_str()) else { return };
    let (state, step) = match last_step(id) {
        Some((status, step)) if is_open(&status) => ("running", step),
        _ => ("unknown", String::new()),
    };
    bridge().send_json(json!({ "v": 1, "type": "status", "id": id, "state": state, "step": step }));
}

/// The browser answered our probe about a task we delegated to it.
pub fn handle_status_reply(v: &Value) {
    let Some(id) = v.get("id").and_then(|x| x.as_str()) else { return };
    match v.get("state").and_then(|x| x.as_str()) {
        Some("running") => {
            let step = v.get("step").and_then(|x| x.as_str()).unwrap_or("");
            task_step(id, "tool-result", &format!("Browser agent still working{}", if step.is_empty() { String::new() } else { format!(": {}", step) }));
        }
        _ => bridge().fail_task(id, "The browser agent lost this task (the extension may have restarted). Try again."),
    }
}

// ── Watcher ──────────────────────────────────────────────────────

pub fn start(app: AppHandle) {
    let _ = app_handle().set(app.clone());
    let _ = config(); // create the pairing code on first launch

    // Mirror every step of a browser-requested run back to the browser, and
    // onto the task board.
    app.listen("agent-progress", |event| {
        let Ok(p) = serde_json::from_str::<Value>(event.payload()) else { return };
        let Some(id) = p.get("source").and_then(|s| s.as_str()).and_then(|s| s.strip_prefix("browser:")) else { return };
        let kind = p.get("kind").and_then(|k| k.as_str()).unwrap_or("tool");
        let message = p.get("message").and_then(|m| m.as_str()).unwrap_or("");
        if kind == "done" {
            return;
        }
        task_step(id, kind, message);
        bridge().send_json(json!({ "v": 1, "type": "desktop_progress", "id": id, "kind": kind, "message": message }));
    });

    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            watch_tick();
        }
    });
}

fn watch_tick() {
    let now = now_ms();
    let mut probes = Vec::new();
    let mut give_up = Vec::new();
    {
        let mut b = board().lock().unwrap();
        for t in b.iter_mut() {
            if !matches!(t.status.as_str(), "running" | "stalled") {
                continue;
            }
            let idle = now - t.updated_ms;
            if idle < STALL_AFTER_MS {
                continue;
            }
            if t.status != "stalled" {
                t.status = "stalled".into();
                emit_task(t);
            }
            if t.direction == "to_browser" {
                if idle > GIVE_UP_AFTER_MS {
                    give_up.push(t.id.clone());
                } else if now - t.last_probe_ms > REPROBE_EVERY_MS {
                    t.last_probe_ms = now;
                    probes.push(t.id.clone());
                }
            }
        }
    }
    for id in probes {
        bridge().send_json(json!({ "v": 1, "type": "status_request", "id": id }));
    }
    for id in give_up {
        bridge().fail_task(&id, "The browser agent stopped responding (no progress for 5 minutes).");
    }
}

// ── Commands ─────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct LinkInfo {
    pub connected: bool,
    pub paired: bool,
    pub port: u16,
    pub pair_code: String,
    pub accept_tasks: bool,
}

#[tauri::command]
pub fn link_info() -> LinkInfo {
    let b = bridge();
    let cfg = config().lock().unwrap().clone();
    LinkInfo {
        connected: b.is_connected(),
        paired: b.is_paired(),
        port: b.port(),
        pair_code: cfg.pair_code,
        accept_tasks: cfg.accept_tasks,
    }
}

#[tauri::command]
pub fn link_set_accept_tasks(accept: bool) -> LinkInfo {
    {
        let mut cfg = config().lock().unwrap();
        cfg.accept_tasks = accept;
        save_config(&cfg);
    }
    link_info()
}

/// New code → the currently connected browser is no longer trusted until the
/// user enters the new code there.
#[tauri::command]
pub fn link_new_pair_code() -> LinkInfo {
    {
        let mut cfg = config().lock().unwrap();
        cfg.pair_code = new_code();
        save_config(&cfg);
    }
    bridge().unpair();
    link_info()
}

#[tauri::command]
pub fn link_tasks() -> Vec<TaskRecord> {
    board().lock().unwrap().clone()
}

#[tauri::command]
pub fn link_cancel_task(id: String) {
    let direction = board().lock().unwrap().iter().find(|t| t.id == id).map(|t| t.direction.clone());
    match direction.as_deref() {
        Some("from_browser") => crate::agent::request_cancel(&format!("browser:{}", id)),
        Some(_) => {
            bridge().cancel_task(&id);
            bridge().fail_task(&id, "cancelled");
        }
        None => {}
    }
}

#[tauri::command]
pub fn link_clear_finished() -> Vec<TaskRecord> {
    let mut b = board().lock().unwrap();
    b.retain(|t| is_open(&t.status));
    b.clone()
}

/// Stop every run the browser asked for (it disconnected, so the results
/// would have nowhere to go).
pub fn cancel_incoming(reason: &str) {
    let ids: Vec<String> = board()
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.direction == "from_browser" && is_open(&t.status))
        .map(|t| t.id.clone())
        .collect();
    for id in ids {
        crate::agent::request_cancel(&format!("browser:{}", id));
        task_step(&id, "error", reason);
    }
}
