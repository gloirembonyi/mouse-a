// Mouse AI desktop agent - scratch dir and path resolution (~, %VAR%, Documents\x).
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// ── Agent scratch space ──────────────────────────────────────────
// Root cause of files like `yt_search.py` turning up inside the app's own
// source tree: run_command/write_file/read_file resolve a relative path
// against whatever the CURRENT PROCESS's cwd happens to be (src-tauri/ under
// `tauri dev`) when the model doesn't pass an absolute one. Rather than
// trust every future prompt revision to remember an absolute path, relative
// paths now resolve here instead - a real OS temp folder, wiped clean at the
// start of every task, so scratch work literally cannot accumulate or land
// in the user's project regardless of what the model does.
pub(crate) fn agent_scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("mouse-ai-agent-scratch");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Wipes the scratch dir back to empty - called once at the start of every
/// `run_task` so a previous run's throwaway files never linger, even if the
/// model never called delete_file on them itself.
pub(crate) static ACTIVE_RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) struct RunGuard;

impl RunGuard {
    pub(crate) fn enter() -> Self {
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

pub(crate) fn reset_scratch_dir() {
    let dir = agent_scratch_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
}

/// The user's real known folders (Desktop/Documents/Downloads… - dirs uses
/// SHGetKnownFolderPath, so OneDrive-redirected folders resolve correctly).
pub(crate) fn known_folder(name: &str) -> Option<std::path::PathBuf> {
    match name.to_ascii_lowercase().as_str() {
        "~" | "home" | "$home" => dirs::home_dir(),
        "desktop" => dirs::desktop_dir(),
        "documents" | "my documents" => dirs::document_dir(),
        "downloads" => dirs::download_dir(),
        "pictures" => dirs::picture_dir(),
        "music" => dirs::audio_dir(),
        "videos" => dirs::video_dir(),
        _ => None,
    }
}

/// Expand `%VAR%`, `$env:VAR` and `$HOME` the way the model writes them - a file
/// tool would otherwise create a folder literally named "%USERPROFILE%".
pub(crate) fn expand_env_vars(raw: &str) -> String {
    let mut s = raw.to_string();
    for _ in 0..8 {
        let Some(start) = s.find('%') else { break };
        let Some(len) = s[start + 1..].find('%') else { break };
        let name = &s[start + 1..start + 1 + len];
        match std::env::var(name) {
            Ok(val) if !name.is_empty() => s = format!("{}{}{}", &s[..start], val, &s[start + 2 + len..]),
            _ => break,
        }
    }
    while let Some(start) = s.to_ascii_lowercase().find("$env:") {
        let rest = &s[start + 5..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let name = &rest[..end];
        let val = std::env::var(name).unwrap_or_default();
        if name.is_empty() || val.is_empty() {
            break;
        }
        s = format!("{}{}{}", &s[..start], val, &rest[end..]);
    }
    if let Some(rest) = s.strip_prefix("$HOME") {
        if let Some(home) = dirs::home_dir() {
            s = format!("{}{}", home.display(), rest);
        }
    }
    s
}

/// Where a path the model wrote actually points.
///  - `~\x`, `%USERPROFILE%\x`, `$env:USERPROFILE\x` → the real home folder;
///  - `Desktop\x`, `Documents\x`, `Downloads\x`… → the real known folder;
///  - `C:\Users\me\Desktop\x` when that folder doesn't exist but the real Desktop is
///    elsewhere (OneDrive) → the real one, instead of creating a phantom folder the
///    user never sees;
///  - any other relative path → the private scratch folder (never the app's cwd).
pub(crate) fn resolve_agent_path(raw: &str) -> std::path::PathBuf {
    use std::path::{Component, Path, PathBuf};
    let expanded = expand_env_vars(raw.trim().trim_matches('"'));
    let p = Path::new(&expanded);

    let mut comps = p.components();
    if let Some(Component::Normal(first)) = comps.clone().next() {
        let first = first.to_string_lossy().to_string();
        if let Some(base) = known_folder(&first) {
            comps.next();
            return base.join(comps.as_path());
        }
    }
    if p.is_absolute() {
        if let Some(home) = dirs::home_dir() {
            if let Ok(rest) = p.strip_prefix(&home) {
                let mut rc = rest.components();
                if let Some(Component::Normal(first)) = rc.next() {
                    let first = first.to_string_lossy().to_string();
                    if let Some(real) = known_folder(&first) {
                        if real != home.join(&first) && !home.join(&first).exists() {
                            return real.join(rc.as_path());
                        }
                    }
                }
            }
        }
        return p.to_path_buf();
    }
    agent_scratch_dir().join(PathBuf::from(p))
}
