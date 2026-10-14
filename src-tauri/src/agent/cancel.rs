// Mouse AI desktop agent - Stop requests.
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
