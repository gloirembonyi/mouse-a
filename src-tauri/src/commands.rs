// Mouse AI Desktop — Tauri IPC Commands
// These are the commands exposed to the JavaScript frontend via Tauri's invoke system.

use crate::backend::{BackendClient, ChatRequest};
use crate::capture;
use crate::explorer;
use crate::mouse;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::RwLock;

/// Shared application state accessible from all commands.
pub struct AppState {
    pub backend: BackendClient,
    pub ai_active: Arc<RwLock<bool>>,
    pub conversation_id: Arc<RwLock<Option<String>>>,
    /// True while the user is interacting with the hover overlay
    /// (pauses the dwell watcher so it doesn't retrigger/hide mid-question).
    pub overlay_pinned: Arc<RwLock<bool>>,
}

#[derive(Serialize)]
pub struct CaptureResult {
    pub image_data_url: String,
    pub cursor_context: mouse::CursorContext,
}

#[derive(Serialize)]
pub struct ChatResult {
    pub text: String,
    pub conversation_id: Option<String>,
}

#[derive(Serialize)]
pub struct AuthState {
    pub signed_in: bool,
    pub user: Option<crate::backend::AuthUser>,
}

// ── Screen Capture Commands ──────────────────────────────────────

#[tauri::command]
pub async fn capture_screen() -> Result<String, String> {
    tokio::task::spawn_blocking(|| capture::capture_full_screen())
        .await
        .map_err(|e| format!("Task failed: {}", e))?
}

#[tauri::command]
pub async fn capture_region(x: i32, y: i32, width: u32, height: u32) -> Result<String, String> {
    tokio::task::spawn_blocking(move || capture::capture_region(x, y, width, height))
        .await
        .map_err(|e| format!("Task failed: {}", e))?
}

#[tauri::command]
pub async fn capture_at_cursor(radius: Option<u32>) -> Result<CaptureResult, String> {
    let r = radius.unwrap_or(200);
    let ctx = mouse::get_cursor_context();
    let cx = ctx.x;
    let cy = ctx.y;

    let image_data_url =
        tokio::task::spawn_blocking(move || capture::capture_window_or_around_point(cx, cy, r))
            .await
            .map_err(|e| format!("Task failed: {}", e))??;

    Ok(CaptureResult {
        image_data_url,
        cursor_context: ctx,
    })
}

// ── Mouse Context Commands ───────────────────────────────────────

#[tauri::command]
pub fn get_cursor_context() -> mouse::CursorContext {
    mouse::get_cursor_context()
}

#[tauri::command]
pub fn get_cursor_position() -> (i32, i32) {
    mouse::get_cursor_position()
}

// ── AI Active State Commands ─────────────────────────────────────

/// Flip AI mode on/off, sync the overlay, and broadcast the new state to
/// every window. Shared by the command, the tray menu, and the hotkey.
pub async fn flip_ai_state(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let now = {
        let mut active = state.ai_active.write().await;
        *active = !*active;
        *active
    };
    apply_ai_side_effects(app, now).await;
    now
}

async fn apply_ai_side_effects(app: &AppHandle, active: bool) {
    let state = app.state::<AppState>();
    if active {
        // Brief "AI active" toast next to the cursor so the user gets feedback
        // without the main window popping up.
        crate::hover::show_overlay_near_cursor(app);
        let _ = app.emit_to("overlay", "ai-toast", true);
    } else {
        *state.overlay_pinned.write().await = false;
        if let Some(overlay) = app.get_webview_window("overlay") {
            let _ = overlay.hide();
        }
    }
    let _ = app.emit("ai-state", active);
}

#[tauri::command]
pub async fn toggle_ai_active(app: AppHandle) -> Result<bool, String> {
    Ok(flip_ai_state(&app).await)
}

#[tauri::command]
pub async fn get_ai_active(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(*state.ai_active.read().await)
}

#[tauri::command]
pub async fn set_ai_active(
    app: AppHandle,
    state: State<'_, AppState>,
    active: bool,
) -> Result<bool, String> {
    *state.ai_active.write().await = active;
    apply_ai_side_effects(&app, active).await;
    Ok(active)
}

/// The core "ask anywhere" flow: user presses the hotkey → grab the cursor
/// position + what's under it → open the input panel right there.
pub fn trigger_ask_at_cursor(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctx = mouse::get_cursor_context();
        let (x, y) = (ctx.x, ctx.y);

        let element =
            tokio::task::spawn_blocking(move || mouse::get_element_under_cursor(x, y))
                .await
                .unwrap_or_default();

        // Pin so the dwell watcher doesn't hide/steal the panel while typing.
        let state = app.state::<AppState>();
        *state.overlay_pinned.write().await = true;

        // Size to panel BEFORE positioning so monitor clamping is correct.
        if let Some(overlay) = app.get_webview_window("overlay") {
            let _ = overlay.set_size(tauri::LogicalSize::new(440.0, 380.0));
        }
        crate::hover::show_overlay_near(&app, x, y);
        if let Some(overlay) = app.get_webview_window("overlay") {
            let _ = overlay.set_focus();
        }

        let _ = app.emit_to(
            "overlay",
            "trigger-ask",
            crate::hover::HoverPayload {
                x,
                y,
                context: ctx,
                element,
            },
        );
    });
}

#[tauri::command]
pub fn ask_at_cursor(app: AppHandle) {
    trigger_ask_at_cursor(&app);
}

// The header ✕ / window close intentionally only hides to the tray (see
// lib.rs's on_window_event) — this is the one explicit way to actually exit,
// surfaced as a "Quit Mouse AI" button in Settings since the tray menu isn't
// discoverable from inside the app itself.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ── Settings Commands ────────────────────────────────────────────

#[tauri::command]
pub async fn get_backend_url(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.backend.get_backend_url().await)
}

#[tauri::command]
pub async fn set_backend_url(state: State<'_, AppState>, url: String) -> Result<String, String> {
    state.backend.set_backend_url(url).await;
    Ok(state.backend.get_backend_url().await)
}

/// Reachability check for the configured backend (Settings → Test connection).
#[tauri::command]
pub async fn test_backend(state: State<'_, AppState>) -> Result<String, String> {
    state.backend.ping().await
}

/// The configured MCP servers as a JSON string (for the Settings editor).
#[tauri::command]
pub async fn get_mcp_servers(state: State<'_, AppState>) -> Result<String, String> {
    let v = state.backend.get_mcp_servers().await;
    Ok(serde_json::to_string_pretty(&v).unwrap_or_else(|_| "[]".to_string()))
}

/// Save the MCP server list from a JSON string. Validates it's a JSON array
/// so a typo can't corrupt the config.
#[tauri::command]
pub async fn set_mcp_servers(state: State<'_, AppState>, json: String) -> Result<String, String> {
    let trimmed = json.trim();
    let value: serde_json::Value = if trimmed.is_empty() {
        serde_json::json!([])
    } else {
        serde_json::from_str(trimmed).map_err(|e| format!("Invalid JSON: {}", e))?
    };
    if !value.is_array() {
        return Err("MCP servers must be a JSON array.".to_string());
    }
    state.backend.set_mcp_servers(value).await;
    Ok("Saved.".to_string())
}

// ── Overlay Commands ─────────────────────────────────────────────

#[tauri::command]
pub async fn expand_overlay(
    app: AppHandle,
    state: State<'_, AppState>,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<(), String> {
    *state.overlay_pinned.write().await = true;
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_size(tauri::LogicalSize::new(
            width.unwrap_or(420.0),
            height.unwrap_or(360.0),
        ));
        let _ = overlay.show();
        let _ = overlay.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub async fn collapse_overlay(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    *state.overlay_pinned.write().await = false;
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
        let _ = overlay.set_size(tauri::LogicalSize::new(380.0, 64.0));
    }
    Ok(())
}

#[tauri::command]
pub async fn hide_overlay(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // Auto-hide from the overlay's own timer — only when not pinned.
    if !*state.overlay_pinned.read().await {
        if let Some(overlay) = app.get_webview_window("overlay") {
            let _ = overlay.hide();
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn show_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}

/// Ask the AI about whatever the user paused the mouse over. Combines a
/// small screenshot around the point with the accessible element text.
#[tauri::command]
pub async fn ask_about_hover(
    state: State<'_, AppState>,
    question: Option<String>,
    x: i32,
    y: i32,
    app_name: Option<String>,
    window_title: Option<String>,
    element_text: Option<String>,
) -> Result<ChatResult, String> {
    // A failed capture (secure screens, etc.) shouldn't kill the ask —
    // the element text alone is often enough. Captures the whole window
    // under the cursor (not just a small crop) so the model actually sees
    // everything relevant, falling back to a radius crop if that fails.
    let image_data_url =
        tokio::task::spawn_blocking(move || capture::capture_window_or_around_point(x, y, 280))
            .await
            .map_err(|e| format!("Task failed: {}", e))?
            .ok();

    // Ground-truth folder contents when hovering a File Explorer window —
    // fixes "I can't see what's inside this folder" for the single most
    // common context question.
    let explorer_ctx = tokio::task::spawn_blocking(move || explorer::get_context_at(x, y))
        .await
        .unwrap_or(None);

    let mut context_info = format!(
        "[DESKTOP HOVER CONTEXT]\nApplication: {}\nWindow: {}\nCursor position: ({}, {})",
        app_name.unwrap_or_default(),
        window_title.unwrap_or_default(),
        x,
        y
    );
    if let Some(el) = element_text.filter(|s| !s.trim().is_empty()) {
        context_info.push_str("\nHovered element: ");
        context_info.push_str(&el);
    }
    if let Some(ctx) = &explorer_ctx {
        context_info.push_str("\n\n");
        context_info.push_str(&explorer::format_context(ctx));
    }

    let prompt = match question.filter(|q| !q.trim().is_empty()) {
        Some(q) => format!(
            "{}\n\nThe user is hovering over this on their screen and asks: {}",
            context_info, q
        ),
        None => format!(
            "{}\n\nBriefly explain what the user is hovering over in this screenshot. Be concise and helpful — a few sentences, no preamble.",
            context_info
        ),
    };

    let conv_id = state.conversation_id.read().await.clone();
    let request = ChatRequest {
        user_text: prompt,
        model: Some(crate::backend::default_gemini_model()),
        image_data_url,
        web_search_needed: false,
        conversation_id: conv_id,
        messages: vec![],
    };

    let response = state.backend.chat(request).await?;
    if let Some(cid) = &response.conversation_id {
        *state.conversation_id.write().await = Some(cid.clone());
    }

    Ok(ChatResult {
        text: response.text.unwrap_or_default(),
        conversation_id: response.conversation_id,
    })
}

// ── Agent Commands ───────────────────────────────────────────────

/// Run a full autonomous agent task: the model plans and calls local tools
/// (shell, files, web research, computer-use) until the task is done, then
/// returns the final answer to whichever window asked. `source` ("overlay" |
/// "main") tags the live `agent-progress` events so the right panel renders
/// the activity log. If x/y are omitted, the current cursor is used so the
/// agent grounds itself in whatever the user is looking at.
#[tauri::command]
pub async fn agent_run(
    app: AppHandle,
    state: State<'_, AppState>,
    task: String,
    x: Option<i32>,
    y: Option<i32>,
    source: Option<String>,
    autonomous: Option<bool>,
    image_data_url: Option<String>,
) -> Result<ChatResult, String> {
    if task.trim().is_empty() {
        return Err("Empty task.".to_string());
    }
    let (cx, cy) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        _ => mouse::get_cursor_position(),
    };
    let source = source.unwrap_or_else(|| "main".to_string());
    let autonomous = autonomous.unwrap_or(true);
    // Same shared conversation the chat view and the overlay's plain-ask both
    // read/write — recent turns get folded into the agent's starting context
    // below so a hover "ask AI" task isn't blind to what was just discussed.
    let conversation_id = state.conversation_id.read().await.clone();

    let result = crate::agent::run_task(
        &app,
        &state.backend,
        &task,
        cx,
        cy,
        &source,
        autonomous,
        conversation_id,
        image_data_url,
    )
    .await;

    match result {
        Ok(text) => Ok(ChatResult { text, conversation_id: None }),
        Err(e) => {
            let _ = app.emit(
                "agent-progress",
                crate::agent::AgentProgress {
                    source: source.clone(),
                    kind: "error".to_string(),
                    message: e.clone(),
                    request_id: None,
                },
            );
            Err(e)
        }
    }
}

/// Called when the user clicks Allow/Deny on an agent's `request_confirmation`
/// prompt. Returns false if that request already timed out or doesn't exist.
#[tauri::command]
pub fn agent_confirm(request_id: String, approved: bool) -> bool {
    crate::agent::resolve_confirmation(&request_id, approved)
}

// ── Chat Commands ────────────────────────────────────────────────

#[tauri::command]
pub async fn send_chat(
    state: State<'_, AppState>,
    text: String,
    image_data_url: Option<String>,
    web_search: Option<bool>,
) -> Result<ChatResult, String> {
    let conv_id = state.conversation_id.read().await.clone();

    let request = ChatRequest {
        user_text: text,
        model: Some(crate::backend::default_gemini_model()),
        image_data_url,
        web_search_needed: web_search.unwrap_or(false),
        conversation_id: conv_id,
        messages: vec![],
    };

    let response = state.backend.chat(request).await?;
    let text = response.text.unwrap_or_default();

    // Update conversation ID for continuity
    if let Some(cid) = &response.conversation_id {
        *state.conversation_id.write().await = Some(cid.clone());
    }

    Ok(ChatResult {
        text,
        conversation_id: response.conversation_id,
    })
}

/// Smart analysis: capture screen around cursor + get context + send to AI
#[tauri::command]
pub async fn analyze_at_cursor(
    state: State<'_, AppState>,
    user_question: Option<String>,
    radius: Option<u32>,
) -> Result<ChatResult, String> {
    let r = radius.unwrap_or(300);
    let ctx = mouse::get_cursor_context();
    let cx = ctx.x;
    let cy = ctx.y;

    // Capture the whole window under the cursor (not just a radius crop)
    // so the model sees everything relevant on screen.
    let image_data_url =
        tokio::task::spawn_blocking(move || capture::capture_window_or_around_point(cx, cy, r))
            .await
            .map_err(|e| format!("Task failed: {}", e))??;

    let element =
        tokio::task::spawn_blocking(move || mouse::get_element_under_cursor(cx, cy))
            .await
            .unwrap_or_default();
    let explorer_ctx = tokio::task::spawn_blocking(move || explorer::get_context_at(cx, cy))
        .await
        .unwrap_or(None);

    // Build context-aware prompt
    let mut context_info = format!(
        "[DESKTOP CONTEXT]\nApplication: {}\nWindow: {}\nCursor position: ({}, {})",
        ctx.process_name, ctx.window_title, ctx.x, ctx.y
    );
    let element_summary = [element.role.as_str(), element.name.as_str(), element.value.as_str()]
        .iter()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    if !element_summary.is_empty() {
        context_info.push_str("\nElement under cursor: ");
        context_info.push_str(&element_summary);
    }
    if let Some(ectx) = &explorer_ctx {
        context_info.push_str("\n\n");
        context_info.push_str(&explorer::format_context(ectx));
    }

    let prompt = if let Some(q) = user_question {
        format!("{}\n\nUser question: {}", context_info, q)
    } else {
        format!(
            "{}\n\nAnalyze what is shown in this screenshot. Describe what you see and provide helpful information about it.",
            context_info
        )
    };

    let conv_id = state.conversation_id.read().await.clone();

    let request = ChatRequest {
        user_text: prompt,
        model: Some(crate::backend::default_gemini_model()),
        image_data_url: Some(image_data_url),
        web_search_needed: true,
        conversation_id: conv_id,
        messages: vec![],
    };

    let response = state.backend.chat(request).await?;
    let text = response.text.unwrap_or_default();

    if let Some(cid) = &response.conversation_id {
        *state.conversation_id.write().await = Some(cid.clone());
    }

    Ok(ChatResult {
        text,
        conversation_id: response.conversation_id,
    })
}

/// Start a new conversation
#[tauri::command]
pub async fn new_conversation(state: State<'_, AppState>) -> Result<(), String> {
    *state.conversation_id.write().await = None;
    Ok(())
}

// ── History Commands ─────────────────────────────────────────────

#[tauri::command]
pub async fn get_history_stats(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state.backend.get_history_stats().await
}

#[tauri::command]
pub async fn get_conversations(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<crate::backend::HistoryConversation>, String> {
    state.backend.get_conversations(limit).await
}

#[tauri::command]
pub async fn get_conversation_messages(
    state: State<'_, AppState>,
    id: String,
    limit: Option<u32>,
) -> Result<Vec<crate::backend::HistoryMessage>, String> {
    state.backend.get_conversation_messages(&id, limit).await
}

/// Resume a past conversation — the next chat message continues this thread
/// (same mechanism new_conversation resets) instead of starting a fresh one.
#[tauri::command]
pub async fn set_conversation_id(state: State<'_, AppState>, id: Option<String>) -> Result<(), String> {
    *state.conversation_id.write().await = id;
    Ok(())
}

/// Permanently delete a past conversation. If it's the one currently open,
/// clears conversation_id too so the next message doesn't try to continue a
/// thread that no longer exists.
#[tauri::command]
pub async fn delete_conversation(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.backend.delete_conversation(&id).await?;
    let mut current = state.conversation_id.write().await;
    if current.as_deref() == Some(id.as_str()) {
        *current = None;
    }
    Ok(())
}

// ── Auth Commands ────────────────────────────────────────────────

/// Called on app startup instead of gating on a login screen: if there's
/// already a session (guest or Google), keep it; otherwise silently
/// provision an anonymous guest session so the app is usable right away.
/// Google sign-in (`start_google_signin`) stays available separately for
/// anyone who wants to link a real account later.
#[tauri::command]
pub async fn ensure_session(state: State<'_, AppState>) -> Result<AuthState, String> {
    if state.backend.is_signed_in().await {
        return Ok(AuthState {
            signed_in: true,
            user: state.backend.get_current_user().await,
        });
    }

    let response = state.backend.guest_sign_in().await?;
    Ok(AuthState {
        signed_in: response.success,
        user: response.user,
    })
}

/// Full browser-based Google sign-in: opens the consent screen in the default
/// browser, catches the redirect on a localhost listener, then exchanges the
/// access token for a Thothai backend session.
#[tauri::command]
pub async fn start_google_signin(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AuthState, String> {
    use tauri_plugin_opener::OpenerExt;

    let listener = crate::oauth::bind_listener()?;
    let url = crate::oauth::build_auth_url();
    if let Err(e) = app.opener().open_url(url, None::<&str>) {
        crate::oauth::release_flow();
        return Err(format!("Couldn't open your browser for sign-in: {}", e));
    }

    let token = tokio::task::spawn_blocking(move || crate::oauth::wait_for_token(listener))
        .await
        .map_err(|e| {
            crate::oauth::release_flow();
            format!("Sign-in task failed: {}", e)
        })??;

    let response = state.backend.google_sign_in(&token).await?;
    Ok(AuthState {
        signed_in: response.success,
        user: response.user,
    })
}

#[tauri::command]
pub async fn sign_in_with_token(
    state: State<'_, AppState>,
    access_token: String,
) -> Result<AuthState, String> {
    let response = state.backend.google_sign_in(&access_token).await?;

    Ok(AuthState {
        signed_in: response.success,
        user: response.user,
    })
}

#[tauri::command]
pub async fn get_auth_state(state: State<'_, AppState>) -> Result<AuthState, String> {
    let user = state.backend.get_current_user().await;
    let signed_in = state.backend.is_signed_in().await;

    Ok(AuthState {
        signed_in,
        user,
    })
}

#[tauri::command]
pub async fn sign_out(state: State<'_, AppState>) -> Result<(), String> {
    state.backend.clear_session().await;
    *state.conversation_id.write().await = None;
    Ok(())
}

#[tauri::command]
pub async fn verify_session(state: State<'_, AppState>) -> Result<AuthState, String> {
    match state.backend.get_me().await {
        Ok(resp) => Ok(AuthState {
            signed_in: resp.success,
            user: resp.user,
        }),
        Err(_) => {
            // get_me() already clears the session on a real 401 rejection.
            // For network errors keep the cached session so being offline
            // doesn't sign the user out.
            Ok(AuthState {
                signed_in: state.backend.is_signed_in().await,
                user: state.backend.get_current_user().await,
            })
        }
    }
}
