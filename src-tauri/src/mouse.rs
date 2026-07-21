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
pub fn get_cursor_context() -> CursorContext {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::{
        GetClassNameW, GetCursorPos, GetWindowTextW, GetWindowThreadProcessId, WindowFromPoint,
    };

    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point);
    }

    let hwnd = unsafe { WindowFromPoint(point) };
    if hwnd.is_null() {
        return CursorContext {
            x: point.x,
            y: point.y,
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
        x: point.x,
        y: point.y,
        window_title,
        process_name,
        class_name,
    }
}

#[cfg(windows)]
fn get_process_name(pid: u32) -> String {
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

#[cfg(windows)]
pub fn get_element_under_cursor(x: i32, y: i32) -> ElementInfo {
    use windows::core::Interface;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Variant::{VARIANT, VT_I4};
    use windows::Win32::UI::Accessibility::{
        AccessibleObjectFromPoint, GetRoleTextW, IAccessible,
    };

    let mut info = ElementInfo::default();

    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

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

        if com.is_ok() {
            CoUninitialize();
        }
    }

    info
}

#[cfg(not(windows))]
pub fn get_element_under_cursor(_x: i32, _y: i32) -> ElementInfo {
    ElementInfo::default()
}
