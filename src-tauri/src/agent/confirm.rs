// Kudu Desktop agent - human confirmations and the per-app allow/deny gate.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// ── Confirmation gate ────────────────────────────────────────────
// The agent can call `request_confirmation` before a sensitive/ambiguous
// action; this pauses that tool call on a channel until the UI reports back
// which button the user clicked (Allow/Deny), via the `agent_confirm` command.
pub(crate) static PENDING_CONFIRMATIONS: OnceLock<Mutex<HashMap<String, oneshot::Sender<bool>>>> = OnceLock::new();

pub(crate) fn confirmations() -> &'static Mutex<HashMap<String, oneshot::Sender<bool>>> {
    PENDING_CONFIRMATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Called by the `agent_confirm` Tauri command when the user clicks a button.
/// Returns false if the request already timed out or doesn't exist.
pub fn resolve_confirmation(request_id: &str, approved: bool) -> bool {
    if let Some(tx) = confirmations().lock().unwrap().remove(request_id) {
        tx.send(approved).is_ok()
    } else {
        false
    }
}

/// Shows the user a real Allow/Deny button and waits for their click (with a
/// 5-minute timeout, treated as a deny so the task doesn't hang forever).
/// Shared by the model-invoked `request_confirmation` tool and the automatic
/// per-app gate in `guard_target_app`.
pub(crate) async fn confirm(app: &AppHandle, source: &str, message: &str) -> bool {
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    confirmations().lock().unwrap().insert(request_id.clone(), tx);

    let _ = app.emit(
        "agent-progress",
        AgentProgress {
            source: source.to_string(),
            kind: "confirm".to_string(),
            message: message.to_string(),
            request_id: Some(request_id.clone()),
        },
    );

    // Wait for Allow/Deny, but keep listening for Stop: a prompt nobody answers must
    // not pin the run for five minutes after the user already cancelled it.
    const CONFIRM_TIMEOUT: Duration = Duration::from_secs(300);
    let started = std::time::Instant::now();
    let mut rx = rx;
    loop {
        tokio::select! {
            res = &mut rx => {
                return matches!(res, Ok(true));
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if cancel_pending(source) || started.elapsed() >= CONFIRM_TIMEOUT {
                    confirmations().lock().unwrap().remove(&request_id);
                    return false;
                }
            }
        }
    }
}

pub(crate) async fn request_confirmation_tool(app: &AppHandle, source: &str, args: &Value) -> ToolOutcome {
    let message = args
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("Proceed with this action?")
        .to_string();
    if confirm(app, source, &message).await {
        ToolOutcome::text("The user clicked Allow. Proceed.")
    } else {
        ToolOutcome::text(
            "The user clicked Deny (or didn't respond within 5 minutes). Do not proceed with this \
action - ask what they'd like instead, or stop.",
        )
    }
}

/// Automatic safety net, independent of the model's own judgment: the first
/// time in a run that an input-affecting tool (click/double_click/scroll/
/// type_text/press_key) is about to act on a given process, this pauses and
/// shows the user a real Allow/Deny prompt - exactly like `request_confirmation`,
/// but the model doesn't have to remember to call it, and can't skip it either.
/// Approval is remembered for the rest of THIS run only (not persisted), so a
/// single task touching one app in a loop only interrupts once.
pub(crate) async fn guard_target_app(
    app: &AppHandle,
    source: &str,
    trusted: &mut HashSet<String>,
    process_name: &str,
    window_title: &str,
) -> Result<(), String> {
    let key = process_name.trim().to_lowercase();
    if key.is_empty() || trusted.contains(&key) {
        return Ok(());
    }
    let label = if window_title.trim().is_empty() {
        process_name.to_string()
    } else {
        format!("{} ({})", window_title, process_name)
    };
    let message = format!("First action on \"{}\" this task - let Kudu control it?", label);
    if confirm(app, source, &message).await {
        trusted.insert(key);
        Ok(())
    } else {
        Err(format!(
            "Denied: the user has not approved Kudu controlling \"{}\" for this task.",
            process_name
        ))
    }
}
