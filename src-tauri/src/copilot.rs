// Mouse AI Desktop - in-call copilot: hands-on tools.
//
// meeting.rs listens and answers; this module lets the copilot bar *act*
// while the user is presenting, without anything showing up on the shared
// screen that they didn't choose to show:
//
//   • Fix it - runs the full desktop agent (agent.rs: shell, files, search,
//     the browser agent via the link) on "what just broke in my demo", with
//     the recent transcript and the user's active project as context. Its
//     live steps stream to the bar under source "copilot".
//   • Private insert - pastes or types an answer / code block into the app
//     the user was working in. The bar is invisible to screen share, so the
//     audience only sees the text land, as if the user typed it.
//
// The bar takes focus when clicked, so we remember the last foreground
// window that isn't ours and hand focus back to it before inserting.

use crate::commands::AppState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;
use tauri::{AppHandle, State};

pub const AGENT_SOURCE: &str = "copilot";

/// HWND (as isize) of the last non-Mouse-AI foreground window; 0 = unknown.
static TARGET_WINDOW: AtomicIsize = AtomicIsize::new(0);
static INSERT_CANCEL: AtomicBool = AtomicBool::new(false);
static INSERTING: AtomicBool = AtomicBool::new(false);

// ── Foreground tracking ──────────────────────────────────────────

#[cfg(windows)]
mod win {
    use winapi::shared::windef::{HWND, RECT};
    use winapi::um::winuser::{
        GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, SetForegroundWindow,
    };

    pub fn foreground() -> isize {
        unsafe { GetForegroundWindow() as isize }
    }

    pub fn is_ours(hwnd: isize) -> bool {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd as HWND, &mut pid) };
        pid == std::process::id()
    }

    pub fn is_valid(hwnd: isize) -> bool {
        hwnd != 0 && unsafe { IsWindow(hwnd as HWND) } != 0
    }

    pub fn activate(hwnd: isize) -> bool {
        unsafe {
            if IsIconic(hwnd as HWND) != 0 {
                winapi::um::winuser::ShowWindow(hwnd as HWND, winapi::um::winuser::SW_RESTORE);
            }
            SetForegroundWindow(hwnd as HWND) != 0
        }
    }

    pub fn center(hwnd: isize) -> Option<(i32, i32)> {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if unsafe { GetWindowRect(hwnd as HWND, &mut r) } == 0 || r.right <= r.left {
            return None;
        }
        Some(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
    }
}

/// Remember the window the user is working in, if it isn't one of ours.
/// Called when the bar is shown or clicked - both happen while the user's
/// app still has focus.
pub fn remember_foreground() {
    #[cfg(windows)]
    {
        let hwnd = win::foreground();
        if hwnd != 0 && !win::is_ours(hwnd) {
            TARGET_WINDOW.store(hwnd, Ordering::Relaxed);
        }
    }
}

/// Keeps the insert target current: whichever non-Mouse-AI window the user
/// last worked in. Clicking the bar makes it the foreground window before any
/// of our code runs, so sampling only at click time would be too late.
pub fn start_foreground_watch() {
    let _ = std::thread::Builder::new().name("copilot-foreground".into()).spawn(|| loop {
        remember_foreground();
        std::thread::sleep(Duration::from_millis(250));
    });
}

fn target_window() -> Option<isize> {
    let hwnd = TARGET_WINDOW.load(Ordering::Relaxed);
    #[cfg(windows)]
    if !win::is_valid(hwnd) {
        return None;
    }
    (hwnd != 0).then_some(hwnd)
}

// ── Fix it: the agent, from the bar ──────────────────────────────

const FIX_DEFAULT: &str = "Something just went wrong in my live demo. Look at my screen and the recent conversation, \
find the cause (read the error, search and read the relevant project files, run commands to reproduce or check), \
fix it with the smallest safe change, and verify the fix.";

fn agent_task(request: &str, project: Option<&str>) -> String {
    let request = request.trim();
    let request = if request.is_empty() { FIX_DEFAULT } else { request };
    let transcript = crate::meeting::recent_transcript(3_000);
    let mut task = String::from(
        "[LIVE SESSION] The user is in a call and may be sharing their screen right now. Work like a quiet pair \
programmer behind them:\n\
- Prefer background tools (run_command, find_files, read_file, write_file, web_search, web_fetch, the browser agent). \
Only click, type or open windows on screen if the task truly needs it - the audience sees every mouse move.\n\
- Never run anything destructive; ask with request_confirmation before overwriting files outside the project, \
installing things, or anything hard to undo.\n\
- Finish with a short answer the user can glance at mid-call: what was wrong, what you changed (file paths), and \
anything they should say or do next. Put any code they may need to type in fenced code blocks.\n",
    );
    if let Some(p) = project.map(str::trim).filter(|p| !p.is_empty()) {
        task.push_str(&format!("\n[ACTIVE PROJECT] {}\n(Relative paths and searches should start here.)\n", p));
    }
    if !transcript.is_empty() {
        task.push_str(&format!("\n[RECENT CONVERSATION - context only]\n{}\n", transcript));
    }
    task.push_str(&format!("\n[REQUEST]\n{}", request));
    task
}

#[derive(Serialize)]
pub struct CopilotAgentResult {
    pub text: String,
}

/// Run the desktop agent from the copilot bar. Progress streams as
/// `agent-progress` events with source "copilot".
#[tauri::command]
pub async fn copilot_agent_run(
    app: AppHandle,
    state: State<'_, AppState>,
    task: String,
    project: Option<String>,
) -> Result<CopilotAgentResult, String> {
    let full = agent_task(&task, project.as_deref());
    // Ground the agent's first screenshot in the app the user is presenting,
    // not in the bar (which is invisible to capture anyway).
    #[cfg(windows)]
    let point = target_window().and_then(win::center);
    #[cfg(not(windows))]
    let point: Option<(i32, i32)> = None;
    let (cx, cy) = point.unwrap_or_else(crate::mouse::get_cursor_position);
    let text = crate::agent::run_task(&app, &state.backend, &full, cx, cy, AGENT_SOURCE, true, None, None).await?;
    let summary = if task.trim().is_empty() { "Copilot: fix my demo" } else { task.trim() };
    let _ = state.backend.log_agent_run(None, summary, &text).await;
    Ok(CopilotAgentResult { text })
}

// ── Private insert ───────────────────────────────────────────────

fn jitter_ms(seed: &mut u64, lo: u64, hi: u64) -> u64 {
    // xorshift - just enough irregularity that typing doesn't look robotic.
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    lo + *seed % (hi - lo + 1)
}

/// Still safe to keep typing: not cancelled, and the user hasn't clicked
/// somewhere else (typing into whatever has focus now would be a disaster).
fn may_continue(target: isize) -> bool {
    if INSERT_CANCEL.load(Ordering::Relaxed) {
        return false;
    }
    #[cfg(windows)]
    {
        win::foreground() == target
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        true
    }
}

fn paste(text: &str) -> Result<(), String> {
    // Shared with the agent's type_text paste mode (input.rs): clipboard set, Ctrl+V,
    // clipboard restored so the user's own clipboard isn't clobbered.
    crate::input::paste_via_clipboard(text)
}

/// Types like a person: per-character jitter, and line by line so editors'
/// auto-indent doesn't double the indentation (each new line's auto-indent is
/// selected with Shift+Home and replaced by the real indentation).
fn type_naturally(text: &str, target: isize) -> Result<bool, String> {
    let mut seed = (chrono::Utc::now().timestamp_nanos_opt().unwrap_or(1) as u64) | 1;
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            // Close any autocomplete popup so Enter inserts a newline instead
            // of accepting a suggestion.
            crate::input::press_combo("esc")?;
            crate::input::press_combo("enter")?;
            std::thread::sleep(Duration::from_millis(25));
            crate::input::press_combo("shift+home")?;
        }
        for ch in line.chars() {
            if !may_continue(target) {
                return Ok(false);
            }
            let mut buf = [0u8; 4];
            crate::input::type_text(ch.encode_utf8(&mut buf));
            let pause = if ch == ' ' { jitter_ms(&mut seed, 25, 70) } else { jitter_ms(&mut seed, 12, 45) };
            std::thread::sleep(Duration::from_millis(pause));
        }
    }
    Ok(true)
}

#[derive(Serialize)]
pub struct InsertResult {
    /// false = stopped early (cancelled, or the user clicked elsewhere).
    pub completed: bool,
}

/// Put `text` into the app the user was working in. `mode`: "paste" (instant,
/// via the clipboard, which is restored afterwards) or "type" (natural typing).
#[tauri::command]
pub async fn copilot_insert(text: String, mode: String) -> Result<InsertResult, String> {
    if text.trim().is_empty() {
        return Err("Nothing to insert.".to_string());
    }
    let target = target_window().ok_or("Click into the app you want to insert into first, then try again.")?;
    if INSERTING.swap(true, Ordering::Relaxed) {
        return Err("Already inserting - stop it first.".to_string());
    }
    INSERT_CANCEL.store(false, Ordering::Relaxed);
    let result = tokio::task::spawn_blocking(move || -> Result<InsertResult, String> {
        #[cfg(windows)]
        {
            win::activate(target);
            std::thread::sleep(Duration::from_millis(160));
            if win::foreground() != target {
                return Err("Couldn't switch back to your app - click into it and try again.".to_string());
            }
        }
        if mode == "type" {
            type_naturally(&text, target).map(|completed| InsertResult { completed })
        } else {
            paste(&text).map(|_| InsertResult { completed: true })
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e));
    INSERTING.store(false, Ordering::Relaxed);
    result?
}

#[tauri::command]
pub fn copilot_insert_cancel() {
    INSERT_CANCEL.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_request_becomes_fix_task_with_project() {
        let t = agent_task("  ", Some("C:\\code\\app"));
        assert!(t.contains(FIX_DEFAULT));
        assert!(t.contains("[ACTIVE PROJECT] C:\\code\\app"));
    }

    #[test]
    fn jitter_stays_in_range() {
        let mut seed = 42;
        for _ in 0..1000 {
            let v = jitter_ms(&mut seed, 12, 45);
            assert!((12..=45).contains(&v));
        }
    }
}
