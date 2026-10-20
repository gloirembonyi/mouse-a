// Kudu Desktop - usage analytics.
//
// Counts which features get used (app opened, an agent task finished and how, the browser
// linked...), never what is on screen, typed, said or answered. Events go to Kudu Core's
// /api/analytics/collect, which allow-lists names, drops anything that looks like text, an
// address or a URL, and forwards to Google Analytics 4. Off when Settings -> Desktop ->
// "Share anonymous usage statistics" is off (prefs.share_usage_stats).

use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const FLUSH_DELAY: Duration = Duration::from_secs(4);
const MAX_BATCH: usize = 20;
const SESSION_IDLE_SECS: u64 = 30 * 60;

static QUEUE: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static SCHEDULED: AtomicBool = AtomicBool::new(false);
static SESSION_ID: AtomicU64 = AtomicU64::new(0);
static LAST_EVENT: AtomicU64 = AtomicU64::new(0);

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Anonymous per-install id (not the account), kept next to the other prefs.
fn client_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let path = dirs::config_dir().map(|d| d.join("mouse-ai").join("analytics_id"));
        if let Some(p) = &path {
            if let Ok(s) = std::fs::read_to_string(p) {
                let s = s.trim().to_string();
                if s.len() >= 8 {
                    return s;
                }
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        if let Some(p) = &path {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(p, &id);
        }
        id
    })
    .clone()
}

/// GA session: a new one after 30 minutes without events.
fn session_id() -> u64 {
    let now = now_secs();
    let last = LAST_EVENT.swap(now, Ordering::SeqCst);
    let current = SESSION_ID.load(Ordering::SeqCst);
    if current == 0 || now.saturating_sub(last) > SESSION_IDLE_SECS {
        SESSION_ID.store(now, Ordering::SeqCst);
        now
    } else {
        current
    }
}

/// Keep only short scalar params (enums, counts, flags) - the server re-checks.
fn scrub(params: Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Value::Object(map) = params {
        for (k, v) in map {
            match &v {
                Value::Bool(_) | Value::Number(_) => {
                    out.insert(k, v);
                }
                Value::String(s) if s.len() <= 64 && !s.contains(char::is_whitespace) => {
                    out.insert(k, v);
                }
                _ => {}
            }
        }
    }
    Value::Object(out)
}

/// Queue one event. Never blocks, never fails the caller.
pub fn track(app: &AppHandle, name: &str, params: Value) {
    if !crate::prefs::get().share_usage_stats {
        return;
    }
    {
        let mut q = QUEUE.lock().unwrap();
        q.push(json!({ "name": name, "params": scrub(params) }));
        if q.len() > 200 {
            let excess = q.len() - 200;
            q.drain(0..excess);
        }
    }
    let _ = session_id();
    if !SCHEDULED.swap(true, Ordering::SeqCst) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FLUSH_DELAY).await;
            SCHEDULED.store(false, Ordering::SeqCst);
            flush(&app).await;
        });
    }
}

async fn flush(app: &AppHandle) {
    loop {
        let batch: Vec<Value> = {
            let mut q = QUEUE.lock().unwrap();
            let n = q.len().min(MAX_BATCH);
            q.drain(0..n).collect()
        };
        if batch.is_empty() {
            return;
        }
        if !crate::prefs::get().share_usage_stats {
            return; // switched off while queued: drop
        }
        let body = json!({
            "app": "desktop",
            "appVersion": env!("CARGO_PKG_VERSION"),
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "clientId": client_id(),
            "sessionId": SESSION_ID.load(Ordering::SeqCst).to_string(),
            "events": batch,
        });
        let state = app.state::<crate::commands::AppState>();
        state.backend.post_analytics(body).await;
    }
}

/// The frontend's events (update installed, meeting started, settings...).
#[tauri::command]
pub fn analytics_track(app: AppHandle, name: String, params: Option<Value>) {
    track(&app, &name, params.unwrap_or(Value::Null));
}
