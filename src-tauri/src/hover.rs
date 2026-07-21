// Mouse AI Desktop — Global Hover-Dwell Watcher
// While AI mode is active, watches the cursor system-wide. When the mouse
// pauses over something (dwell), it shows the floating "Ask AI" overlay next
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

                // Cursor left the overlay area — hide the pill again.
                let away = (x - overlay_anchor.0)
                    .abs()
                    .max((y - overlay_anchor.1).abs());
                if away > HIDE_DISTANCE {
                    if let Some(overlay) = app.get_webview_window("overlay") {
                        let _ = overlay.hide();
                    }
                }
                continue;
            }

            if dwell_handled || still_since.elapsed().as_millis() < DWELL_MS {
                continue;
            }
            dwell_handled = true;

            let ctx = mouse::get_cursor_context();

            // Never trigger while hovering Mouse AI's own windows.
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

            show_overlay_near(&app, x, y);
            let _ = app.emit_to(
                "overlay",
                "hover-dwell",
                HoverPayload {
                    x,
                    y,
                    context: ctx,
                    element,
                },
            );
        }
    });
}

/// Position the overlay window next to a screen point (clamped to the
/// monitor that point is on) and show it without stealing focus.
pub fn show_overlay_near(app: &AppHandle, x: i32, y: i32) {
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    let (w, h) = overlay
        .outer_size()
        .map(|s| (s.width as i32, s.height as i32))
        .unwrap_or((380, 64));

    let mut px = x + 16;
    let mut py = y + 22;

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
}

/// Show the overlay near wherever the cursor currently is.
pub fn show_overlay_near_cursor(app: &AppHandle) {
    let (x, y) = mouse::get_cursor_position();
    show_overlay_near(app, x, y);
}
