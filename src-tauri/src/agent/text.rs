// Kudu Desktop agent - small string helpers.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

pub(crate) const MAX_TOOL_OUTPUT: usize = 6000;

pub(crate) const MAX_FILE_READ: usize = 20_000;

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let kept: String = s.chars().take(n).collect();
        format!("{}\n…[truncated, {} chars total]", kept, s.chars().count())
    } else {
        s.to_string()
    }
}

// ── Phase rollups (Cursor-style "Explored 12 files, 3 searches +367 -14") ──
//
// The raw step-by-step tool log (see `.agent-step` in the UI) is accurate but
// unreadable for a long run. Between each narration ("thinking") message the
// model sends, this tallies what its tool calls actually did, so a burst of
// 20 read_file/list_directory calls collapses into one line instead of 20.

/// Short clip with a plain ellipsis (unlike `truncate`, which annotates the
/// total length - right for tool output, noisy for one-line labels).
pub(crate) fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let kept: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", kept.trim_end())
    } else {
        s.to_string()
    }
}

/// Lowercased, punctuation-free, space-padded text for whole-word phrase checks.
pub(crate) fn normalize_words(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push(' ');
    let mut last_space = true;
    for ch in s.chars() {
        let c = if ch == '’' { '\'' } else { ch };
        if c.is_alphanumeric() || c == '\'' {
            out.extend(c.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    if !last_space {
        out.push(' ');
    }
    out
}

pub(crate) fn has_any_phrase(normalized: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| normalized.contains(&format!(" {} ", p)))
}

pub(crate) fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Keep the start and (mostly) the end of long output. Command output reads
/// top-down, but the verdict - the error, the failing test, the exit code - is
/// at the bottom.
pub(crate) fn clip_middle(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let head_n = max / 4;
    let tail_n = max - head_n;
    let head: String = s.chars().take(head_n).collect();
    let tail: String = s.chars().skip(total - tail_n).collect();
    format!("{}\n…[{} chars omitted from the middle]…\n{}", head, total - head_n - tail_n, tail)
}
