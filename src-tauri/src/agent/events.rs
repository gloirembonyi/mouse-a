// Kudu Desktop agent - progress events to the UI and the phase roll-up.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

pub(crate) fn emit(app: &AppHandle, source: &str, kind: &str, message: impl Into<String>) {
    let _ = app.emit(
        "agent-progress",
        AgentProgress {
            source: source.to_string(),
            kind: kind.to_string(),
            message: redact_secrets(&message.into()),
            request_id: None,
        },
    );
}

/// Mask tokens that look like API keys or access tokens (a script the agent ran once
/// printed a Gemini key into the step log, which is saved to history).
pub(crate) fn redact_secrets(text: &str) -> String {
    const PREFIXES: &[(&str, usize)] = &[
        ("AIza", 35), ("sk-ant-", 30), ("sk-", 24), ("ghp_", 30), ("gho_", 30), ("github_pat_", 40),
        ("xoxb-", 20), ("xoxp-", 20), ("xoxa-", 20), ("AKIA", 20), ("ya29.", 40), ("Bearer ", 0),
    ];
    let is_tok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/' | '+' | '=');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let run_len = rest.chars().take_while(|c| is_tok(*c)).map(|c| c.len_utf8()).sum::<usize>();
        if run_len == 0 {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
            continue;
        }
        let tok = &rest[..run_len];
        let secret = PREFIXES.iter().any(|(p, min)| *min > 0 && tok.starts_with(p) && tok.len() >= *min && !tok.contains("://"));
        if secret {
            out.push_str(&tok[..tok.len().min(6)]);
            out.push_str("…[redacted]");
        } else {
            out.push_str(tok);
        }
        rest = &rest[run_len..];
    }
    out
}

#[derive(Default, Clone)]
pub(crate) struct PhaseCounts {
    pub(crate) files_touched: u32,
    pub(crate) searches: u32,
    pub(crate) edited: u32,
    pub(crate) commands: u32,
    pub(crate) actions: u32,
    pub(crate) lines_added: u32,
    pub(crate) lines_removed: u32,
    pub(crate) has_diff_stats: bool,
}

impl PhaseCounts {
    pub(crate) fn is_empty(&self) -> bool {
        self.files_touched == 0 && self.searches == 0 && self.edited == 0 && self.commands == 0 && self.actions == 0
    }

    pub(crate) fn record(&mut self, tool_name: &str) {
        match tool_name {
            "read_file" | "list_directory" | "find_files" | "get_screen_context" | "capture_screen"
            | "list_windows" | "focus_window" | "web_fetch" | "read_app_content" => self.files_touched += 1,
            "web_search" | "search_in_files" => self.searches += 1,
            "write_file" | "edit_file" | "delete_file" | "download_file" => self.edited += 1,
            "run_command" => self.commands += 1,
            _ => self.actions += 1,
        }
    }

    pub(crate) fn record_diff(&mut self, added: u32, removed: u32) {
        self.lines_added += added;
        self.lines_removed += removed;
        self.has_diff_stats = true;
    }

    /// e.g. "Edited 11 files, explored 7 files, 3 searches, ran 2 commands +367 -14"
    pub(crate) fn summarize(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let plural = |n: u32| if n == 1 { "" } else { "s" };
        let mut parts: Vec<String> = Vec::new();
        if self.edited > 0 {
            parts.push(format!("edited {} file{}", self.edited, plural(self.edited)));
        }
        if self.files_touched > 0 {
            parts.push(format!("explored {} file{}", self.files_touched, plural(self.files_touched)));
        }
        if self.searches > 0 {
            parts.push(format!("{} search{}", self.searches, if self.searches == 1 { "" } else { "es" }));
        }
        if self.commands > 0 {
            parts.push(format!("ran {} command{}", self.commands, plural(self.commands)));
        }
        if self.actions > 0 {
            parts.push(format!("{} other action{}", self.actions, plural(self.actions)));
        }
        let mut line = parts.join(", ");
        if let Some(first) = line.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        if self.has_diff_stats {
            line.push_str(&format!(" +{} -{}", self.lines_added, self.lines_removed));
        }
        Some(line)
    }
}

/// Emits the accumulated tool-call tally as one "phase-summary" event (if
/// anything happened) and resets the counter for the next phase.
pub(crate) fn flush_phase(app: &AppHandle, source: &str, phase: &mut PhaseCounts) {
    if let Some(summary) = phase.summarize() {
        emit(app, source, "phase-summary", summary);
    }
    *phase = PhaseCounts::default();
}

/// write_file/delete_file smuggle their diff stats through `ToolOutcome.log`
/// as a trailing `\u{1}DIFF <added> <removed>` marker (rather than adding a
/// field to `ToolOutcome`, which ~35 call sites construct as a plain struct
/// literal) - stripped back out here before the log line reaches the model
/// or the UI, neither of which should ever see the raw marker.
pub(crate) fn extract_diff_marker(log: &str) -> (String, Option<(u32, u32)>) {
    let Some(idx) = log.find('\u{1}') else {
        return (log.to_string(), None);
    };
    let (base, marker) = log.split_at(idx);
    let base = base.to_string();
    let Some(rest) = marker[1..].strip_prefix("DIFF ") else {
        return (base, None);
    };
    let mut nums = rest.split_whitespace();
    match (
        nums.next().and_then(|a| a.parse::<u32>().ok()),
        nums.next().and_then(|r| r.parse::<u32>().ok()),
    ) {
        (Some(added), Some(removed)) => (base, Some((added, removed))),
        _ => (base, None),
    }
}
