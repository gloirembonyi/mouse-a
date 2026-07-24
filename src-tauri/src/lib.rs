// Mouse AI Desktop — Main Application Entry Point
// Sets up Tauri with system tray, global shortcuts, and all plugins.

mod agent;
mod backend;
mod capture;
mod commands;
mod explorer;
mod hover;
mod input;
mod mcp;
mod mouse;
mod oauth;

use commands::AppState;
use std::sync::Arc;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};
use tokio::sync::RwLock;

/// Load MOUSE_AI_* vars from .env.local / .env so they don't have to be set in
/// the shell. Checked in the cwd and its parent (covers `tauri dev`, whose cwd
/// is src-tauri/, with the env file at the project root). Real environment
/// variables always win — dotenvy never overrides an already-set var.
fn load_env_files() {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.to_path_buf());
        }
        candidates.push(cwd);
    }
    for dir in candidates {
        for name in [".env.local", ".env"] {
            let path = dir.join(name);
            if path.is_file() {
                let _ = dotenvy::from_path(&path);
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    load_env_files();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState {
            backend: backend::BackendClient::new(None),
            ai_active: Arc::new(RwLock::new(false)),
            conversation_id: Arc::new(RwLock::new(None)),
            overlay_pinned: Arc::new(RwLock::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            commands::capture_screen,
            commands::capture_region,
            commands::capture_at_cursor,
            commands::get_cursor_context,
            commands::get_cursor_position,
            commands::toggle_ai_active,
            commands::get_ai_active,
            commands::set_ai_active,
            commands::send_chat,
            commands::agent_run,
            commands::agent_confirm,
            commands::analyze_at_cursor,
            commands::new_conversation,
            commands::ensure_session,
            commands::start_google_signin,
            commands::sign_in_with_token,
            commands::test_backend,
            commands::get_auth_state,
            commands::sign_out,
            commands::verify_session,
            commands::expand_overlay,
            commands::collapse_overlay,
            commands::hide_overlay,
            commands::show_main_window,
            commands::ask_about_hover,
            commands::ask_at_cursor,
            commands::get_backend_url,
            commands::set_backend_url,
            commands::get_gemini_key_set,
            commands::set_gemini_key,
            commands::get_mcp_servers,
            commands::set_mcp_servers,
        ])
        .setup(|app| {
            // ── System Tray ──────────────────────────────────────
            let quit = MenuItem::with_id(app, "quit", "Quit Mouse AI", true, None::<&str>)?;
            let ask = MenuItem::with_id(app, "ask", "Ask at Cursor (Ctrl+Shift+Space)", true, None::<&str>)?;
            let toggle = MenuItem::with_id(app, "toggle", "Toggle Hover Mode (Ctrl+Shift+Q)", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&ask, &toggle, &show, &quit])?;

            let _tray = TrayIconBuilder::new()
                .icon(Image::from_bytes(include_bytes!("../icons/32x32.png"))?)
                .menu(&menu)
                .tooltip("Mouse AI — Thothai Desktop Assistant")
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "ask" => {
                        commands::trigger_ask_at_cursor(app);
                    }
                    "toggle" => {
                        let app_handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            commands::flip_ai_state(&app_handle).await;
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // ── Global Shortcuts ─────────────────────────────────
            use tauri_plugin_global_shortcut::{
                Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
            };

            // Ctrl+Shift+Space — ask about whatever is under the cursor, anywhere.
            let ask_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            // Ctrl+Shift+S — screenshot & analyze in the main window.
            let capture_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS);
            // Ctrl+Shift+Q — toggle hover mode (auto "Ask AI" pill on dwell).
            let hover_mode_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyQ);

            // NOTE: shortcut handlers fire on both key press AND release —
            // only act on Pressed, otherwise every hotkey runs twice.
            let app_handle = app.handle().clone();
            app.global_shortcut().on_shortcut(ask_shortcut, {
                let app_handle = app_handle.clone();
                move |_app, _shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    commands::trigger_ask_at_cursor(&app_handle);
                }
            })?;

            let app_handle2 = app.handle().clone();
            app.global_shortcut().on_shortcut(capture_shortcut, {
                let app_handle = app_handle2.clone();
                move |_app, _shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.emit("capture-analyze", ());
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            })?;

            let app_handle3 = app.handle().clone();
            app.global_shortcut().on_shortcut(hover_mode_shortcut, {
                let app_handle = app_handle3.clone();
                move |_app, _shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let app_handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        commands::flip_ai_state(&app_handle).await;
                    });
                }
            })?;

            // ── Main Window Setup ────────────────────────────────
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(true);
                // Start hidden — user activates via hotkey or tray
                // Window will be shown when ready via frontend
            }

            // ── Hover Watcher ────────────────────────────────────
            hover::spawn_hover_watcher(app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            // Minimize to tray instead of closing
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Mouse AI Desktop");
}
