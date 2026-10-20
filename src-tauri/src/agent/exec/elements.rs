// Kudu Desktop agent - element-level tools (find_elements / click_element / set_value).
//
// The reliable way to drive a desktop app is by the names of its controls, not by
// pixels on a screenshot. These tools walk the foreground window's UI Automation tree
// (crate::uia), list what a person would see ("Save" button, "Search" edit, "Settings"
// menu item) with ids and screen rectangles, and act on an element by re-finding it at
// action time - so a dialog that opened a few pixels lower still gets the right click.

#[allow(unused_imports)]
use crate::agent::*;
use crate::uia::{self, Act, Query, UiElement};

/// The last listing, so click_element/set_value can take an id instead of a name.
static LAST_ELEMENTS: Mutex<Vec<UiElement>> = Mutex::new(Vec::new());

fn remember(list: &[UiElement]) {
    *LAST_ELEMENTS.lock().unwrap() = list.to_vec();
}

/// (name, role, centre of the remembered rect) for an id or name argument.
fn resolve_target(args: &Value) -> Result<(String, String, Option<(i32, i32)>), String> {
    let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let role = args.get("role").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !id.is_empty() {
        let list = LAST_ELEMENTS.lock().unwrap();
        if let Some(e) = list.iter().find(|e| e.id == id) {
            let key = if e.name.is_empty() { e.automation_id.clone() } else { e.name.clone() };
            return Ok((key, e.role.clone(), Some((e.x + e.w / 2, e.y + e.h / 2))));
        }
        if name.is_empty() {
            return Err(format!("no element \"{}\" in the last find_elements result - call find_elements again (the window changed) or pass name", id));
        }
    }
    if name.is_empty() {
        return Err("pass id (from find_elements) or name".to_string());
    }
    Ok((name, role, None))
}

pub(crate) async fn find_elements_tool(args: &Value) -> ToolOutcome {
    let q = Query {
        text: args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        role: args.get("role").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        limit: args.get("limit").and_then(|v| v.as_u64()).unwrap_or(40).clamp(1, 120) as usize,
        include_text: args.get("include_text").and_then(|v| v.as_bool()).unwrap_or(false),
    };
    let fg = foreground_context_async().await;
    if fg.process_name.to_lowercase().contains("mouse-ai") || fg.window_title.to_lowercase().contains("kudu") {
        return ToolOutcome::err("the foreground window is Kudu Desktop itself - focus_window the app you want to inspect first");
    }
    let query_text = q.text.clone();
    match tokio::task::spawn_blocking(move || uia::find_elements(&q)).await {
        Ok(Ok(list)) => {
            remember(&list);
            let n = list.len();
            let note = if n == 0 {
                if query_text.is_empty() {
                    "No controls exposed. The app may not support accessibility (games, some Electron/canvas apps) - use capture_screen + click, or read_app_content."
                } else {
                    "Nothing matched. Try a shorter word, drop the role, or find_elements with no query to see what the window exposes."
                }
            } else {
                "Act with click_element (id or name) or set_value (text fields). Ids stay valid until the window changes; x/y/w/h are screen pixels."
            };
            ToolOutcome {
                response: json!({
                    "window": fg.window_title,
                    "process": fg.process_name,
                    "count": n,
                    "elements": list,
                    "note": note,
                }),
                image_data_url: None,
                log: format!("{} element(s) in {}", n, fg.process_name),
            }
        }
        Ok(Err(e)) => ToolOutcome::err(format!("find_elements: {}", e)),
        Err(e) => ToolOutcome::err(format!("find_elements task failed: {}", e)),
    }
}

async fn guard_point(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    x: i32,
    y: i32,
) -> Result<(), ToolOutcome> {
    if let Some(rejected) = reject_own_window_at(x, y).await {
        return Err(rejected);
    }
    let target = context_at_point_async(x, y).await;
    if let Err(e) = guard_target_app(app, source, trusted, &target.process_name, &target.window_title).await {
        return Err(ToolOutcome::err(e));
    }
    let hwnd = tokio::task::spawn_blocking(move || mouse::target_hwnd_at_point(x, y)).await.unwrap_or(0);
    remember_target_window(hwnd, &target.window_title);
    Ok(())
}

pub(crate) async fn click_element_tool(app: &AppHandle, source: &str, trusted: &mut HashSet<String>, args: &Value) -> ToolOutcome {
    let (name, role, near) = match resolve_target(args) {
        Ok(t) => t,
        Err(e) => return ToolOutcome::err(e),
    };
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("click").to_lowercase();
    let (n2, r2) = (name.clone(), role.clone());
    let found = match tokio::task::spawn_blocking(move || uia::locate(&n2, &r2, near)).await {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => return ToolOutcome::err(e),
        Err(e) => return ToolOutcome::err(format!("locate task failed: {}", e)),
    };
    let (cx, cy) = (found.x + found.w / 2, found.y + found.h / 2);
    let visible = !found.offscreen && found.w > 0 && found.h > 0;
    if action == "click" && visible {
        if let Err(rejected) = guard_point(app, source, trusted, cx, cy).await {
            return rejected;
        }
        let _ = tokio::task::spawn_blocking(move || input::click(cx, cy, false)).await;
        return ToolOutcome::text(format!("Clicked {} \"{}\" at ({}, {}).", found.role, found.name, cx, cy));
    }
    // Not visible, or an accessibility action was asked for: go through the pattern.
    let act = match action.as_str() {
        "click" | "invoke" | "press" | "activate" => Act::Invoke,
        "toggle" | "check" | "uncheck" => Act::Toggle,
        "select" | "choose" => Act::Select,
        "expand" | "open" => Act::Expand,
        "focus" => Act::Focus,
        other => return ToolOutcome::err(format!("unknown action \"{}\" (click | invoke | toggle | select | expand | focus)", other)),
    };
    if visible {
        if let Err(rejected) = guard_point(app, source, trusted, cx, cy).await {
            return rejected;
        }
    }
    let (n3, r3) = (name.clone(), role.clone());
    match tokio::task::spawn_blocking(move || uia::act_on(&n3, &r3, near, act)).await {
        Ok(Ok((d, receipt))) => ToolOutcome::text(format!("{} {} \"{}\".", capitalize(&receipt), d.role, d.name)),
        Ok(Err(e)) => ToolOutcome::err(format!(
            "{} - the element is {}; try action=\"click\" after scrolling it into view, or click its coordinates from a screenshot",
            e,
            if visible { "visible" } else { "off-screen" }
        )),
        Err(e) => ToolOutcome::err(format!("element action task failed: {}", e)),
    }
}

pub(crate) async fn set_value_tool(app: &AppHandle, source: &str, trusted: &mut HashSet<String>, args: &Value) -> ToolOutcome {
    let mut text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    // Same rule as type_text: a field gets plain text, not Markdown (unless format=raw,
    // code, or a code editor / terminal / Markdown site has focus).
    if args.get("format").and_then(|f| f.as_str()) != Some("raw") && !looks_like_code(&text) {
        let fg = foreground_context_async().await;
        if let Some(plain) = crate::text_format::for_app(&text, classify_process(&fg.process_name), &fg.window_title) {
            text = plain;
        }
    }
    let (name, role, near) = match resolve_target(args) {
        Ok(t) => t,
        Err(e) => return ToolOutcome::err(e),
    };
    let (n2, r2) = (name.clone(), role.clone());
    let found = match tokio::task::spawn_blocking(move || uia::locate(&n2, &r2, near)).await {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => return ToolOutcome::err(e),
        Err(e) => return ToolOutcome::err(format!("locate task failed: {}", e)),
    };
    let (cx, cy) = (found.x + found.w / 2, found.y + found.h / 2);
    let visible = !found.offscreen && found.w > 0 && found.h > 0;
    if visible {
        if let Err(rejected) = guard_point(app, source, trusted, cx, cy).await {
            return rejected;
        }
    }
    let (n3, r3, t3) = (name.clone(), role.clone(), text.clone());
    match tokio::task::spawn_blocking(move || uia::act_on(&n3, &r3, near, Act::SetValue(t3))).await {
        Ok(Ok((d, receipt))) => ToolOutcome::text(format!("{} \"{}\": {}.", d.role, d.name, receipt)),
        Ok(Err(e)) if visible => {
            // No Value pattern (many web and custom fields): do it like a person.
            let t4 = text.clone();
            let typed = tokio::task::spawn_blocking(move || -> Result<&'static str, String> {
                input::click(cx, cy, false);
                std::thread::sleep(Duration::from_millis(120));
                input::press_combo("ctrl+a")?;
                std::thread::sleep(Duration::from_millis(40));
                match input::paste_via_clipboard(&t4) {
                    Ok(()) => Ok("paste"),
                    Err(_) => {
                        input::type_text_keys(&t4);
                        Ok("keystrokes")
                    }
                }
            })
            .await;
            match typed {
                Ok(Ok(how)) => ToolOutcome::text(format!(
                    "{} \"{}\" has no value pattern ({}); clicked it, selected all and entered the text via {} - verify from the screenshot.",
                    found.role, found.name, e, how
                )),
                Ok(Err(e2)) => ToolOutcome::err(format!("{}; keyboard fallback failed: {}", e, e2)),
                Err(e2) => ToolOutcome::err(format!("set_value task failed: {}", e2)),
            }
        }
        Ok(Err(e)) => ToolOutcome::err(format!("{} (element is off-screen; scroll it into view and retry)", e)),
        Err(e) => ToolOutcome::err(format!("set_value task failed: {}", e)),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
