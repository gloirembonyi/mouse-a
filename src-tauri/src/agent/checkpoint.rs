// Kudu Desktop agent - run checkpoints, so "continue" actually continues.
//
// Every tool round saves the ledger (plan, notes, step log, data) to
// %APPDATA%/mouse-ai/runs/<source>.json. When the next task from the same source is a
// continue-phrase ("continue", "keep going", "finish it"), or the same task again, and
// the file is recent, run_task seeds the new run with a [RESUMING] digest instead of
// starting from nothing. Mirrors the browser extension's checkpoint (browser-agent
// ledgerCheckpoint / createLedger). The transcript is deliberately NOT persisted.

#[allow(unused_imports)]
use crate::agent::*;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RunCheckpoint {
    pub(crate) version: u32,
    pub(crate) source: String,
    pub(crate) task: String,
    /// running | incomplete | cancelled
    pub(crate) status: String,
    pub(crate) updated_at_ms: u64,
    pub(crate) ledger: LedgerSnapshot,
}

const RESUME_WINDOW_MS: u64 = 30 * 60 * 1000;
const SWEEP_AFTER_MS: u64 = 24 * 60 * 60 * 1000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn runs_dir() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("mouse-ai")
        .join("runs")
}

fn path_for(source: &str) -> std::path::PathBuf {
    let safe: String = source
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    runs_dir().join(format!("{}.json", safe))
}

/// Persist the run's ledger with a status. Atomic (tmp + rename); failures are ignored -
/// a checkpoint is a convenience, never a reason to fail a run.
pub(crate) fn save(source: &str, task: &str, status: &str, ledger: &Ledger) {
    let cp = RunCheckpoint {
        version: 1,
        source: source.to_string(),
        task: task.to_string(),
        status: status.to_string(),
        updated_at_ms: now_ms(),
        ledger: ledger.snapshot(),
    };
    let path = path_for(source);
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_vec(&cp) {
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

pub(crate) fn clear(source: &str) {
    let _ = std::fs::remove_file(path_for(source));
}

/// Drop checkpoints nobody will resume (older than a day).
pub(crate) fn sweep() {
    let Ok(entries) = std::fs::read_dir(runs_dir()) else { return };
    let now = now_ms();
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        let age = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| now.saturating_sub(d.as_millis() as u64))
            .unwrap_or(u64::MAX);
        if age > SWEEP_AFTER_MS {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

const CONTINUE_PHRASES: &[&str] = &[
    "continue", "resume", "keep going", "carry on", "go on", "proceed", "finish it",
    "finish the task", "finish up", "pick up where you left off", "continue please", "please continue",
];

/// If `task` asks to continue (or repeats the prior task) and a fresh, unfinished
/// checkpoint exists for `source`, return it plus whatever the user added after the
/// continue phrase ("continue, but skip the zip step").
pub(crate) fn load_for_resume(source: &str, task: &str) -> Option<(RunCheckpoint, Option<String>)> {
    let raw = std::fs::read(path_for(source)).ok()?;
    let cp: RunCheckpoint = serde_json::from_slice(&raw).ok()?;
    if now_ms().saturating_sub(cp.updated_at_ms) > RESUME_WINDOW_MS {
        return None;
    }
    if !matches!(cp.status.as_str(), "running" | "incomplete" | "cancelled") {
        return None;
    }
    let normalized = normalize_words(task);
    let prior = normalize_words(&cp.task);
    if !prior.is_empty() && normalized == prior {
        return Some((cp, None));
    }
    for phrase in CONTINUE_PHRASES {
        if normalized == *phrase {
            return Some((cp, None));
        }
        if let Some(rest) = normalized.strip_prefix(&format!("{} ", phrase)) {
            let rest = rest.trim_start_matches(|c: char| c == ',' || c == ':' || c == '-' || c.is_whitespace()).trim();
            return Some((cp, if rest.is_empty() { None } else { Some(rest.to_string()) }));
        }
    }
    None
}
