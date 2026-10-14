// Mouse AI desktop agent - screen, windows, app content, open.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

pub(crate) async fn capture_screen_tool(screen: &mut Option<ScreenMapping>) -> ToolOutcome {
    let shot = tokio::task::spawn_blocking(capture::capture_agent_full).await;
    match shot {
        Ok(Ok(shot)) => {
            // Refresh the mapping so the NEXT click/scroll converts against
            // this screenshot, not a stale one.
            *screen = Some(ScreenMapping::from_shot(&shot));
            ToolOutcome {
                response: json!({ "result": "screenshot captured - use normalized 0-1000 x/y relative to this image for the next click/scroll" }),
                image_data_url: Some(shot.data_url),
                log: "captured screenshot".to_string(),
            }
        }
        Ok(Err(e)) => ToolOutcome::err(e),
        Err(e) => ToolOutcome::err(format!("capture task failed: {}", e)),
    }
}

pub(crate) async fn get_screen_context_tool() -> ToolOutcome {
    let outcome = tokio::task::spawn_blocking(|| {
        let (cx, cy) = mouse::get_cursor_position();
        let ctx = mouse::get_cursor_context();
        let foreground = mouse::get_foreground_context();
        let element = mouse::get_element_under_cursor(cx, cy);
        // Reported separately because they can differ (cursor resting over a
        // background window while another has focus) - acting via type_text/
        // press_key always goes to the FOREGROUND window, not whatever's under
        // the cursor, so both need to be visible to avoid acting on the wrong one.
        let mut response = json!({
            "activeApp": ctx.process_name,
            "activeWindow": ctx.window_title,
            "foregroundApp": foreground.process_name,
            "foregroundWindow": foreground.window_title,
            "cursor": { "x": cx, "y": cy },
            "elementName": element.name,
            "elementRole": element.role,
            "elementValue": element.value,
        });
        if let Some(ectx) = explorer::get_context_at(cx, cy) {
            response["folder"] = json!({
                "path": ectx.path,
                "entries": ectx.entries.iter().map(|e| {
                    format!("{}{}", e.name, if e.is_dir { "/" } else { "" })
                }).collect::<Vec<_>>(),
                "tech": ectx.tech_tags,
            });
        }
        response
    })
    .await;

    match outcome {
        Ok(response) => ToolOutcome { response, image_data_url: None, log: "read screen context".to_string() },
        Err(e) => ToolOutcome::err(format!("context task failed: {}", e)),
    }
}

/// One visible top-level window, as seen by `list_windows`/`focus_window`.
pub(crate) struct WindowInfo {
    pub(crate) title: String,
    pub(crate) process_name: String,
    pub(crate) foreground: bool,
}

#[cfg(windows)]
pub(crate) fn enumerate_top_windows() -> Vec<WindowInfo> {
    use winapi::shared::minwindef::{BOOL, LPARAM, TRUE};
    use winapi::shared::windef::HWND;
    use winapi::um::winuser::{
        EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible,
    };

    struct Collector {
        windows: Vec<WindowInfo>,
        foreground_hwnd: HWND,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let collector = &mut *(lparam as *mut Collector);
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return TRUE;
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let actual = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if actual == 0 {
            return TRUE;
        }
        let title = String::from_utf16_lossy(&buf[..actual as usize]).trim().to_string();
        if title.is_empty() {
            return TRUE;
        }
        // Skip the class of invisible/utility "windows" Windows itself creates in bulk
        // (tray icons, IME, tooltips) - they have a title but are noise for this list.
        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
        let class_name = if class_len > 0 {
            String::from_utf16_lossy(&class_buf[..class_len as usize])
        } else {
            String::new()
        };
        if matches!(class_name.as_str(), "Shell_TrayWnd" | "Progman" | "WorkerW" | "IME") {
            return TRUE;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process_name = if pid > 0 { mouse::get_process_name(pid) } else { String::new() };
        let foreground = hwnd == collector.foreground_hwnd;
        collector.windows.push(WindowInfo { title, process_name, foreground });
        TRUE
    }

    let mut collector = Collector { windows: Vec::new(), foreground_hwnd: unsafe { GetForegroundWindow() } };
    unsafe {
        EnumWindows(Some(enum_proc), &mut collector as *mut Collector as LPARAM);
    }
    collector.windows
}

#[cfg(not(windows))]
pub(crate) fn enumerate_top_windows() -> Vec<WindowInfo> {
    Vec::new()
}

#[cfg(windows)]
pub(crate) fn focus_window_by_title(needle: &str) -> Result<String, String> {
    use winapi::shared::minwindef::{BOOL, LPARAM, TRUE};
    use winapi::shared::windef::HWND;
    use winapi::um::processthreadsapi::GetCurrentThreadId;
    use winapi::um::winuser::{
        AttachThreadInput, BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    struct Search<'a> {
        needle: &'a str,
        found: Option<(HWND, String)>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam as *mut Search);
        if search.found.is_some() || IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        let len = GetWindowTextLengthW(hwnd);
        let title = if len > 0 {
            let mut buf = vec![0u16; (len + 1) as usize];
            let actual = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            String::from_utf16_lossy(&buf[..actual.max(0) as usize])
        } else {
            String::new()
        };
        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
        let class_name = if class_len > 0 {
            String::from_utf16_lossy(&class_buf[..class_len as usize])
        } else {
            String::new()
        };
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process_name = if pid > 0 { mouse::get_process_name(pid) } else { String::new() };

        let haystack = format!("{} {}", title, process_name).to_lowercase();
        if !title.trim().is_empty()
            && !matches!(class_name.as_str(), "Shell_TrayWnd" | "Progman" | "WorkerW" | "IME")
            && haystack.contains(search.needle)
        {
            search.found = Some((hwnd, if title.trim().is_empty() { process_name } else { title }));
        }
        TRUE
    }

    let mut search = Search { needle, found: None };
    unsafe {
        EnumWindows(Some(enum_proc), &mut search as *mut Search as LPARAM);
    }

    let (hwnd, label) = search
        .found
        .ok_or_else(|| format!("no open window matches \"{}\" - check list_windows for the exact title", needle))?;

    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        let foreground = GetForegroundWindow();
        let current_thread = GetCurrentThreadId();
        let fg_thread = if !foreground.is_null() {
            GetWindowThreadProcessId(foreground, std::ptr::null_mut())
        } else {
            0
        };
        if fg_thread != 0 && fg_thread != current_thread {
            AttachThreadInput(current_thread, fg_thread, 1);
        }
        BringWindowToTop(hwnd);
        let ok = SetForegroundWindow(hwnd);
        if fg_thread != 0 && fg_thread != current_thread {
            AttachThreadInput(current_thread, fg_thread, 0);
        }
        if ok == 0 {
            return Err(format!(
                "found \"{}\" but Windows blocked bringing it to the foreground - try clicking its taskbar icon instead",
                label
            ));
        }
    }
    remember_target_window(hwnd as isize, &label);
    Ok(label)
}

#[cfg(not(windows))]
pub(crate) fn focus_window_by_title(_needle: &str) -> Result<String, String> {
    Err("window focus is only supported on Windows".to_string())
}

pub(crate) async fn list_windows_tool() -> ToolOutcome {
    let mut windows = tokio::task::spawn_blocking(enumerate_top_windows).await.unwrap_or_default();
    // Surface the actually-focused window first - it's the one type_text/
    // press_key would act on right now, so it's the most decision-relevant entry.
    windows.sort_by_key(|w| !w.foreground);
    let count = windows.len();
    let entries: Vec<Value> = windows
        .iter()
        .take(40)
        .map(|w| json!({
            "title": w.title,
            "process": w.process_name,
            "category": classify_process(&w.process_name),
            "focused": w.foreground,
        }))
        .collect();
    ToolOutcome {
        response: json!({ "windows": entries, "count": count }),
        image_data_url: None,
        log: format!("{} open window(s)", count),
    }
}

pub(crate) async fn focus_window_tool(args: &Value) -> ToolOutcome {
    let needle = args
        .get("title_contains")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if needle.is_empty() {
        return ToolOutcome::err("title_contains is required");
    }
    let result = tokio::task::spawn_blocking(move || focus_window_by_title(&needle)).await;
    match result {
        Ok(Ok(label)) => ToolOutcome::text(format!("Focused \"{}\".", label)),
        Ok(Err(e)) => ToolOutcome::err(e),
        Err(e) => ToolOutcome::err(format!("focus task failed: {}", e)),
    }
}

/// Launch a URL / app / file via the shell (ShellExecute semantics), which
/// picks the right default handler - a browser for URLs, Explorer for folders,
/// the associated app for files.
pub(crate) fn open_target_tool(args: &Value, key: &str) -> ToolOutcome {
    let target = args.get(key).and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
    if target.is_empty() {
        return ToolOutcome::err("nothing to open");
    }
    // `cmd /C start "" <target>` uses the shell to resolve URLs (default
    // browser), file/folder paths (associated app / Explorer), and app names
    // on PATH. The empty "" is start's title argument, so a quoted target
    // isn't mistaken for the window title.
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", &target])
        .spawn();
    match result {
        Ok(_) => ToolOutcome::text(format!("Opened {}.", target)),
        Err(e) => ToolOutcome::err(format!("could not open {}: {}", target, e)),
    }
}

pub(crate) async fn read_app_content_tool() -> ToolOutcome {
    let outcome = tokio::task::spawn_blocking(|| {
        let fg = mouse::get_foreground_context();
        let nodes = mouse::foreground_window_rect()
            .map(|(x, y, w, h)| mouse::sample_window_content(x, y, w, h))
            .unwrap_or_default();
        (fg, nodes)
    })
    .await;

    let (fg, nodes) = match outcome {
        Ok(v) => v,
        Err(e) => return ToolOutcome::err(format!("content read task failed: {}", e)),
    };

    if nodes.is_empty() {
        return ToolOutcome::text(format!(
            "No readable accessible content found in \"{}\" ({}) - it may not expose accessible \
content, could be empty, or you may need to click into it first.",
            fg.window_title, fg.process_name
        ));
    }

    let mut text = format!("[APP CONTENT - {} ({})]\n", fg.window_title, fg.process_name);
    for n in &nodes {
        let line = [n.role.as_str(), n.name.as_str(), n.value.as_str()]
            .iter()
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(": ");
        if !line.trim().is_empty() {
            text.push_str("- ");
            text.push_str(&line);
            text.push('\n');
        }
    }
    let count = nodes.len();
    ToolOutcome {
        response: json!({ "result": truncate(&text, MAX_TOOL_OUTPUT) }),
        image_data_url: None,
        log: format!("read {} content node(s) from {}", count, fg.process_name),
    }
}

/// Coarse app category from its process name - good enough for "should I
/// reuse this window or open a new one", not a real app registry.
pub(crate) fn classify_process(process_name: &str) -> &'static str {
    let p = process_name.to_lowercase();
    let browsers = [
        "chrome.exe", "msedge.exe", "firefox.exe", "comet.exe", "brave.exe", "opera.exe",
        "opera_gx.exe", "vivaldi.exe", "iexplore.exe",
    ];
    let office = [
        "excel.exe", "winword.exe", "powerpnt.exe", "outlook.exe", "onenote.exe", "access.exe",
        "msaccess.exe",
    ];
    let editors = [
        "code.exe", "cursor.exe", "notepad.exe", "notepad++.exe", "sublime_text.exe", "vim.exe",
        "gvim.exe", "devenv.exe", "idea64.exe", "pycharm64.exe",
    ];
    let terminals = ["powershell.exe", "pwsh.exe", "cmd.exe", "windowsterminal.exe", "conhost.exe"];
    if browsers.contains(&p.as_str()) {
        "browser"
    } else if office.contains(&p.as_str()) {
        "office"
    } else if editors.contains(&p.as_str()) {
        "editor"
    } else if terminals.contains(&p.as_str()) {
        "terminal"
    } else {
        "other"
    }
}


// ── Productivity tools: precise edits, content search, clipboard, downloads ──
