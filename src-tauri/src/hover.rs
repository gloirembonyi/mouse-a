// Kudu Desktop - Global Hover-Dwell Watcher
// While AI mode is active, watches the cursor system-wide. When the mouse
// pauses over something (dwell), it shows the floating "Ask Kudu" overlay next
// to the cursor and sends it the context of what's being hovered.

use crate::commands::AppState;
use crate::mouse;
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Serialize)]
pub struct HoverPayload {
    pub x: i32,
    pub y: i32,
    pub context: mouse::CursorContext,
    pub element: mouse::ElementInfo,
    /// Top-level window the pill appeared over (0 = none / our own). Every later
    /// capture and context read uses this, because once the panel is open the
    /// point under the cursor is the panel itself.
    pub hwnd: i64,
}

const POLL_MS: u64 = 90;
const IDLE_POLL_MS: u64 = 400;
const DWELL_MS: u128 = 550;
const MOVE_THRESHOLD: i32 = 6;
const RETRIGGER_DISTANCE: i32 = 48;
const HIDE_DISTANCE: i32 = 340;

pub fn spawn_hover_watcher(app: AppHandle) {
    let own_exe = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_name()
                .map(|f| f.to_string_lossy().to_lowercase())
        })
        .unwrap_or_default();

    tauri::async_runtime::spawn(async move {
        let mut last_pos = mouse::get_cursor_position();
        let mut still_since = Instant::now();
        let mut dwell_handled = true;
        let mut last_trigger_pos = (i32::MIN / 2, i32::MIN / 2);
        let mut overlay_anchor = (i32::MIN / 2, i32::MIN / 2);
        // Only true between a hover-dwell trigger and the pill being
        // hidden/dismissed - distinguishes "the collapsed pill is up and
        // should drift with the cursor" from other reasons the overlay window
        // might be visible (the brief AI-on/off toast, the expanded panel),
        // which must NOT get repositioned by ordinary mouse movement.
        let mut pill_showing = false;

        loop {
            tokio::time::sleep(Duration::from_millis(POLL_MS)).await;

            let state = app.state::<AppState>();
            if !*state.ai_active.read().await {
                tokio::time::sleep(Duration::from_millis(IDLE_POLL_MS)).await;
                continue;
            }
            // While the user is typing in the overlay, don't retrigger or hide.
            if *state.overlay_pinned.read().await {
                continue;
            }

            let (x, y) = mouse::get_cursor_position();
            let moved = (x - last_pos.0).abs().max((y - last_pos.1).abs());

            if moved > MOVE_THRESHOLD {
                last_pos = (x, y);
                still_since = Instant::now();
                dwell_handled = false;

                let away = (x - overlay_anchor.0)
                    .abs()
                    .max((y - overlay_anchor.1).abs());
                if let Some(overlay) = app.get_webview_window("overlay") {
                    if away > HIDE_DISTANCE {
                        // Cursor left the overlay area - hide the pill again.
                        pill_showing = false;
                        let _ = overlay.hide();
                    } else if pill_showing && overlay.is_visible().unwrap_or(false) {
                        // Still nearby and the collapsed pill is what's up (the
                        // expanded panel pins itself via overlay_pinned, which
                        // already `continue`d past this whole branch above) -
                        // drift the pill along with the cursor instead of
                        // leaving it stuck at the original dwell point.
                        overlay_anchor = (x, y);
                        // Drift only: reposition, don't re-sample the theme.
                        // Sampling screenshots the region, and this branch runs
                        // every poll while the mouse moves.
                        position_overlay(&app, x, y, false);
                    }
                }
                continue;
            }

            if dwell_handled || still_since.elapsed().as_millis() < DWELL_MS {
                continue;
            }
            dwell_handled = true;

            let ctx = mouse::get_cursor_context();

            // Never trigger while hovering Kudu Desktop's own windows.
            if !own_exe.is_empty() && ctx.process_name.to_lowercase() == own_exe {
                continue;
            }
            // Ignore the bare desktop (no window under cursor).
            if ctx.process_name.is_empty() && ctx.window_title.is_empty() {
                continue;
            }
            // Don't re-show for the same spot the user just dismissed.
            let dist = (x - last_trigger_pos.0)
                .abs()
                .max((y - last_trigger_pos.1).abs());
            if dist < RETRIGGER_DISTANCE {
                continue;
            }

            let element =
                tokio::task::spawn_blocking(move || mouse::get_element_under_cursor(x, y))
                    .await
                    .unwrap_or_default();

            last_trigger_pos = (x, y);
            overlay_anchor = (x, y);
            pill_showing = true;
            let hwnd = mouse::target_hwnd_at_point(x, y) as i64;

            show_overlay_near(&app, x, y);
            let _ = app.emit_to(
                "overlay",
                "hover-dwell",
                HoverPayload {
                    x,
                    y,
                    context: ctx,
                    element,
                    hwnd,
                },
            );
        }
    });
}

/// Position the overlay window next to a screen point (clamped to the
/// monitor that point is on), show it without stealing focus, and pick its
/// light/dark surface from what's behind it.
pub fn show_overlay_near(app: &AppHandle, x: i32, y: i32) {
    position_overlay(app, x, y, true);
}

fn position_overlay(app: &AppHandle, x: i32, y: i32, sample_theme: bool) {
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    let (w, h) = overlay
        .outer_size()
        .map(|s| (s.width as i32, s.height as i32))
        .unwrap_or((380, 64));

    // Close to the pointer, like an editor's inline hint - but never under it.
    let mut px = x + 12;
    let mut py = y + 16;

    if let Ok(Some(mon)) = app.monitor_from_point(x as f64, y as f64) {
        let mpos = mon.position();
        let msize = mon.size();
        let max_x = mpos.x + msize.width as i32 - w - 8;
        let max_y = mpos.y + msize.height as i32 - h - 8;
        if px > max_x {
            px = x - w - 16;
        }
        if py > max_y {
            py = y - h - 22;
        }
        px = px.max(mpos.x + 8);
        py = py.max(mpos.y + 8);
    }

    let _ = overlay.set_position(tauri::PhysicalPosition::new(px, py));
    let _ = overlay.show();
    crate::meeting::protect_shown(&overlay);

    // Sample the screen right where the overlay now sits and tell it whether
    // to render as light-glass-on-dark-text or dark-glass-on-light-text -
    // whichever stays readable against whatever's actually behind it (a dark
    // code editor vs. a bright document, a light desktop wallpaper, etc.),
    // mirroring the browser extension's adaptive-theme.js.
    //
    // The sample is a real screen capture, so it runs on the blocking pool -
    // doing it inline stalled the async runtime (and with it the browser
    // bridge) for the length of a capture on every call.
    const LIGHT_THEME_THRESHOLD: f32 = 0.58;
    if !sample_theme {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(lum) = crate::capture::average_luminance_at(px, py, w as u32, h as u32) {
            let _ = app.emit_to("overlay", "surface-theme", lum >= LIGHT_THEME_THRESHOLD);
        }
    });
}

/// After the overlay changes size, pull it back onto the monitor it is on. The
/// pill is placed for a 64px-tall window; expanding to 400-700px from the lower
/// half of the screen used to push the input row (the LAST thing in the panel)
/// below the screen edge - "I can't see the text box".
pub fn clamp_overlay_into_monitor(app: &AppHandle) {
    let Some(overlay) = app.get_webview_window("overlay") else { return };
    let (Ok(pos), Ok(size)) = (overlay.outer_position(), overlay.outer_size()) else { return };
    let (w, h) = (size.width as i32, size.height as i32);
    let probe = (pos.x + 24, pos.y + 24);
    let monitor = app
        .monitor_from_point(probe.0 as f64, probe.1 as f64)
        .ok()
        .flatten()
        .or_else(|| overlay.current_monitor().ok().flatten());
    let Some(mon) = monitor else { return };
    let mpos = mon.position();
    let msize = mon.size();
    // Leave room for a bottom taskbar (Tauri's monitor size is the full monitor).
    let taskbar = (48.0 * mon.scale_factor()) as i32;
    let max_x = mpos.x + msize.width as i32 - w - 8;
    let max_y = mpos.y + msize.height as i32 - h - taskbar;
    let nx = pos.x.min(max_x).max(mpos.x + 8);
    let ny = pos.y.min(max_y).max(mpos.y + 8);
    if nx != pos.x || ny != pos.y {
        let _ = overlay.set_position(tauri::PhysicalPosition::new(nx, ny));
    }
}

/// Show the overlay near wherever the cursor currently is.
pub fn show_overlay_near_cursor(app: &AppHandle) {
    let (x, y) = mouse::get_cursor_position();
    show_overlay_near(app, x, y);
}
