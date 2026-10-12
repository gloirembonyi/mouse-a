// Mouse AI Desktop — Main Application Entry Point
// Sets up Tauri with system tray, global shortcuts, and all plugins.

mod agent;
mod backend;
mod bridge;
mod link;
mod capture;
mod commands;
mod explorer;
mod fs_browser;
mod git_diff;
mod hover;
mod input;
mod mcp;
mod meeting;
mod mouse;
mod oauth;
mod terminal;

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
        // Must be the first plugin. A second launch used to start a full second
        // copy: another tray icon, hotkeys that fail to register (fatal, see
        // below) and a bridge that can't bind its port. Now the second launch
        // just brings the running window forward and exits.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            backend: backend::BackendClient::new(None),
            ai_active: Arc::new(RwLock::new(false)),
            conversation_id: Arc::new(RwLock::new(None)),
            overlay_pinned: Arc::new(RwLock::new(false)),
        })
        .manage(terminal::PtyState::default())
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
            commands::agent_cancel,
            commands::analyze_at_cursor,
            commands::new_conversation,
            commands::get_history_stats,
            commands::get_conversations,
            commands::get_conversation_messages,
            commands::set_conversation_id,
            commands::delete_conversation,
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
            commands::get_mcp_servers,
            commands::set_mcp_servers,
            commands::quit_app,
            fs_browser::fs_home_dir,
            fs_browser::fs_list_dir,
            fs_browser::fs_read_text_file,
            fs_browser::fs_write_text_file,
            fs_browser::fs_parent_dir,
            fs_browser::fs_search_files,
            terminal::pty_spawn,
            terminal::pty_write,
            terminal::pty_resize,
            git_diff::git_status,
            git_diff::git_diff_file,
            commands::bridge_status,
            link::link_info,
            link::link_set_accept_tasks,
            link::link_new_pair_code,
            link::link_tasks,
            link::link_cancel_task,
            link::link_clear_finished,
            meeting::meeting_start,
            meeting::meeting_stop,
            meeting::meeting_status,
            meeting::meeting_transcript,
            meeting::meeting_add_entry,
            meeting::meeting_clear,
            meeting::meeting_assist,
            meeting::meeting_get_profile,
            meeting::meeting_set_profile,
            meeting::meeting_save_notes,
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
            // Ctrl+Shift+Enter — meeting copilot: "What should I say?"
            let meeting_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Enter);

            // NOTE: shortcut handlers fire on both key press AND release —
            // only act on Pressed, otherwise every hotkey runs twice.
            //
            // Registration failures are reported, not propagated: if another
            // app already owns a combo, `?` here used to abort setup and the
            // whole app died at launch. Now that one hotkey is just missing
            // (the tray menu still offers the same actions).
            let report = |name: &str, res: Result<(), tauri_plugin_global_shortcut::Error>| {
                if let Err(e) = res {
                    eprintln!("[shortcuts] could not register {}: {}", name, e);
                    let _ = app.handle().emit(
                        "shortcut-unavailable",
                        serde_json::json!({ "shortcut": name, "error": e.to_string() }),
                    );
                }
            };

            let app_handle = app.handle().clone();
            report("Ctrl+Shift+Space", app.global_shortcut().on_shortcut(ask_shortcut, {
                let app_handle = app_handle.clone();
                move |_app, _shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    commands::trigger_ask_at_cursor(&app_handle);
                }
            }));

            let app_handle2 = app.handle().clone();
            report("Ctrl+Shift+S", app.global_shortcut().on_shortcut(capture_shortcut, {
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
            }));

            let app_handle3 = app.handle().clone();
            report("Ctrl+Shift+Q", app.global_shortcut().on_shortcut(hover_mode_shortcut, {
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
            }));

            let app_handle4 = app.handle().clone();
            report("Ctrl+Shift+Enter", app.global_shortcut().on_shortcut(meeting_shortcut, {
                let app_handle = app_handle4.clone();
                move |_app, _shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    // Show without stealing focus from the call window.
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.emit("meeting-hotkey", ());
                    }
                }
            }));

            // ── Main Window Setup ────────────────────────────────
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(true);
                // Start hidden — user activates via hotkey or tray
                // Window will be shown when ready via frontend
            }

            // ── Hover Watcher ────────────────────────────────────
            hover::spawn_hover_watcher(app.handle().clone());

            // ── Browser bridge (WebSocket server for the extension) ──
            // Lets the desktop agent delegate web tasks to the Thothai browser
            // extension in real time. Runs for the whole app lifetime.
            link::start(app.handle().clone());
            let bridge_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                bridge::serve(bridge_app).await;
            });

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
