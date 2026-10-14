// Mouse AI desktop agent - the run ledger (plan, log, facts) and run_status.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

pub(crate) const LEDGER_LOG_KEEP: usize = 40;

pub(crate) const LEDGER_FACTS_KEEP: usize = 12;

pub(crate) const LEDGER_NOTES_MAX: usize = 3000;

pub(crate) struct LogEntry {
    pub(crate) n: usize,
    pub(crate) what: String,
    pub(crate) ok: bool,
    pub(crate) note: Option<String>,
}

#[derive(Default)]
pub(crate) struct Ledger {
    pub(crate) plan: Vec<String>,
    pub(crate) done_when: String,
    pub(crate) plan_done: usize,
    pub(crate) notes: String,
    pub(crate) log: std::collections::VecDeque<LogEntry>,
    pub(crate) older_steps: usize,
    pub(crate) facts: std::collections::VecDeque<String>,
    /// Model steps taken so far.
    pub(crate) steps: usize,
    /// Real tool calls (update_plan excluded).
    pub(crate) actions: usize,
    /// Successful state-changing tool calls - the evidence a "done" claim needs.
    pub(crate) effects: usize,
    pub(crate) errors: usize,
}

impl Ledger {
    /// Applies an `update_plan` call. Returns true if it marked more steps done.
    pub(crate) fn apply_plan_update(&mut self, args: &Value) -> bool {
        let before = self.plan_done;
        if let Some(items) = args.get("plan").and_then(|p| p.as_array()) {
            let plan: Vec<String> = items
                .iter()
                .filter_map(|p| p.as_str())
                .map(|p| clip(p.trim(), 200))
                .filter(|p| !p.is_empty())
                .take(20)
                .collect();
            if !plan.is_empty() {
                self.plan = plan;
            }
        }
        if let Some(d) = args.get("done_when").and_then(|d| d.as_str()) {
            if !d.trim().is_empty() {
                self.done_when = clip(d.trim(), 300);
            }
        }
        if let Some(c) = args.get("completed").and_then(|c| c.as_f64()) {
            if c.is_finite() {
                self.plan_done = c.max(0.0).floor() as usize;
            }
        }
        self.plan_done = self.plan_done.min(self.plan.len());
        if let Some(n) = args.get("notes").and_then(|n| n.as_str()) {
            // Notes accumulate (newest last) - models send deltas more often
            // than totals. When over the cap, the OLDEST notes go first.
            let add = n.trim();
            if !add.is_empty() && !self.notes.contains(add) {
                if !self.notes.is_empty() {
                    self.notes.push('\n');
                }
                self.notes.push_str(add);
                let len = self.notes.chars().count();
                if len > LEDGER_NOTES_MAX {
                    self.notes = self.notes.chars().skip(len - LEDGER_NOTES_MAX).collect();
                }
            }
        }
        self.plan_done > before
    }

    pub(crate) fn plan_lines(&self) -> Vec<String> {
        self.plan
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mark = if i < self.plan_done { "✓" } else if i == self.plan_done { "→" } else { "·" };
                format!("{} {}. {}", mark, i + 1, clip(p, 140))
            })
            .collect()
    }

    /// One-line plan for the UI, e.g. "Plan: ✓ Read config → Fix bug · Run tests".
    pub(crate) fn plan_summary_line(&self) -> String {
        let items: Vec<String> = self
            .plan
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mark = if i < self.plan_done { "✓" } else if i == self.plan_done { "→" } else { "·" };
                format!("{} {}", mark, clip(p, 48))
            })
            .collect();
        format!("Plan: {}", items.join(" "))
    }

    pub(crate) fn record(&mut self, what: &str, ok: bool, note: Option<String>) {
        self.log.push_back(LogEntry { n: self.steps, what: clip(what, 90), ok, note });
        while self.log.len() > LEDGER_LOG_KEEP {
            self.log.pop_front();
            self.older_steps += 1;
        }
    }

    pub(crate) fn record_fact(&mut self, text: String) {
        self.facts.push_back(clip(&text, 500));
        while self.facts.len() > LEDGER_FACTS_KEEP {
            self.facts.pop_front();
        }
    }

    /// Human/model-readable digest of the run so far.
    pub(crate) fn digest(&self, max_log: usize) -> String {
        let mut out: Vec<String> = Vec::new();
        if !self.plan.is_empty() {
            out.push(format!("Plan:\n{}", self.plan_lines().join("\n")));
        }
        if !self.done_when.is_empty() {
            out.push(format!("Done when: {}", self.done_when));
        }
        if !self.notes.is_empty() {
            out.push(format!("Notes: {}", clip(&self.notes, 1200)));
        }
        let skip = self.log.len().saturating_sub(max_log);
        let skipped = self.older_steps + skip;
        let recent: Vec<String> = self
            .log
            .iter()
            .skip(skip)
            .map(|e| {
                format!(
                    "{}. {}{}{}",
                    e.n,
                    e.what,
                    if e.ok { "" } else { " - FAILED" },
                    e.note.as_ref().map(|n| format!(" ({})", n)).unwrap_or_default()
                )
            })
            .collect();
        if !recent.is_empty() {
            let head = if skipped > 0 { format!("Steps taken ({} earlier steps not listed):", skipped) } else { "Steps taken:".to_string() };
            out.push(format!("{}\n{}", head, recent.join("\n")));
        }
        if !self.facts.is_empty() {
            let facts: Vec<String> = self.facts.iter().map(|f| format!("- {}", f)).collect();
            out.push(format!("Data collected:\n{}", facts.join("\n")));
        }
        out.join("\n\n")
    }
}

/// The run_status attached to the last result of each tool round: the model
/// sees where the run stands at the moment it decides the next move.
pub(crate) fn run_status(ledger: &Ledger, budget: usize, can_extend: bool, project: Option<&str>, warning: Option<String>) -> Value {
    let mut status = serde_json::Map::new();
    status.insert(
        "step".into(),
        json!(format!(
            "{} of {}{}",
            ledger.steps,
            budget,
            if can_extend { " (extends while you make progress)" } else { " (final budget)" }
        )),
    );
    if ledger.plan.is_empty() {
        status.insert("plan".into(), json!("no plan recorded - call update_plan if this takes more than a few steps"));
    } else {
        status.insert("plan".into(), json!(ledger.plan_lines()));
    }
    if !ledger.done_when.is_empty() {
        status.insert("done_when".into(), json!(ledger.done_when));
    }
    if !ledger.notes.is_empty() {
        status.insert("notes".into(), json!(clip(&ledger.notes, 400)));
    }
    if let Some(p) = project {
        status.insert("project_folder".into(), json!(p));
    }
    if let Some(w) = warning {
        status.insert("warning".into(), json!(w));
    }
    Value::Object(status)
}

/// Fallback report when the model can't write one itself.
pub(crate) fn ledger_report(ledger: &Ledger, reason: &str) -> String {
    let digest = ledger.digest(12);
    let mut text = format!("I stopped before finishing - {}.", reason);
    if !digest.is_empty() {
        text.push_str("\n\n");
        text.push_str(&digest);
    }
    text.push_str(CONTINUE_HINT);
    text
}

pub(crate) const CONTINUE_HINT: &str = "\n\n_(Say \"continue\" to pick up from here.)_";
