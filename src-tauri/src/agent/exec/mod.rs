// Kudu Desktop agent - tool execution: dispatch plus the small tools that fit nowhere else.

#[allow(unused_imports)]
use crate::agent::*;

mod computer;
mod elements;
mod files;
mod shell;
mod web;
mod windows;

pub(crate) use computer::*;
pub(crate) use elements::*;
pub(crate) use files::*;
pub(crate) use shell::*;
pub(crate) use web::*;
pub(crate) use windows::*;

pub(crate) async fn execute_tool(
    app: &AppHandle,
    backend: &BackendClient,
    source: &str,
    name: &str,
    args: &Value,
    screen: &mut Option<ScreenMapping>,
    trusted: &mut HashSet<String>,
) -> ToolOutcome {
    match name {
        "run_command" => run_command_tool(source, args).await,
        "read_file" => read_file_tool(args),
        "use_skill" => match crate::skills::use_skill(args) {
            Ok(text) => ToolOutcome::text(text),
            Err(e) => ToolOutcome::err(e),
        },
        "create_document" => create_document_tool(args),
        "write_file" => write_file_tool(args),
        "edit_file" => edit_file_tool(args),
        "search_in_files" => search_in_files_tool(args).await,
        "get_clipboard" => clipboard_tool(None).await,
        "set_clipboard" => clipboard_tool(Some(args.get("text").and_then(|t| t.as_str()).unwrap_or(""))).await,
        "download_file" => download_file_tool(args).await,
        "notify" => notify_tool(app, args),
        "delete_file" => delete_file_tool(args),
        "list_directory" => list_directory_tool(args),
        "find_files" => find_files_tool(args).await,
        "web_search" => web_search_tool(backend, args).await,
        "web_fetch" => web_fetch_tool(backend, args).await,
        "watch_video" => watch_video_tool(app, source, backend, args).await,
        "capture_screen" => capture_screen_tool(screen).await,
        "get_screen_context" => get_screen_context_tool().await,
        "list_windows" => list_windows_tool().await,
        "focus_window" => focus_window_tool(args).await,
        "read_app_content" => read_app_content_tool().await,
        "find_elements" => find_elements_tool(args).await,
        "click_element" => click_element_tool(app, source, trusted, args).await,
        "set_value" => set_value_tool(app, source, trusted, args).await,
        "click" => click_tool(app, source, trusted, args, screen).await,
        "double_click" => double_click_tool(app, source, trusted, args, screen).await,
        "scroll" => scroll_tool(app, source, trusted, args, screen).await,
        "type_text" => type_text_tool(app, source, trusted, args).await,
        "press_key" => press_key_tool(app, source, trusted, args).await,
        "open_url" => open_target_tool(app, args, "url"),
        "open_app" => open_target_tool(app, args, "target"),
        "wait" => wait_tool(args).await,
        "request_confirmation" => request_confirmation_tool(app, source, args).await,
        "delegate_to_browser" => delegate_to_browser_tool(source, args).await,
        "check_browser_task" => check_browser_task_tool(source, args).await,
        other => ToolOutcome::err(format!("Unknown tool: {}", other)),
    }
}

// ── Observe-act-verify: automatic post-action screenshots ────────
// The root cause of the agent confidently claiming "I created the note!" when
// nothing was saved: UI actions used to return only a text receipt ("Typed 22
// characters."), so unless the model REMEMBERED to call capture_screen it was
// flying blind and assumed success. Production browser/computer-use agents
// (Manus, Claude computer use, OpenAI CUA) all close this loop by observing
// after every act - so now every action that changes what's on screen gets a
// fresh screenshot attached to its result automatically, after a short settle
// delay. The screenshot also refreshes the coordinate mapping, keeping the
// "coordinates are relative to the most recent screenshot" contract intact.

/// None = read the clipboard; Some(text) = write it. Goes through PowerShell's
/// Get-/Set-Clipboard (no extra native dependency), with the window hidden.
pub(crate) async fn clipboard_tool(write: Option<&str>) -> ToolOutcome {
    // arboard talks to the OS clipboard on Windows, macOS and Linux (X11) - no
    // PowerShell round trip, so it also works where there is no PowerShell.
    let write = write.map(|s| s.to_string());
    let wrote = write.as_ref().map(|t| t.chars().count());
    let res = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
        let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {}", e))?;
        match write {
            None => Ok(Some(cb.get_text().unwrap_or_default())),
            Some(text) => {
                cb.set_text(text).map_err(|e| format!("clipboard error: {}", e))?;
                Ok(None)
            }
        }
    })
    .await;
    match res {
        Ok(Ok(Some(text))) => {
            if text.trim().is_empty() {
                ToolOutcome::text("The clipboard is empty (or holds an image/files, not text).")
            } else {
                ToolOutcome::text(truncate(&text, MAX_FILE_READ))
            }
        }
        Ok(Ok(None)) => ToolOutcome::text(format!("Copied {} characters to the clipboard.", wrote.unwrap_or(0))),
        Ok(Err(e)) => ToolOutcome::err(e),
        Err(e) => ToolOutcome::err(format!("clipboard task failed: {}", e)),
    }
}

pub(crate) fn notify_tool(app: &AppHandle, args: &Value) -> ToolOutcome {
    use tauri_plugin_notification::NotificationExt;
    let title = args.get("title").and_then(|p| p.as_str()).unwrap_or("Kudu");
    let message = args.get("message").and_then(|p| p.as_str()).unwrap_or("");
    match app.notification().builder().title(title).body(message).show() {
        Ok(()) => ToolOutcome::text("Notification shown."),
        Err(e) => ToolOutcome::err(format!("could not show a notification: {}", e)),
    }
}
