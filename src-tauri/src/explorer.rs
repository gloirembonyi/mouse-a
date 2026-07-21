// Mouse AI Desktop — Windows Explorer folder-content context
//
// The accessible name/role under the cursor (e.g. "list item", a bare file
// name) can't answer "what's in this folder?" or "what language is this
// project?" — the AI has to hedge, which is exactly what users see. This
// module finds the real, currently-open Explorer window under the cursor
// via Shell Automation (the same COM API Windows Explorer itself is built
// on: CoCreateInstance(CLSID_ShellWindows) → match by HWND → read the
// folder's real path) and lists that folder straight from disk — cheaper
// and more complete than enumerating COM FolderItems one at a time.

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct ExplorerEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExplorerContext {
    pub path: String,
    pub entries: Vec<ExplorerEntry>,
    pub total_entries: usize,
    pub tech_tags: Vec<String>,
}

const MAX_ENTRIES: usize = 80;

#[cfg(windows)]
pub fn get_context_at(x: i32, y: i32) -> Option<ExplorerContext> {
    let path = win::find_explorer_path_at(x, y)?;
    let (entries, total_entries) = list_directory(&path);
    let tech_tags = detect_tech(&entries);
    Some(ExplorerContext { path, entries, total_entries, tech_tags })
}

#[cfg(not(windows))]
pub fn get_context_at(_x: i32, _y: i32) -> Option<ExplorerContext> {
    None
}

#[cfg(windows)]
mod win {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_LOCAL_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::{Folder2, IShellFolderViewDual, IShellWindows, IWebBrowserApp};
    use winapi::shared::windef::{HWND, POINT, RECT};
    use winapi::um::winuser::{GetAncestor, GetClassNameW, GetWindowRect, WindowFromPoint, GA_ROOT};

    // {9BA05972-F6A8-11CF-A442-00A0C90A8F39} — CLSID_ShellWindows, stable since
    // Windows 2000. No generated CoClass ships in the `windows` crate for it, so
    // the GUID is spelled out directly (the standard approach for this API).
    const CLSID_SHELL_WINDOWS: windows::core::GUID = windows::core::GUID::from_values(
        0x9BA0_5972,
        0xF6A8,
        0x11CF,
        [0xA4, 0x42, 0x00, 0xA0, 0xC9, 0x0A, 0x8F, 0x39],
    );

    fn root_window_at(x: i32, y: i32) -> Option<HWND> {
        unsafe {
            let point_hwnd = WindowFromPoint(POINT { x, y });
            if point_hwnd.is_null() {
                return None;
            }
            let root = GetAncestor(point_hwnd, GA_ROOT);
            Some(if root.is_null() { point_hwnd } else { root })
        }
    }

    fn class_name_of(hwnd: HWND) -> String {
        let mut buf = [0u16; 256];
        let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize])
    }

    /// `CabinetWClass` is a normal Explorer folder window; `ExploreWClass` is
    /// the legacy "Explore" view some tools still spawn. Anything else isn't
    /// Explorer, and callers should skip the COM round-trip entirely.
    fn is_explorer_class(class: &str) -> bool {
        class == "CabinetWClass" || class == "ExploreWClass"
    }

    /// Bounding rect of the top-level window under the given point, in
    /// global (virtual-desktop) screen coordinates — used by the capture
    /// module to grab the whole window instead of a small crop.
    pub fn window_rect_at(x: i32, y: i32) -> Option<(i32, i32, u32, u32)> {
        let hwnd = root_window_at(x, y)?;
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return None;
        }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((rect.left, rect.top, w as u32, h as u32))
    }

    pub fn find_explorer_path_at(x: i32, y: i32) -> Option<String> {
        let hwnd = root_window_at(x, y)?;
        if !is_explorer_class(&class_name_of(hwnd)) {
            return None;
        }
        let target = hwnd as isize;

        unsafe {
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let result = find_path_for_hwnd(target);
            if com.is_ok() {
                CoUninitialize();
            }
            result
        }
    }

    unsafe fn find_path_for_hwnd(target: isize) -> Option<String> {
        let shell_windows: IShellWindows =
            CoCreateInstance(&CLSID_SHELL_WINDOWS, None, CLSCTX_LOCAL_SERVER).ok()?;
        let count = shell_windows.Count().ok()?;

        for i in 0..count {
            let idx = VARIANT::from(i);
            let Ok(disp) = shell_windows.Item(&idx) else { continue };
            let Ok(browser) = disp.cast::<IWebBrowserApp>() else { continue };
            let Ok(browser_hwnd) = browser.HWND() else { continue };
            if browser_hwnd.0 != target {
                continue;
            }
            // Found the matching Explorer window — any failure past this
            // point means "no path available", not "keep looking".
            let doc = browser.Document().ok()?;
            let view = doc.cast::<IShellFolderViewDual>().ok()?;
            let folder = view.Folder().ok()?;
            let folder2 = folder.cast::<Folder2>().ok()?;
            let self_item = folder2.Self_().ok()?;
            let path = self_item.Path().ok()?;
            return Some(path.to_string());
        }
        None
    }
}

#[cfg(windows)]
pub fn window_rect_at(x: i32, y: i32) -> Option<(i32, i32, u32, u32)> {
    win::window_rect_at(x, y)
}

#[cfg(not(windows))]
pub fn window_rect_at(_x: i32, _y: i32) -> Option<(i32, i32, u32, u32)> {
    None
}

fn list_directory(path: &str) -> (Vec<ExplorerEntry>, usize) {
    let mut entries = Vec::new();
    let mut total = 0usize;
    if let Ok(read_dir) = std::fs::read_dir(path) {
        for entry in read_dir.flatten() {
            total += 1;
            if entries.len() >= MAX_ENTRIES {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push(ExplorerEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir,
            });
        }
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    (entries, total)
}

/// Cheap, deterministic project-type detection from what's actually on
/// disk — gives the model a factual answer instead of a guess from the
/// folder name alone.
fn detect_tech(entries: &[ExplorerEntry]) -> Vec<String> {
    let has = |n: &str| entries.iter().any(|e| e.name.eq_ignore_ascii_case(n));
    let mut tags = Vec::new();

    if has("package.json") {
        tags.push("Node.js/JavaScript project".to_string());
    }
    if has("Cargo.toml") {
        tags.push("Rust project".to_string());
    }
    if has("requirements.txt") || has("pyproject.toml") || has("Pipfile") {
        tags.push("Python project".to_string());
    }
    if has("go.mod") {
        tags.push("Go project".to_string());
    }
    if has("pom.xml") || has("build.gradle") || has("build.gradle.kts") {
        tags.push("Java/Kotlin project".to_string());
    }
    if has("composer.json") {
        tags.push("PHP project".to_string());
    }
    if has("Gemfile") {
        tags.push("Ruby project".to_string());
    }
    if has("tsconfig.json") {
        tags.push("TypeScript".to_string());
    }
    if has("Dockerfile") {
        tags.push("Docker".to_string());
    }
    if has(".git") {
        tags.push("Git repository".to_string());
    }

    if tags.is_empty() {
        // No recognized project marker — fall back to a plain extension
        // histogram so the model still gets something concrete to reason
        // from instead of "not possible to determine".
        use std::collections::HashMap;
        let mut counts: HashMap<String, u32> = HashMap::new();
        for e in entries {
            if e.is_dir {
                continue;
            }
            if let Some(ext) = Path::new(&e.name).extension() {
                *counts.entry(ext.to_string_lossy().to_lowercase()).or_default() += 1;
            }
        }
        let mut by_count: Vec<_> = counts.into_iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (ext, n) in by_count.into_iter().take(5) {
            tags.push(format!(".{} × {}", ext, n));
        }
    }

    tags
}

pub fn format_context(ctx: &ExplorerContext) -> String {
    let mut out = format!(
        "[FOLDER CONTENTS — real directory listing, not a guess]\nPath: {}\n",
        ctx.path
    );
    if !ctx.tech_tags.is_empty() {
        out.push_str(&format!("Detected: {}\n", ctx.tech_tags.join(", ")));
    }
    let shown = ctx.entries.len();
    if ctx.total_entries > shown {
        out.push_str(&format!("{} items total (showing first {}):\n", ctx.total_entries, shown));
    } else {
        out.push_str(&format!("{} items:\n", ctx.total_entries));
    }
    for e in &ctx.entries {
        out.push_str(&format!("- {}{}\n", e.name, if e.is_dir { "/" } else { "" }));
    }
    out
}
