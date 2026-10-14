// Mouse AI desktop agent - model-call resilience.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// ── Model-call resilience ────────────────────────────────────────
// Gemini occasionally returns a transient "model overloaded / high demand"
// error (a 502/503 with that text), the per-route limiter returns a 429, or
// the network hiccups. None of those means the task failed - it means "ask
// again in a moment" - so the step is retried with backoff instead of losing
// all progress. A 413 means the conversation itself got too big: compact it
// hard and retry immediately.
pub(crate) const STEP_RETRY_ATTEMPTS: usize = 4;

pub(crate) const STEP_RETRY_BASE_DELAY_SECS: u64 = 3;

pub(crate) const STEP_RETRY_MAX_DELAY_SECS: u64 = 30;

/// HTTP status out of a backend error shaped like `"Agent step failed (503)…"`.
pub(crate) fn step_error_status(err: &str) -> Option<u16> {
    let start = err.find("failed (")? + "failed (".len();
    let rest = &err[start..];
    let end = rest.find(')')?;
    rest[..end].trim().parse::<u16>().ok()
}

/// A quota/billing 429 ("You exceeded your current quota…") is NOT a transient
/// rate limit - retrying it just delays an inevitable failure.
pub(crate) fn is_quota_exhausted(err: &str) -> bool {
    let lower = err.to_lowercase();
    ["exceeded your current quota", "check your plan and billing", "quota_exceeded", "resource_exhausted"]
        .iter()
        .any(|needle| lower.contains(needle))
}

pub(crate) fn is_payload_too_large(err: &str) -> bool {
    if step_error_status(err) == Some(413) {
        return true;
    }
    let lower = err.to_lowercase();
    lower.contains("payload too large") || lower.contains("request entity too large") || lower.contains("body exceeded")
}

pub(crate) fn is_retryable_step_error(err: &str) -> bool {
    if is_quota_exhausted(err) {
        return false;
    }
    let lower = err.to_lowercase();
    if lower.contains("session expired") || lower.contains("not signed in") {
        return false;
    }
    const TRANSIENT_WORDING: [&str; 4] = ["high demand", "overloaded", "unavailable", "please try again later"];
    if let Some(status) = step_error_status(err) {
        if status == 408 || status == 429 || status >= 500 || status == 413 {
            return true;
        }
        // Other 4xx are request problems; only overload wording is worth a retry.
        return TRANSIENT_WORDING.iter().any(|n| lower.contains(n));
    }
    // No status: a network-level failure (see backend.rs friendly_network_error)
    // or a response body cut off mid-flight.
    TRANSIENT_WORDING.iter().any(|n| lower.contains(n))
        || [
            "network error",
            "reach the thothai backend",
            "econnreset",
            "connection",
            "timed out",
            "timeout",
            "too many requests",
            "rate limit",
            "error sending request",
            "failed to parse agent step response",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Pulls a server-suggested delay out of an error string shaped like
/// `"...[retry-after=7]..."` (see `backend.rs::agent_step`), so a rate-limited
/// step waits exactly as long as the server asked instead of guessing.
pub(crate) fn retry_after_hint_secs(err: &str) -> Option<u64> {
    let start = err.find("[retry-after=")? + "[retry-after=".len();
    let rest = &err[start..];
    let end = rest.find(']')?;
    rest[..end].trim().parse::<u64>().ok()
}

/// Calls the model, riding out transient failures (backoff, honoring
/// retry-after) and compacting the history once if it's too large to send.
/// Returns early (with an error) if the user presses Stop during a wait.
pub(crate) async fn agent_step_with_retry(
    app: &AppHandle,
    backend: &BackendClient,
    ctx: &RunContext,
    contents: &mut Vec<Value>,
    source: &str,
    ledger: &Ledger,
    thinking: Option<u32>,
) -> Result<Value, String> {
    let mut attempt = 0usize;
    let mut compacted_for_size = false;
    loop {
        let req = crate::backend::AgentStepRequest {
            system: &ctx.system,
            contents: Value::Array(contents.clone()),
            tools: ctx.tools.clone(),
            thinking_budget: thinking,
            profile: Some("desktop"),
            prompt_context: Some(ctx.prompt_context()),
        };
        match backend.agent_step_with_profile(req).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if is_payload_too_large(&e) && !compacted_for_size {
                    compacted_for_size = true;
                    emit(app, source, "thinking", "The conversation got too large - compacting older steps and retrying…");
                    prune_old_images(contents, 1);
                    slim_old_tool_results(contents, 1);
                    compact_history(contents, &ledger.digest(30), true);
                    continue;
                }
                if attempt >= STEP_RETRY_ATTEMPTS || !is_retryable_step_error(&e) {
                    return Err(e);
                }
                attempt += 1;
                let backoff = STEP_RETRY_BASE_DELAY_SECS * (1 << (attempt - 1)); // 3s, 6s, 12s, 24s
                let delay = retry_after_hint_secs(&e)
                    .map(|hint| hint.clamp(1, STEP_RETRY_MAX_DELAY_SECS))
                    .unwrap_or(backoff);
                let lower = e.to_lowercase();
                let reason = if step_error_status(&e) == Some(429)
                    || lower.contains("rate limit")
                    || lower.contains("too many requests")
                {
                    "Too many requests right now"
                } else if step_error_status(&e).is_none() {
                    "Connection problem"
                } else {
                    "The model is busy"
                };
                emit(
                    app,
                    source,
                    "thinking",
                    format!("{} ({}). Retrying in {}s… ({}/{})", reason, truncate(&e, 120), delay, attempt, STEP_RETRY_ATTEMPTS),
                );
                // Sleep in slices so Stop doesn't have to wait out a 24s backoff.
                let until = std::time::Instant::now() + Duration::from_secs(delay);
                while std::time::Instant::now() < until {
                    if cancel_pending(source) {
                        return Err("Stopped".to_string());
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    }
}

// ── Run ledger ───────────────────────────────────────────────────
// The run's memory outside the conversation: plan, definition of done, a
// compact step log, notes and key data. It feeds the run_status every tool
// round carries, the summary that replaces old rounds on compaction, the
// completion check, and the report written when a run has to stop.
