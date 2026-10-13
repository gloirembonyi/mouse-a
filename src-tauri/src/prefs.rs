// Mouse AI Desktop - window preferences
//
// How the app sits on the desktop: whether the main window has a taskbar
// button, and whether it opens on launch or starts quietly in the tray. With
// both on, Mouse AI lives only behind the tray icon (the "^" overflow on the
// taskbar) and its hotkeys.
//
// Its own file rather than config.json, for the same reason as the meeting
// profile: BackendClient rewrites config.json from a fixed struct.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowPrefs {
    /// No taskbar button for the main window - reach it from the tray icon.
    #[serde(default)]
    pub hide_from_taskbar: bool,
    /// Don't open the main window at launch; run in the tray.
    #[serde(default)]
    pub start_in_background: bool,
}

static PREFS: Mutex<Option<WindowPrefs>> = Mutex::new(None);

fn prefs_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mouse-ai").join("window_prefs.json"))
}

pub fn get() -> WindowPrefs {
    let mut cached = PREFS.lock().unwrap();
    if cached.is_none() {
        *cached = Some(
            prefs_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
        );
    }
    cached.clone().unwrap_or_default()
}

fn save(prefs: &WindowPrefs) -> Result<(), String> {
    let path = prefs_path().ok_or("No config directory available")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(prefs).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("Failed to save window preferences: {}", e))?;
    *PREFS.lock().unwrap() = Some(prefs.clone());
    Ok(())
}

/// Apply the taskbar preference to the main window (the overlay and the
/// copilot bar never have a taskbar button).
pub fn apply(app: &AppHandle) {
    let prefs = get();
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_skip_taskbar(prefs.hide_from_taskbar);
    }
}

pub fn update(app: &AppHandle, prefs: WindowPrefs) -> Result<WindowPrefs, String> {
    save(&prefs)?;
    apply(app);
    let _ = app.emit("window-prefs", &prefs);
    Ok(prefs)
}

#[tauri::command]
pub fn window_prefs_get() -> WindowPrefs {
    get()
}

#[tauri::command]
pub fn window_prefs_set(app: AppHandle, prefs: WindowPrefs) -> Result<WindowPrefs, String> {
    update(&app, prefs)
}
