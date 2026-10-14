// Mouse AI Desktop - the "working" banner and the yield-to-user rule.
//
// Two halves of one idea: while the agent drives the mouse and keyboard, the user must
// (1) see it, unmistakably, with Stop in reach, and (2) never have to fight it. The
// banner is a slim always-on-top pill at the top of the active monitor (shield.html),
// excluded from screen capture so the agent's own screenshots never show it. The yield
// rule lives in `wait_for_user_idle`: before every UI action the agent checks whether
// the person has touched the mouse or keyboard in the last moment (GetLastInputInfo vs.
// the agent's own last SendInput) and, if so, waits until they stop - saying so in the
// banner - instead of clicking into whatever they were doing. Blocking the user's input
// outright (BlockInput) needs administrator rights and would also lock them out; this
// does not.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Milliseconds (GetTickCount domain) of the agent's own most recent SendInput.
pub static OUR_LAST_INPUT_TICK: AtomicU64 = AtomicU64::new(0);
/// Runs currently driving the screen. The banner (and the Esc hotkey) stay up
/// until the last one ends - two runs at once must not hide each other's banner.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

// ── Esc stops everything ──────────────────────────────────────────
// While the agent drives the mouse, a Stop *button* is hard to reach - the
// pointer is being moved out from under you. Esc is registered as a global
// hotkey only for the length of a run, and ignored when the keystroke was the
// agent's own (press_key "escape" lands within a few ms of our SendInput).
const OWN_KEY_WINDOW_MS: u64 = 400;

fn apply_esc_hotkey(app: &AppHandle, on: bool) {
    use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};
    static LOCK: Mutex<()> = Mutex::new(());
    let _serial = LOCK.lock().unwrap();
    let gs = app.global_shortcut();
    let sc = Shortcut::new(None, Code::Escape);
    let registered = gs.is_registered(sc);
    if on && !registered {
        let res = gs.on_shortcut(sc, move |app, _sc, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let ours = OUR_LAST_INPUT_TICK.load(Ordering::Relaxed);
            if tick_now().saturating_sub(ours) < OWN_KEY_WINDOW_MS {
                return; // the agent pressed Esc, not the person
            }
            let n = crate::agent::request_cancel_all();
            if n > 0 {
                let _ = app.emit_to("shield", "shield-stopping", n);
            }
        });
        if let Err(e) = res {
            eprintln!("[shield] could not register Esc: {}", e);
        }
    } else if !on && registered {
        let _ = gs.unregister(sc);
    }
}

#[cfg(windows)]
fn tick_now() -> u64 {
    unsafe { winapi::um::sysinfoapi::GetTickCount64() }
}
#[cfg(not(windows))]
fn tick_now() -> u64 {
    0
}

/// Tick of the last input event the system saw - from anyone, including the agent.
#[cfg(windows)]
fn last_input_tick() -> u64 {
    use winapi::um::winuser::{GetLastInputInfo, LASTINPUTINFO};
    unsafe {
        let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut info) != 0 {
            // dwTime is a 32-bit tick; compare in the same wrap-around domain.
            let now = tick_now();
            let now32 = (now & 0xFFFF_FFFF) as u32;
            let delta = now32.wrapping_sub(info.dwTime) as u64;
            now.saturating_sub(delta)
        } else {
            0
        }
    }
}
#[cfg(not(windows))]
fn last_input_tick() -> u64 {
    0
}

/// Milliseconds since the USER (not the agent) last touched mouse or keyboard, or
/// None when the last input was the agent's own.
pub fn user_idle_ms() -> Option<u64> {
    let last = last_input_tick();
    if last == 0 {
        return Some(u64::MAX);
    }
    let ours = OUR_LAST_INPUT_TICK.load(Ordering::Relaxed);
    // The agent's SendInput registers as input too; anything within ~250 ms of our
    // own send is ours (queueing jitter), anything later is the person.
    if last <= ours.saturating_add(250) {
        return None;
    }
    Some(tick_now().saturating_sub(last))
}

/// Wait until the person has been idle for `quiet_ms` (bounded by `max_wait`). Emits
/// one progress line the first time it has to wait. Returns true if it waited.
pub async fn wait_for_user_idle(app: &AppHandle, source: &str, quiet_ms: u64, max_wait: Duration) -> bool {
    let started = std::time::Instant::now();
    let mut announced = false;
    loop {
        match user_idle_ms() {
            None => return announced,
            Some(idle) if idle >= quiet_ms => return announced,
            _ => {}
        }
        if started.elapsed() >= max_wait {
            return announced;
        }
        if !announced {
            announced = true;
            crate::agent::emit(app, source, "thinking", "Waiting - you are using the mouse or keyboard; I'll continue when you stop.");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Show the banner at the top-centre of the monitor under the cursor.
pub fn show(app: &AppHandle, source: &str, task: &str) {
    let Some(w) = app.get_webview_window("shield") else { return };
    let (cx, cy) = crate::mouse::get_cursor_position();
    let monitor = app
        .monitor_from_point(cx as f64, cy as f64)
        .ok()
        .flatten()
        .or_else(|| w.primary_monitor().ok().flatten());
    if let Some(mon) = monitor {
        let width = w.outer_size().map(|s| s.width as i32).unwrap_or(460);
        let x = mon.position().x + (mon.size().width as i32 - width) / 2;
        let y = mon.position().y + (8.0 * mon.scale_factor()) as i32;
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = w.set_always_on_top(true);
    let _ = w.show();
    // Never steal focus from the app the agent is driving.
    let _ = crate::mouse::set_own_window_no_activate("mouse ai working", true);
    if ACTIVE.fetch_add(1, Ordering::SeqCst) == 0 {
        let app2 = app.clone();
        std::thread::spawn(move || apply_esc_hotkey(&app2, true));
    }
    let _ = app.emit_to("shield", "shield-show", serde_json::json!({ "source": source, "task": task.chars().take(80).collect::<String>() }));
}

pub fn hide(app: &AppHandle) {
    let prev = ACTIVE.load(Ordering::SeqCst);
    if prev == 0 {
        return;
    }
    if ACTIVE.fetch_sub(1, Ordering::SeqCst) != 1 {
        return; // another run is still driving the screen
    }
    if let Some(w) = app.get_webview_window("shield") {
        let _ = w.hide();
    }
    let app2 = app.clone();
    std::thread::spawn(move || apply_esc_hotkey(&app2, false));
}
