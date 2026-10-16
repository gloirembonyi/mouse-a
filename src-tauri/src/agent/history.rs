// Kudu Desktop agent - conversation slimming and compaction.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// ── Context management ───────────────────────────────────────────
/// Screenshots kept in history (they're the main context bloat here).
pub(crate) const KEEP_IMAGES: usize = 2;

/// Tool rounds whose results stay untrimmed.
pub(crate) const KEEP_FULL_TOOL_ROUNDS: usize = 3;

/// Older tool results are cut to this many characters.
pub(crate) const SLIM_TOOL_RESULT_CHARS: usize = 1500;

/// Tool rounds kept verbatim when older ones are folded into a summary…
pub(crate) const COMPACT_KEEP_ROUNDS: usize = 8;

/// …which happens once this many rounds pile up…
pub(crate) const COMPACT_AT_ROUNDS: usize = 18;

/// …or the non-image history text gets this large.
pub(crate) const COMPACT_AT_CHARS: usize = 120_000;

pub(crate) fn is_tool_round(content: &Value) -> bool {
    content.get("role").and_then(|r| r.as_str()) == Some("user")
        && content
            .get("parts")
            .and_then(|p| p.as_array())
            .is_some_and(|parts| parts.iter().any(|p| p.get("functionResponse").is_some()))
}

pub(crate) fn tool_round_indices(contents: &[Value]) -> Vec<usize> {
    contents.iter().enumerate().filter(|(_, c)| is_tool_round(c)).map(|(i, _)| i).collect()
}

/// Size of the history without images (those are bounded separately).
pub(crate) fn history_text_chars(contents: &[Value]) -> usize {
    contents
        .iter()
        .filter_map(|c| c.get("parts").and_then(|p| p.as_array()))
        .flatten()
        .filter(|p| p.get("inlineData").is_none())
        .map(|p| p.to_string().len())
        .sum()
}

/// Adds a text note to the conversation as the user, merging into the last
/// user turn when there is one so roles keep alternating.
pub(crate) fn push_user_text(contents: &mut Vec<Value>, text: &str) {
    if let Some(last) = contents.last_mut() {
        if last.get("role").and_then(|r| r.as_str()) == Some("user") {
            if let Some(parts) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                parts.push(json!({ "text": text }));
                return;
            }
        }
    }
    contents.push(json!({ "role": "user", "parts": [{ "text": text }] }));
}

/// Only the newest round carries a run_status; older copies are noise.
pub(crate) fn strip_run_status(contents: &mut [Value]) {
    for c in contents.iter_mut() {
        if let Some(parts) = c.get_mut("parts").and_then(|p| p.as_array_mut()) {
            for p in parts.iter_mut() {
                if let Some(resp) = p.pointer_mut("/functionResponse/response").and_then(|r| r.as_object_mut()) {
                    resp.remove("run_status");
                }
            }
        }
    }
}

/// Trim tool results older than the last `keep_full` rounds to a preview -
/// a 20k-char file read from 10 steps ago doesn't need to ride along in full.
pub(crate) fn slim_old_tool_results(contents: &mut [Value], keep_full: usize) {
    let rounds = tool_round_indices(contents);
    let cutoff = rounds.len().saturating_sub(keep_full);
    for &idx in rounds.iter().take(cutoff) {
        let Some(parts) = contents[idx].get_mut("parts").and_then(|p| p.as_array_mut()) else { continue };
        for p in parts.iter_mut() {
            let Some(resp) = p.pointer_mut("/functionResponse/response") else { continue };
            if let Some(obj) = resp.as_object_mut() {
                obj.remove("verification");
                obj.remove("run_status");
                if obj.contains_key("trimmed") {
                    continue;
                }
            }
            let serialized = resp.to_string();
            if serialized.len() <= SLIM_TOOL_RESULT_CHARS {
                continue;
            }
            let slim = match resp.get("error") {
                Some(err) => json!({
                    "error": clip(&err.as_str().map(|s| s.to_string()).unwrap_or_else(|| err.to_string()), 600),
                    "trimmed": true
                }),
                None => json!({
                    "result_preview": clip(&response_text(resp), SLIM_TOOL_RESULT_CHARS),
                    "trimmed": "older result shortened to save space - re-run the tool if you need it in full"
                }),
            };
            *resp = slim;
        }
    }
}

pub(crate) const COMPACTION_MARKER: &str = "[EARLIER WORK - compacted";

/// Fold every tool round except the most recent ones into one summary. Rounds
/// are removed as (model call → tool response) pairs from the front, so the
/// rest stays a valid function-calling transcript; the summary rides on the
/// first user turn (the task), so roles still alternate. Returns true if it
/// compacted.
pub(crate) fn compact_history(contents: &mut Vec<Value>, digest: &str, force: bool) -> bool {
    let rounds = tool_round_indices(contents);
    let keep = if force { (COMPACT_KEEP_ROUNDS / 2).max(2) } else { COMPACT_KEEP_ROUNDS };
    if rounds.len() <= keep {
        return false;
    }
    if !force && rounds.len() < COMPACT_AT_ROUNDS && history_text_chars(contents) < COMPACT_AT_CHARS {
        return false;
    }
    // Cut at the model turn that issued the first round we keep.
    let mut cut = rounds[rounds.len() - keep] - 1;
    while cut > 1 && contents[cut].get("role").and_then(|r| r.as_str()) != Some("model") {
        cut -= 1;
    }
    if cut <= 1 || contents[cut].get("role").and_then(|r| r.as_str()) != Some("model") {
        return false;
    }
    contents.drain(1..cut);
    let summary = format!(
        "{} to save space. This is the authoritative record of what already happened; do not redo \
completed steps.]\n{}",
        COMPACTION_MARKER, digest
    );
    if let Some(parts) = contents[0].get_mut("parts").and_then(|p| p.as_array_mut()) {
        parts.retain(|p| !p.get("text").and_then(|t| t.as_str()).is_some_and(|t| t.starts_with(COMPACTION_MARKER)));
        parts.push(json!({ "text": summary }));
    }
    true
}
