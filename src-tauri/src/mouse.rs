// Mouse AI Desktop — Mouse Position & Context Detection
// Uses Windows API to track cursor position and detect what's under it.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CursorContext {
    pub x: i32,
    pub y: i32,
    pub window_title: String,
    pub process_name: String,
    pub class_name: String,
}

#[cfg(windows)]
pub fn get_cursor_position() -> (i32, i32) {
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::GetCursorPos;
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point);
    }
    (point.x, point.y)
}

#[cfg(not(windows))]
pub fn get_cursor_position() -> (i32, i32) {
    (0, 0)
}

#[cfg(windows)]
fn context_for_hwnd(hwnd: winapi::shared::windef::HWND, x: i32, y: i32) -> CursorContext {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::um::winuser::{GetClassNameW, GetWindowTextW, GetWindowThreadProcessId};

    if hwnd.is_null() {
        return CursorContext {
            x,
            y,
            window_title: String::new(),
            process_name: String::new(),
            class_name: String::new(),
        };
    }

    // Get window title
    let mut title_buf = [0u16; 512];
    let title_len = unsafe { GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32) };
    let window_title = if title_len > 0 {
        OsString::from_wide(&title_buf[..title_len as usize])
            .to_string_lossy()
            .to_string()
    } else {
        String::new()
    };

    // Get class name
    let mut class_buf = [0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32) };
    let class_name = if class_len > 0 {
        OsString::from_wide(&class_buf[..class_len as usize])
            .to_string_lossy()
            .to_string()
    } else {
        String::new()
    };

    // Get process name
    let mut process_id: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut process_id);
    }
    let process_name = if process_id > 0 {
        get_process_name(process_id)
    } else {
        String::new()
    };

    CursorContext {
        x,
        y,
        window_title,
        process_name,
        class_name,
    }
}

/// Same lookup `get_cursor_context()` does, but for an arbitrary global point
/// instead of always re-reading the live cursor position — lets a caller ask
/// "what app/window is AT this specific point" (e.g. where a click is about
/// to land) without first moving the mouse there.
#[cfg(windows)]
pub fn context_at_point(x: i32, y: i32) -> CursorContext {
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::WindowFromPoint;
    let hwnd = unsafe { WindowFromPoint(POINT { x, y }) };
    context_for_hwnd(hwnd, x, y)
}

#[cfg(not(windows))]
pub fn context_at_point(x: i32, y: i32) -> CursorContext {
    CursorContext { x, y, window_title: String::new(), process_name: String::new(), class_name: String::new() }
}

#[cfg(windows)]
pub fn get_cursor_context() -> CursorContext {
    let (x, y) = get_cursor_position();
    context_at_point(x, y)
}

/// The window that actually has keyboard focus right now — distinct from
/// `get_cursor_context()`, which reports whatever window is merely UNDER the
/// pointer. These commonly differ (e.g. the cursor rests over a background
/// window while a dialog elsewhere holds focus), and the agent needs to know
/// which one it's really about to type into/act on.
#[cfg(windows)]
pub fn get_foreground_context() -> CursorContext {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::um::winuser::{
        GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return CursorContext {
            x: 0,
            y: 0,
            window_title: String::new(),
            process_name: String::new(),
            class_name: String::new(),
        };
    }

    let mut title_buf = [0u16; 512];
    let title_len = unsafe { GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32) };
    let window_title = if title_len > 0 {
        OsString::from_wide(&title_buf[..title_len as usize]).to_string_lossy().to_string()
    } else {
        String::new()
    };

    let mut class_buf = [0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32) };
    let class_name = if class_len > 0 {
        OsString::from_wide(&class_buf[..class_len as usize]).to_string_lossy().to_string()
    } else {
        String::new()
    };

    let mut process_id: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut process_id);
    }
    let process_name = if process_id > 0 { get_process_name(process_id) } else { String::new() };

    CursorContext { x: 0, y: 0, window_title, process_name, class_name }
}

#[cfg(not(windows))]
pub fn get_foreground_context() -> CursorContext {
    CursorContext {
        x: 0,
        y: 0,
        window_title: String::new(),
        process_name: String::new(),
        class_name: String::new(),
    }
}

#[cfg(windows)]
pub(crate) fn get_process_name(pid: u32) -> String {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::OpenProcess;
    use winapi::um::psapi::GetModuleFileNameExW;
    use winapi::um::winnt::PROCESS_QUERY_INFORMATION;
    use winapi::um::winnt::PROCESS_VM_READ;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            return String::new();
        }

        let mut name_buf = [0u16; 512];
        let len = GetModuleFileNameExW(
            handle,
            std::ptr::null_mut(),
            name_buf.as_mut_ptr(),
            name_buf.len() as u32,
        );
        CloseHandle(handle);

        if len > 0 {
            let full_path = OsString::from_wide(&name_buf[..len as usize])
                .to_string_lossy()
                .to_string();
            // Extract just the filename
            full_path
                .rsplit('\\')
                .next()
                .unwrap_or(&full_path)
                .to_string()
        } else {
            String::new()
        }
    }
}

#[cfg(not(windows))]
pub fn get_cursor_context() -> CursorContext {
    CursorContext {
        x: 0,
        y: 0,
        window_title: String::new(),
        process_name: String::new(),
        class_name: String::new(),
    }
}

// ── Element under cursor (MSAA / IAccessible) ──────────────────────
// Reads the accessible name/value/role of whatever UI element sits under
// the given screen point, so the AI gets real text context without OCR.

#[derive(Debug, Clone, Default, Serialize)]
pub struct ElementInfo {
    pub name: String,
    pub value: String,
    pub role: String,
    /// Approximate immediate container (e.g. a list/table/pane name) —
    /// gives the model surrounding context beyond the single hovered
    /// control, at the cost of occasionally being one level too high.
    pub container: String,
}

// 400 chars was too tight for anything past a short label (a paragraph of
// chat/email/document text would get cut off right when it mattered).
const MAX_ELEMENT_TEXT: usize = 900;

fn truncate_text(mut s: String) -> String {
    if s.chars().count() > MAX_ELEMENT_TEXT {
        s = s.chars().take(MAX_ELEMENT_TEXT).collect::<String>() + "…";
    }
    s
}

/// Core lookup shared by `get_element_under_cursor` and `sample_window_content`
/// — assumes COM is already initialized on this thread by the caller, so a
/// multi-point sample only pays the CoInitializeEx/CoUninitialize cost once.
#[cfg(windows)]
fn element_at_point_raw(x: i32, y: i32) -> ElementInfo {
    use windows::core::Interface;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::Variant::{VARIANT, VT_I4};
    use windows::Win32::UI::Accessibility::{AccessibleObjectFromPoint, GetRoleTextW, IAccessible};

    let mut info = ElementInfo::default();
    unsafe {
        let point = POINT { x, y };
        let mut acc: Option<IAccessible> = None;
        let mut var_child = VARIANT::default();

        if AccessibleObjectFromPoint(point, &mut acc, &mut var_child).is_ok() {
            if let Some(acc) = acc {
                if let Ok(name) = acc.get_accName(&var_child) {
                    info.name = truncate_text(name.to_string());
                }
                if let Ok(value) = acc.get_accValue(&var_child) {
                    info.value = truncate_text(value.to_string());
                }
                if let Ok(role_var) = acc.get_accRole(&var_child) {
                    let vt = role_var.Anonymous.Anonymous.vt;
                    if vt == VT_I4 {
                        let role_id = role_var.Anonymous.Anonymous.Anonymous.lVal;
                        let mut buf = [0u16; 128];
                        let n = GetRoleTextW(role_id as u32, Some(&mut buf));
                        if n > 0 {
                            info.role = String::from_utf16_lossy(&buf[..n as usize]);
                        }
                    }
                }
                if let Ok(parent_dispatch) = acc.accParent() {
                    if let Ok(parent_acc) = parent_dispatch.cast::<IAccessible>() {
                        let self_child = VARIANT::from(0i32);
                        if let Ok(parent_name) = parent_acc.get_accName(&self_child) {
                            info.container = truncate_text(parent_name.to_string());
                        }
                    }
                }
            }
        }
    }
    info
}

#[cfg(windows)]
pub fn get_element_under_cursor(x: i32, y: i32) -> ElementInfo {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let info = element_at_point_raw(x, y);
        if com.is_ok() {
            CoUninitialize();
        }
        info
    }
}

#[cfg(not(windows))]
pub fn get_element_under_cursor(_x: i32, _y: i32) -> ElementInfo {
    ElementInfo::default()
}

// ── Generic "what's in this window" content sampling ─────────────
// File Explorer gets a real directory listing (see explorer.rs); everything
// else — Excel, Notepad, a browser, any other app — only exposes its content
// through the accessibility tree. Rather than write a bespoke integration per
// app, this walks a grid of points across the window's bounds and reads
// whatever accessible element sits at each one via the same MSAA call
// `get_element_under_cursor` already uses, deduplicating repeats. Cruder than
// a proper accChild tree walk, but it's built entirely on the one accessibility
// entry point already proven to work here, and it generalizes to any app
// without per-app code.
#[derive(Debug, Clone, Serialize)]
pub struct ContentNode {
    pub name: String,
    pub value: String,
    pub role: String,
}

const CONTENT_GRID_COLS: i32 = 12;
const CONTENT_GRID_ROWS: i32 = 10;
const MAX_CONTENT_NODES: usize = 80;

#[cfg(windows)]
pub fn sample_window_content(x: i32, y: i32, w: u32, h: u32) -> Vec<ContentNode> {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    let mut out = Vec::new();
    if w == 0 || h == 0 {
        return out;
    }
    let mut seen = std::collections::HashSet::new();

    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        'grid: for row in 0..CONTENT_GRID_ROWS {
            for col in 0..CONTENT_GRID_COLS {
                if out.len() >= MAX_CONTENT_NODES {
                    break 'grid;
                }
                let px = x + (col as i64 * w as i64 / CONTENT_GRID_COLS as i64) as i32
                    + (w as i32 / (CONTENT_GRID_COLS * 2)).max(1);
                let py = y + (row as i64 * h as i64 / CONTENT_GRID_ROWS as i64) as i32
                    + (h as i32 / (CONTENT_GRID_ROWS * 2)).max(1);
                let el = element_at_point_raw(px, py);
                if el.name.trim().is_empty() && el.value.trim().is_empty() {
                    continue;
                }
                let key = format!("{}\u{1}{}\u{1}{}", el.role, el.name, el.value);
                if seen.insert(key) {
                    out.push(ContentNode { name: el.name, value: el.value, role: el.role });
                }
            }
        }
        if com.is_ok() {
            CoUninitialize();
        }
    }
    out
}

#[cfg(not(windows))]
pub fn sample_window_content(_x: i32, _y: i32, _w: u32, _h: u32) -> Vec<ContentNode> {
    Vec::new()
}

/// True when the top-level window at the given global point belongs to THIS
/// process (Mouse AI's own overlay/main window). The always-on-top overlay
/// floats above the app the agent is trying to drive, so without this check
/// the agent can click its own panel — or type into its own input box — and
/// believe it acted on the target app.
#[cfg(windows)]
pub fn is_own_process_at(x: i32, y: i32) -> bool {
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::{GetWindowThreadProcessId, WindowFromPoint};
    unsafe {
        let hwnd = WindowFromPoint(POINT { x, y });
        if hwnd.is_null() {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid == std::process::id()
    }
}

#[cfg(not(windows))]
pub fn is_own_process_at(_x: i32, _y: i32) -> bool {
    false
}

/// True when keyboard focus is currently on one of THIS process's own windows
/// — the state in which type_text/press_key would land in Mouse AI's own UI
/// instead of the app the user meant.
#[cfg(windows)]
pub fn is_own_process_foreground() -> bool {
    use winapi::um::winuser::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid == std::process::id()
    }
}

#[cfg(not(windows))]
pub fn is_own_process_foreground() -> bool {
    false
}

/// Bounding rect of whichever window currently has keyboard focus — the
/// counterpart to `explorer::window_rect_at` (which resolves a window under an
/// arbitrary point) for tools that care about "the app the user is actually
/// typing into right now" instead of whatever's under the cursor.
#[cfg(windows)]
pub fn foreground_window_rect() -> Option<(i32, i32, u32, u32)> {
    use winapi::shared::windef::RECT;
    use winapi::um::winuser::{GetForegroundWindow, GetWindowRect};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((rect.left, rect.top, w as u32, h as u32))
    }
}

#[cfg(not(windows))]
pub fn foreground_window_rect() -> Option<(i32, i32, u32, u32)> {
    None
}
