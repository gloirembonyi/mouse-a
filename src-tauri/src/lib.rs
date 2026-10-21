// Kudu Desktop - Main Application Entry Point
// Sets up Tauri with system tray, global shortcuts, and all plugins.

mod agent;
mod analytics;
mod auth_sync;
mod backend;
mod bridge;
mod link;
mod capture;
mod commands;
mod documents;
mod explorer;
mod fs_browser;
mod git_diff;
mod hover;
mod input;
mod kudu_files;
mod mcp;
mod skills;
mod text_format;
mod copilot;
mod meeting;
mod meeting_stream;
mod sessions;
mod mouse;
mod oauth;
mod prefs;
mod quill;
mod shield;
mod terminal;
mod uia;

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
/// variables always win - dotenvy never overrides an already-set var.
fn load_env_files() {
    // Development only. An installed app must never read a .env file from whatever
    // folder it happens to be started in: a planted .env.local could point
    // MOUSE_AI_BACKEND_URL at someone else's server and capture the sign-in.
    // Release builds use the built-in defaults (DEFAULT_BACKEND_URL, DEFAULT_WEB_CLIENT_ID).
    if !cfg!(debug_assertions) {
        return;
    }
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
        // In-app updates: the frontend's lib/updater.ts checks the signed
        // latest.json on the GitHub releases and installs; process relaunches.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState {
            backend: backend::BackendClient::new(None),
            ai_active: Arc::new(RwLock::new(false)),
            conversation_id: Arc::new(RwLock::new(None)),
            overlay_pinned: Arc::new(RwLock::new(false)),
        })
        .manage(terminal::PtyState::default())
        .invoke_handler(tauri::generate_handler![
            analytics::analytics_track,
            commands::get_history_changes,
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
            commands::agent_cancel_all,
            commands::agent_active_runs,
            commands::agent_steer,
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
            commands::open_in_main,
            commands::copy_text,
            commands::project_notes_append,
            commands::project_notes_read,
            commands::ask_about_hover,
            commands::resolve_mention,
            commands::ask_at_cursor,
            commands::get_backend_url,
            commands::set_backend_url,
            commands::get_mcp_servers,
            commands::set_mcp_servers,
            commands::quit_app,
            commands::install_kind,
            fs_browser::fs_home_dir,
            fs_browser::fs_list_dir,
            fs_browser::fs_read_text_file,
            fs_browser::fs_write_text_file,
            fs_browser::fs_parent_dir,
            fs_browser::fs_search_files,
            fs_browser::fs_file_info,
            fs_browser::fs_read_data_url,
            fs_browser::fs_create_file,
            fs_browser::fs_create_dir,
            fs_browser::fs_rename,
            fs_browser::fs_delete,
            fs_browser::fs_reveal,
            fs_browser::fs_open_default,
            kudu_files::kudu_file_save,
            kudu_files::fs_write_base64,
            documents::doc_read_docx,
            documents::doc_read_pptx,
            documents::doc_read_sheet,
            documents::fs_extract_text,
            terminal::pty_spawn,
            terminal::pty_write,
            terminal::pty_resize,
            terminal::pty_kill,
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
            meeting::meeting_restore,
            sessions::session_save,
            sessions::session_list,
            sessions::session_get,
            sessions::session_delete,
            meeting::meeting_assist,
            meeting::meeting_get_profile,
            meeting::meeting_set_profile,
            meeting::meeting_save_notes,
            meeting::copilot_show,
            meeting::copilot_hide,
            meeting::copilot_resize,
            meeting::copilot_focus,
            copilot::copilot_agent_run,
            copilot::copilot_insert,
            quill::quill_apply,
            quill::quill_pending,
            quill::quill_hide,
            quill::writing_api,
            quill::memory_api,
            copilot::copilot_insert_cancel,
            meeting::stealth_get,
            meeting::stealth_set,
            meeting::meeting_research,
            meeting::meeting_context_append,
            prefs::window_prefs_get,
            prefs::window_prefs_set,
        ])
        .setup(|app| {
            // ── System Tray ──────────────────────────────────────
            let quit = MenuItem::with_id(app, "quit", "Quit Kudu Desktop", true, None::<&str>)?;
            let ask = MenuItem::with_id(app, "ask", "Ask Kudu at cursor… (Ctrl+Shift+Space)", true, None::<&str>)?;
            let toggle = MenuItem::with_id(app, "toggle", "Toggle Kudu Lens (Ctrl+Shift+Q)", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Open Kudu Desktop", true, None::<&str>)?;
            let copilot = MenuItem::with_id(app, "copilot", r"Kudu Pilot bar (Ctrl+Shift+\)", true, None::<&str>)?;
            let taskbar = tauri::menu::CheckMenuItem::with_id(
                app,
                "taskbar",
                "Show in Taskbar",
                true,
                !prefs::get().hide_from_taskbar,
                None::<&str>,
            )?;
            let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
            let sep2 = tauri::menu::PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&show, &ask, &toggle, &copilot, &sep, &taskbar, &sep2, &quit])?;
            let taskbar_item = taskbar.clone();

            let _tray = TrayIconBuilder::new()
                .icon(Image::from_bytes(include_bytes!("../icons/32x32.png"))?)
                .menu(&menu)
                .tooltip("Kudu Desktop")
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
                    "copilot" => {
                        meeting::toggle_copilot(app);
                    }
                    "taskbar" => {
                        let mut p = prefs::get();
                        p.hide_from_taskbar = !p.hide_from_taskbar;
                        let _ = taskbar_item.set_checked(!p.hide_from_taskbar);
                        let _ = prefs::update(app, p);
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

            // Ctrl+Shift+Space - ask about whatever is under the cursor, anywhere.
            let ask_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            // Ctrl+Shift+S - screenshot & analyze in the main window.
            let capture_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS);
            // Ctrl+Shift+Q - toggle hover mode (auto "Ask Kudu" pill on dwell).
            let hover_mode_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyQ);
            // Ctrl+Shift+Enter - meeting copilot: "What should I say?"
            let meeting_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Enter);

            // NOTE: shortcut handlers fire on both key press AND release -
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
                    // Mid-call, "look at my screen" means: answer what's on it,
                    // right in the copilot bar over the call.
                    if meeting::is_live() && app_handle.get_webview_window("copilot").is_some() {
                        meeting::show_copilot(&app_handle);
                        let _ = app_handle.emit_to("copilot", "copilot-assist", "screen");
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
                    // During a live call the in-call bar answers, so the
                    // user never leaves the meeting app; otherwise fall back
                    // to the full copilot in the main window.
                    if meeting::is_live() && app_handle.get_webview_window("copilot").is_some() {
                        meeting::show_copilot(&app_handle);
                        let _ = app_handle.emit_to("copilot", "copilot-assist", "suggest");
                        return;
                    }
                    // Show without stealing focus from the call window.
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.emit("meeting-hotkey", ());
                    }
                }
            }));

            // Ctrl+Shift+\ - show/hide the in-call copilot bar. (Plain Ctrl+\ is
            // taken: the main window uses it to hide the chat next to the editor.)
            let copilot_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Backslash);
            let app_handle5 = app.handle().clone();
            report("Ctrl+Shift+\\", app.global_shortcut().on_shortcut(copilot_shortcut, {
                let app_handle = app_handle5.clone();
                move |_app, _shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    meeting::toggle_copilot(&app_handle);
                }
            }));

            // Ctrl+Alt+G - Kudu Quill anywhere: check the selection (or the whole field) of the app
            // you are in, in the small Quill window. Works offline.
            let quill_shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyG);
            let app_handle6 = app.handle().clone();
            report("Ctrl+Alt+G", app.global_shortcut().on_shortcut(quill_shortcut, {
                let app_handle = app_handle6.clone();
                move |_app, _shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    quill::check_selection(&app_handle);
                }
            }));

            // Keep Kudu Desktop out of screen shares / recordings by default.
            meeting::apply_stealth(app.handle());
            copilot::start_foreground_watch();

            // ── Main Window Setup ────────────────────────────────
            prefs::apply(app.handle());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(true);
                // Run quietly from the tray when the user asked for that.
                if prefs::get().start_in_background {
                    let _ = window.hide();
                }
                // Start hidden - user activates via hotkey or tray
                // Window will be shown when ready via frontend
            }

            // ── Hover Watcher ────────────────────────────────────
            hover::spawn_hover_watcher(app.handle().clone());

            // ── Browser bridge (WebSocket server for the extension) ──
            // Lets the desktop agent delegate web tasks to the Kudu Connect browser
            // extension in real time. Runs for the whole app lifetime.
            link::start(app.handle().clone());
            let bridge_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                bridge::serve(bridge_app).await;
            });

            analytics::track(
                app.handle(),
                "app_open",
                serde_json::json!({ "start_in_background": prefs::get().start_in_background }),
            );

            Ok(())
        })
        .on_window_event(|window, event| {
            // Minimize to tray instead of closing
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
            // The main window is also shown from the frontend (appWindow.show());
            // focus is the one signal every show path produces, so the screen-share
            // rule catches up here if it changed while the window was hidden.
            if let WindowEvent::Focused(true) = event {
                if let Some(w) = window.app_handle().get_webview_window(window.label()) {
                    crate::meeting::protect_shown(&w);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Kudu Desktop");
}
