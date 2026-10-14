// Mouse AI desktop agent - mouse and keyboard tools.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// ── Last target window ───────────────────────────────────────────
// The window the agent last clicked or explicitly focused, kept so
// type_text/press_key can hand keyboard focus BACK to it when focus has
// drifted onto Mouse AI's own UI (typically: the user touched the overlay
// mid-run to watch the log). Without this the step fails with a corrective
// error and burns a whole model round-trip on something the app can fix
// itself in milliseconds.
pub(crate) static LAST_TARGET_WINDOW: Mutex<Option<(isize, String)>> = Mutex::new(None);

pub(crate) fn remember_target_window(hwnd: isize, label: &str) {
    if hwnd != 0 {
        *LAST_TARGET_WINDOW.lock().unwrap() = Some((hwnd, label.to_string()));
    }
}

pub(crate) fn last_target_window() -> Option<(isize, String)> {
    LAST_TARGET_WINDOW.lock().unwrap().clone()
}

/// Reads normalized x/y args and converts them to real screen pixels via the
/// most recent screenshot's mapping. Returns an error (instead of guessing)
/// if there's no mapping yet - the model needs to capture_screen first.
pub(crate) fn resolve_point(args: &Value, screen: &Option<ScreenMapping>) -> Result<(i32, i32), String> {
    let nx = args.get("x").and_then(|v| v.as_f64());
    let ny = args.get("y").and_then(|v| v.as_f64());
    let (nx, ny) = match (nx, ny) {
        (Some(x), Some(y)) => (x, y),
        _ => return Err("requires numeric x and y".to_string()),
    };
    let mapping = screen
        .as_ref()
        .ok_or("no screenshot has been captured yet - call capture_screen first so coordinates have something to map to")?;
    Ok(mapping.to_real(nx, ny))
}

/// The agent's own always-on-top overlay floats above whatever app it's
/// driving - a click aimed "at the page" can actually land on Mouse AI's own
/// panel, and typing right after the user pressed Enter often goes into Mouse
/// AI's own input box. Both look like success to the model while doing nothing
/// to the target app (the classic "I created the note!" when nothing saved).
/// These return a corrective error instead of letting that happen.
pub(crate) async fn reject_own_window_at(x: i32, y: i32) -> Option<ToolOutcome> {
    let own = tokio::task::spawn_blocking(move || mouse::is_own_process_at(x, y))
        .await
        .unwrap_or(false);
    if own {
        Some(ToolOutcome::err(
            "those coordinates land on Mouse AI's OWN floating panel, which is covering that part \
of the target app - the action was NOT performed. Aim at a point on the target app itself; if the \
element you need is hidden behind the panel, pick the same element elsewhere on screen, scroll it \
into a visible spot, or focus_window the target app and work from a fresh screenshot.",
        ))
    } else {
        None
    }
}

/// Keyboard-focus gate for type_text/press_key. When focus is on Mouse AI's
/// own window (the user touched the overlay mid-run), first try to hand focus
/// back to the last window the agent clicked or focused - only if that isn't
/// possible does the step fail with the corrective error. Returns an optional
/// note describing the auto-recovery for the tool's success message.
pub(crate) async fn ensure_target_focused() -> Result<Option<String>, Box<ToolOutcome>> {
    let own = tokio::task::spawn_blocking(mouse::is_own_process_foreground)
        .await
        .unwrap_or(false);
    if !own {
        return Ok(None);
    }
    if let Some((hwnd, label)) = last_target_window() {
        let refocused = tokio::task::spawn_blocking(move || mouse::focus_hwnd(hwnd)).await;
        if matches!(refocused, Ok(Ok(()))) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let still_own = tokio::task::spawn_blocking(mouse::is_own_process_foreground)
                .await
                .unwrap_or(true);
            if !still_own {
                return Ok(Some(format!(
                    "focus had drifted to Mouse AI's own window - auto-refocused \"{}\" first",
                    label
                )));
            }
        }
    }
    Err(Box::new(ToolOutcome::err(
        "keyboard focus is currently on Mouse AI's OWN window, so the keystrokes would go into \
the assistant's UI instead of the target app - nothing was typed. First click on the target app or \
focus_window it, verify focus from the attached screenshot, then type.",
    )))
}

/// Resolves whatever process/window sits at a real screen point, off the
/// blocking thread pool (the underlying MSAA/Win32 calls aren't async).
pub(crate) async fn context_at_point_async(x: i32, y: i32) -> mouse::CursorContext {
    tokio::task::spawn_blocking(move || mouse::context_at_point(x, y))
        .await
        .unwrap_or(mouse::CursorContext {
            x,
            y,
            window_title: String::new(),
            process_name: String::new(),
            class_name: String::new(),
        })
}

pub(crate) async fn click_tool(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    args: &Value,
    screen: &Option<ScreenMapping>,
) -> ToolOutcome {
    let (x, y) = match resolve_point(args, screen) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::err(e),
    };
    if let Some(rejected) = reject_own_window_at(x, y).await {
        return rejected;
    }
    let target = context_at_point_async(x, y).await;
    if let Err(e) = guard_target_app(app, source, trusted, &target.process_name, &target.window_title).await {
        return ToolOutcome::err(e);
    }
    let hwnd = tokio::task::spawn_blocking(move || mouse::target_hwnd_at_point(x, y)).await.unwrap_or(0);
    remember_target_window(hwnd, &target.window_title);
    let right = args.get("button").and_then(|b| b.as_str()).map(|b| b.eq_ignore_ascii_case("right")).unwrap_or(false);
    input::click(x, y, right);
    ToolOutcome::text(format!("Clicked {} at ({}, {}).", if right { "right" } else { "left" }, x, y))
}

pub(crate) async fn double_click_tool(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    args: &Value,
    screen: &Option<ScreenMapping>,
) -> ToolOutcome {
    let (x, y) = match resolve_point(args, screen) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::err(e),
    };
    if let Some(rejected) = reject_own_window_at(x, y).await {
        return rejected;
    }
    let target = context_at_point_async(x, y).await;
    if let Err(e) = guard_target_app(app, source, trusted, &target.process_name, &target.window_title).await {
        return ToolOutcome::err(e);
    }
    let hwnd = tokio::task::spawn_blocking(move || mouse::target_hwnd_at_point(x, y)).await.unwrap_or(0);
    remember_target_window(hwnd, &target.window_title);
    input::double_click(x, y);
    ToolOutcome::text(format!("Double-clicked ({}, {}).", x, y))
}

pub(crate) async fn scroll_tool(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    args: &Value,
    screen: &Option<ScreenMapping>,
) -> ToolOutcome {
    let amount = match args.get("amount").and_then(|v| v.as_i64()) {
        Some(a) => a,
        None => return ToolOutcome::err("scroll requires an integer amount"),
    };
    let (x, y) = match resolve_point(args, screen) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::err(e),
    };
    if let Some(rejected) = reject_own_window_at(x, y).await {
        return rejected;
    }
    let target = context_at_point_async(x, y).await;
    if let Err(e) = guard_target_app(app, source, trusted, &target.process_name, &target.window_title).await {
        return ToolOutcome::err(e);
    }
    input::scroll(x, y, amount as i32);
    ToolOutcome::text(format!("Scrolled {} at ({}, {}).", amount, x, y))
}

pub(crate) async fn wait_tool(args: &Value) -> ToolOutcome {
    let seconds = args.get("seconds").and_then(|s| s.as_f64()).unwrap_or(1.0).clamp(0.0, 15.0);
    tokio::time::sleep(Duration::from_millis((seconds * 1000.0) as u64)).await;
    ToolOutcome::text(format!("Waited {:.1}s.", seconds))
}

/// type_text/press_key act on whatever has keyboard FOCUS, not whatever's
/// under the cursor - the foreground window is the correct thing to gate on.
pub(crate) async fn foreground_context_async() -> mouse::CursorContext {
    tokio::task::spawn_blocking(mouse::get_foreground_context)
        .await
        .unwrap_or(mouse::CursorContext {
            x: 0,
            y: 0,
            window_title: String::new(),
            process_name: String::new(),
            class_name: String::new(),
        })
}

pub(crate) async fn type_text_tool(app: &AppHandle, source: &str, trusted: &mut HashSet<String>, args: &Value) -> ToolOutcome {
    let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("");
    if text.is_empty() {
        return ToolOutcome::err("no text provided");
    }
    let focus_note = match ensure_target_focused().await {
        Ok(note) => note,
        Err(rejected) => return *rejected,
    };
    let fg = foreground_context_async().await;
    if let Err(e) = guard_target_app(app, source, trusted, &fg.process_name, &fg.window_title).await {
        return ToolOutcome::err(e);
    }
    input::type_text(text);
    match focus_note {
        Some(note) => ToolOutcome::text(format!("Typed {} characters ({}).", text.chars().count(), note)),
        None => ToolOutcome::text(format!("Typed {} characters.", text.chars().count())),
    }
}

pub(crate) async fn press_key_tool(app: &AppHandle, source: &str, trusted: &mut HashSet<String>, args: &Value) -> ToolOutcome {
    let keys = args.get("keys").and_then(|k| k.as_str()).unwrap_or("");
    if keys.is_empty() {
        return ToolOutcome::err("no keys provided");
    }
    let focus_note = match ensure_target_focused().await {
        Ok(note) => note,
        Err(rejected) => return *rejected,
    };
    let fg = foreground_context_async().await;
    if let Err(e) = guard_target_app(app, source, trusted, &fg.process_name, &fg.window_title).await {
        return ToolOutcome::err(e);
    }
    match input::press_combo(keys) {
        Ok(()) => match focus_note {
            Some(note) => ToolOutcome::text(format!("Pressed {} ({}).", keys, note)),
            None => ToolOutcome::text(format!("Pressed {}.", keys)),
        },
        Err(e) => ToolOutcome::err(e),
    }
}
