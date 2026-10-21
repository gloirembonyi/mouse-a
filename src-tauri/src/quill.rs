// Kudu Desktop - Kudu Quill anywhere: spelling and grammar for the text you are writing in ANY app.
//
// Ctrl+Alt+G copies what is selected in the app you are in (or, when nothing is selected, the whole
// field), opens the small Quill window next to your cursor with the checked text, and "Apply" pastes
// the corrected text back over it. The checking itself runs in that window's webview with the same
// rule engine and dictionary the browser extension uses (frontend/src/lib/quill), so it works with no
// internet and no sign-in.
//
// Why copy/paste and not reading the control directly: it works in every app that has a selection and
// a clipboard - Word, Notepad, Outlook, LibreOffice, Slack, a browser - where UI Automation's
// TextPattern only exists in some. There is no keyboard hook: this is one registered hotkey, pressed
// on purpose. The clipboard is restored afterwards (text only, like `input::paste_via_clipboard`).

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const WINDOW: &str = "quill";

#[derive(Serialize, Clone)]
pub struct QuillCapture {
    pub text: String,
    /// "selection" or "all" (nothing was selected, so the whole field was taken).
    pub scope: &'static str,
    /// Foreground window title, shown so the user sees which app the text came from.
    pub app: String,
    pub error: Option<String>,
}

/// The newest capture, for a window that was still loading when the event fired.
fn pending() -> &'static Mutex<Option<QuillCapture>> {
    static P: OnceLock<Mutex<Option<QuillCapture>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// The captured text used CRLF line endings, so the paste back must too (Word and Notepad).
static CRLF: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);

fn ctrl_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    }
}

/// The hotkey's own keys are still down when the handler fires; sending Ctrl+C then would reach the
/// app as Ctrl+Alt+C. Wait (briefly) for them to come up.
#[cfg(windows)]
fn wait_hotkey_released() {
    use winapi::um::winuser::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
    let down = |vk: i32| (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0;
    let deadline = Instant::now() + Duration::from_millis(700);
    while Instant::now() < deadline && [VK_CONTROL, VK_MENU, VK_SHIFT, 0x47].iter().any(|vk| down(*vk)) {
        std::thread::sleep(Duration::from_millis(15));
    }
}
#[cfg(not(windows))]
fn wait_hotkey_released() {
    std::thread::sleep(Duration::from_millis(250));
}

/// Copy with a sentinel on the clipboard so "nothing was selected" is distinguishable from "the
/// selection equals what was already on the clipboard".
fn copy_selection(cb: &mut arboard::Clipboard, timeout: Duration) -> Result<Option<String>, String> {
    let probe = format!("\u{200b}kudu-quill-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    cb.set_text(probe.clone()).map_err(|e| format!("Clipboard unavailable: {}", e))?;
    std::thread::sleep(Duration::from_millis(40));
    crate::input::press_combo(&format!("{}+c", ctrl_key()))?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(t) = cb.get_text() {
            if t != probe {
                return Ok(Some(t));
            }
        }
        if Instant::now() > deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn capture() -> Result<(String, &'static str), String> {
    wait_hotkey_released();
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("Clipboard unavailable: {}", e))?;
    let previous = cb.get_text().ok();
    let result = (|| {
        if let Some(t) = copy_selection(&mut cb, Duration::from_millis(450))?.filter(|t| !t.is_empty()) {
            return Ok((t, "selection"));
        }
        // Nothing selected: take the whole field, like pressing Ctrl+A first.
        crate::input::press_combo(&format!("{}+a", ctrl_key()))?;
        std::thread::sleep(Duration::from_millis(80));
        match copy_selection(&mut cb, Duration::from_millis(600))?.filter(|t| !t.trim().is_empty()) {
            Some(t) => Ok((t, "all")),
            None => Err("No text found. Click into the text you are writing, or select some, then press Ctrl+Alt+G.".to_string()),
        }
    })();
    match previous {
        Some(prev) => {
            let _ = cb.set_text(prev);
        }
        None => {
            let _ = cb.clear();
        }
    }
    result
}

fn place_near_cursor(app: &AppHandle, w: &tauri::WebviewWindow) {
    let (cx, cy) = crate::mouse::get_cursor_position();
    let monitor = app.monitor_from_point(cx as f64, cy as f64).ok().flatten().or_else(|| w.primary_monitor().ok().flatten());
    let Some(mon) = monitor else { return };
    let size = w.outer_size().map(|s| (s.width as i32, s.height as i32)).unwrap_or((460, 520));
    let (mx, my) = (mon.position().x, mon.position().y);
    let (mw, mh) = (mon.size().width as i32, mon.size().height as i32);
    let x = (cx + 18).clamp(mx + 8, (mx + mw - size.0 - 8).max(mx + 8));
    let y = (cy + 26).clamp(my + 8, (my + mh - size.1 - 8).max(my + 8));
    let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
}

fn show(app: &AppHandle, payload: QuillCapture) {
    if let Ok(mut p) = pending().lock() {
        *p = Some(payload.clone());
    }
    let Some(w) = app.get_webview_window(WINDOW) else { return };
    let _ = w.set_always_on_top(true);
    place_near_cursor(app, &w);
    let _ = w.show();
    let _ = w.set_focus();
    crate::meeting::protect_shown(&w);
    let _ = app.emit_to(WINDOW, "quill-capture", payload);
}

/// Hotkey entry point (Ctrl+Alt+G). Runs off the shortcut thread: capturing waits on the clipboard.
pub fn check_selection(app: &AppHandle) {
    if BUSY.swap(true, Ordering::Relaxed) {
        return;
    }
    // The app the user is writing in - where "Apply" pastes back to.
    crate::copilot::remember_foreground();
    let title = crate::mouse::get_foreground_context().window_title;
    let app = app.clone();
    let _ = std::thread::Builder::new().name("quill-capture".into()).spawn(move || {
        let payload = match capture() {
            Ok((text, scope)) => {
                CRLF.store(text.contains("\r\n"), Ordering::Relaxed);
                QuillCapture { text: text.replace("\r\n", "\n").replace('\r', "\n"), scope, app: title, error: None }
            }
            Err(e) => QuillCapture { text: String::new(), scope: "selection", app: title, error: Some(e) },
        };
        show(&app, payload);
        BUSY.store(false, Ordering::Relaxed);
    });
}

/// The capture a freshly loaded Quill window missed.
#[tauri::command]
pub fn quill_pending() -> Option<QuillCapture> {
    pending().lock().ok().and_then(|mut p| p.take())
}

#[tauri::command]
pub fn quill_hide(app: AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.hide();
    }
}

/// Paste the corrected text back into the app it came from (over the selection it was copied from).
#[tauri::command]
pub async fn quill_apply(app: AppHandle, text: String) -> Result<(), String> {
    if text.is_empty() {
        return Err("Nothing to apply.".to_string());
    }
    quill_hide(app);
    let text = if CRLF.load(Ordering::Relaxed) { text.replace("\r\n", "\n").replace('\n', "\r\n") } else { text };
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        crate::copilot::restore_target()?;
        crate::input::paste_via_clipboard(&text)
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?
}

/// The account side of Kudu Quill: the personal dictionary and prefs shared with the browser extension
/// (backend /api/writing/dictionary and /prefs). Offline this just fails and the frontend keeps its
/// local copy, then reconciles when the connection is back (frontend/src/lib/quill/sync.ts).
#[tauri::command]
pub async fn writing_api(
    state: tauri::State<'_, crate::commands::AppState>,
    method: String,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    state.backend.writing_request(&method, &path, body).await
}

/// Kudu Memory for the webview (profile card and facts shared with the extension and the website).
#[tauri::command]
pub async fn memory_api(
    state: tauri::State<'_, crate::commands::AppState>,
    method: String,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    state.backend.memory_request(&method, &path, body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_round_trip_does_not_double() {
        let t = "a\r\nb\nc".replace("\r\n", "\n").replace('\n', "\r\n");
        assert_eq!(t, "a\r\nb\r\nc");
    }
}
