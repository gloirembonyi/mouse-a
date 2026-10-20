// Kudu Desktop agent - Stop requests.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// ── Cancellation ─────────────────────────────────────────────────
// A long agent run used to be unstoppable short of killing the app - the UI
// showed a spinner for minutes with no way out. The Stop button sets a flag
// here (keyed by the same `source` string that routes progress events), and
// the drive loop checks it between model steps and between tool calls.
pub(crate) static CANCEL_REQUESTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub(crate) fn cancels() -> &'static Mutex<HashSet<String>> {
    CANCEL_REQUESTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Sources with a run in flight - so "stop everything" (the Esc key, the
/// banner's Stop) knows whom to stop without the caller naming a source.
static ACTIVE_RUNS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn active_runs() -> &'static Mutex<HashSet<String>> {
    ACTIVE_RUNS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(crate) fn begin_run(source: &str) {
    active_runs().lock().unwrap().insert(source.to_string());
}

pub(crate) fn end_run(source: &str) {
    active_runs().lock().unwrap().remove(source);
    steer().lock().unwrap().remove(source);
    // Hand the mouse/keyboard back at once (agent/scheduler.rs).
    crate::agent::scheduler::release_input(source);
}

/// Whether a run with this source is in flight.
pub fn is_run_active(source: &str) -> bool {
    active_runs().lock().unwrap().contains(source)
}

/// Every run in flight (the Agents view's "all runs" board).
pub fn active_sources() -> Vec<String> {
    let mut v: Vec<String> = active_runs().lock().unwrap().iter().cloned().collect();
    v.sort();
    v
}

/// Stop requested but not yet consumed by the loop - for waits that must not eat the flag.
pub(crate) fn is_cancel_pending(source: &str) -> bool {
    cancels().lock().unwrap().contains(source)
}

// ── Steering: talk to a run while it works ──────────────────────
// The user keeps typing while a task runs: "also rename the file", "no, the other
// tab", "check the video". Each message is queued here per source and the loop
// folds it into the conversation at its next step (or before it finishes), so a
// run never has to be stopped and restarted to be corrected.
static STEER: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();

fn steer() -> &'static Mutex<HashMap<String, Vec<String>>> {
    STEER.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Queue a message for a running source. Returns false when no run with that
/// source is active - the caller should start a new task instead.
pub fn request_steer(source: &str, text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || !active_runs().lock().unwrap().contains(source) {
        return false;
    }
    steer().lock().unwrap().entry(source.to_string()).or_default().push(text.to_string());
    true
}

/// Everything queued for a source since the last call.
pub(crate) fn take_steer(source: &str) -> Vec<String> {
    steer().lock().unwrap().remove(source).unwrap_or_default()
}

/// Stop every run in flight. Returns how many were asked to stop.
pub fn request_cancel_all() -> usize {
    let sources: Vec<String> = active_runs().lock().unwrap().iter().cloned().collect();
    for s in &sources {
        request_cancel(s);
    }
    sources.len()
}

/// Called by the `agent_cancel` Tauri command (the UI's Stop button).
pub fn request_cancel(source: &str) {
    cancels().lock().unwrap().insert(source.to_string());
    // Also cancel any web task this run delegated to the browser extension.
    crate::bridge::bridge().cancel_source(source);
}

/// Check-and-consume: returns true at most once per request_cancel call.
pub(crate) fn cancel_requested(source: &str) -> bool {
    cancels().lock().unwrap().remove(source)
}

/// Non-consuming peek - used while waiting between model retries so a Stop
/// cuts the wait short but is still seen (and consumed) by the loop itself.
pub(crate) fn cancel_pending(source: &str) -> bool {
    cancels().lock().unwrap().contains(source)
}
