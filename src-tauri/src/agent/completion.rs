// Mouse AI desktop agent - the completion check: a 'done' claim held to evidence.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

pub(crate) const SUCCESS_WORDS: &[&str] = &[
    "done", "complete", "completed", "finished", "success", "successful", "successfully", "sent", "posted",
    "published", "added", "saved", "submitted", "created", "updated", "filled", "booked", "ordered", "scheduled",
    "replied", "uploaded", "all set", "installed", "fixed", "edited", "wrote", "written", "moved", "renamed",
    "deleted", "removed", "downloaded", "configured", "deployed", "committed", "resolved", "is now", "are now",
];

pub(crate) const FAILURE_WORDS: &[&str] = &[
    "couldn't", "could not", "couldnt", "unable", "failed", "cannot", "can't", "cant", "blocked", "not able",
    "wasn't able", "was not able", "didn't work", "did not work", "impossible", "no way to", "wasn't possible",
];

pub(crate) const ACTION_WORDS: &[&str] = &[
    "send", "post", "reply", "respond", "fill", "submit", "add", "create", "book", "order", "buy", "click",
    "type", "write", "upload", "delete", "remove", "sign up", "register", "apply", "schedule", "subscribe",
    "publish", "save", "update", "change", "set up", "setup", "enable", "disable", "install", "uninstall",
    "move", "rename", "edit", "fix", "run", "build", "start", "launch", "download", "copy", "commit", "deploy",
    "configure", "implement", "refactor", "make", "generate", "open", "close", "organize", "clean", "convert",
];

/// Decide whether a final answer should be accepted or the run sent back to
/// work. Returns an empty list to accept, or the concrete concerns.
pub(crate) fn completion_concerns(text: &str, ledger: &Ledger, task: &str, last_round_failed: bool) -> Vec<String> {
    let mut concerns = Vec::new();
    // The verdict is in the opening of an answer ("I've fixed…", "I couldn't…");
    // scanning the whole thing would match words inside quoted output.
    let head: String = text.chars().take(400).collect();
    let head = normalize_words(&head);
    let admits_failure = has_any_phrase(&head, FAILURE_WORDS);
    let claims_success = has_any_phrase(&head, SUCCESS_WORDS) && !admits_failure;

    if admits_failure {
        if ledger.steps < 15 && ledger.errors > 0 {
            concerns.push(
                "You're giving up fairly early. Before reporting failure, make sure you tried a genuinely \
different route: a different command or tool, checking the path/prerequisites, the terminal instead of the \
GUI, a direct URL, or web_search for how it's normally done."
                    .to_string(),
            );
        }
        return concerns;
    }
    if last_round_failed {
        concerns.push(
            "Your most recent tool call FAILED (error or non-zero exit code) and nothing since shows it was \
fixed - the last thing you tried did not work."
                .to_string(),
        );
    }
    if claims_success {
        let task_words = normalize_words(strip_project_prefix(task));
        if has_any_phrase(&task_words, ACTION_WORDS) && ledger.effects == 0 {
            concerns.push(
                "You're reporting the task as done, but no state-changing action (write/edit a file, a \
successful command, a click/typing, a browser task…) has succeeded in this run. If the task genuinely needed \
no change, say so explicitly; otherwise do the work."
                    .to_string(),
            );
        }
        if !ledger.plan.is_empty() && ledger.plan_done < ledger.plan.len() {
            let left: Vec<String> =
                ledger.plan.iter().skip(ledger.plan_done).map(|p| format!("\"{}\"", clip(p, 80))).collect();
            concerns.push(format!(
                "Your own plan still has {} unfinished step(s): {}. If they're actually done, verify and call \
update_plan with completed={}.",
                ledger.plan.len() - ledger.plan_done,
                left.join(", "),
                ledger.plan.len()
            ));
        }
    }
    concerns
}

// ── Progress & loop detection ────────────────────────────────────
