// Kudu Desktop - one login across the desktop app and the browser extension.
//
// Over the paired localhost bridge (bridge.rs), whichever client is signed in
// with a real account shares it with the other one - but never by sending its
// session token. It mints a one-time, 60-second code on the backend
// (POST /api/auth/handoff) and sends only that; the other side redeems it
// (POST /api/auth/handoff/redeem) for a session of its OWN, so each device can
// still be signed out independently.
//
// Frames (only acted on from a connection that presented the pairing code):
//   auth_request            "I'm signed out / a guest - share your login if you have one"
//   auth_offer {code,email} a one-time code for the sender's account
//
// An offer is only ever redeemed by a client that has no real account, so a
// signed-in account is never silently replaced by another one.

use crate::bridge::bridge;
use crate::commands::{AppState, AuthState};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};

static REDEEMING: AtomicBool = AtomicBool::new(false);

/// The extension just paired: if this app has no real account, ask for one.
pub fn on_paired(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if !state.backend.has_account().await {
            bridge().send_json(json!({ "v": 1, "type": "auth_request" }));
        }
    });
}

/// The extension asked for a login: share ours if we have a real one.
pub fn handle_request(app: &AppHandle, paired: bool) {
    if paired {
        offer(app);
    }
}

/// Push our login to a paired extension (on request, or right after the user
/// signs in here). No-op without a real account.
pub fn offer(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if !bridge().is_paired() {
            return;
        }
        let state = app.state::<AppState>();
        if !state.backend.has_account().await {
            return;
        }
        match state.backend.create_handoff().await {
            Ok(code) => {
                let email = state.backend.get_current_user().await.and_then(|u| u.email).unwrap_or_default();
                bridge().send_json(json!({ "v": 1, "type": "auth_offer", "code": code, "email": email }));
            }
            Err(e) => eprintln!("[auth-sync] couldn't create handoff: {}", e),
        }
    });
}

/// The extension shared its login: adopt it if we're only a guest.
pub fn handle_offer(app: &AppHandle, v: &Value, paired: bool) {
    if !paired {
        return;
    }
    let Some(code) = v.get("code").and_then(|c| c.as_str()).map(str::to_string) else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if state.backend.has_account().await || REDEEMING.swap(true, Ordering::SeqCst) {
            return;
        }
        let result = state.backend.redeem_handoff(&code).await;
        REDEEMING.store(false, Ordering::SeqCst);
        match result {
            Ok(resp) if resp.success => {
                // A different account: the guest's open chat isn't theirs.
                *state.conversation_id.write().await = None;
                let _ = app.emit("auth-changed", AuthState { signed_in: true, user: resp.user });
            }
            Ok(_) => {}
            Err(e) => eprintln!("[auth-sync] redeem failed: {}", e),
        }
    });
}
