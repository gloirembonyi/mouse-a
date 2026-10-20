// Kudu Desktop - Markdown -> what a person would type into another app.
//
// The model writes Markdown. A chat box, an email body or a form field does not render
// it, so "### **Phase 1**" used to land in the user's apps verbatim. Text the agent types
// (type_text) or the copilot bar inserts goes through `for_app()` first: plain text with
// headings on their own lines, "• " bullets, no ** / __ / # / backticks. Code editors,
// terminals, Markdown-native sites and code itself are left untouched.
//
// A port of the extension's shared/text-format.js (looksLikeMarkdown / blocks /
// toPlain) and of frontend/src/lib/textFormat.ts - keep the three behaving the same.
// JavaScript's lookahead (used for *italic* / _italic_) has no equivalent in the regex
// crate, so those two rules are a small hand-written scanner with the same semantics.

use regex::Regex;
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("valid regex"))
}

macro_rules! rx {
    ($pat:expr) => {{
        static CELL: OnceLock<Regex> = OnceLock::new();
        re(&CELL, $pat)
    }};
}

/// True when the text carries Markdown syntax worth converting.
pub fn looks_like_markdown(s: &str) -> bool {
    if s.chars().count() < 3 {
        return false;
    }
    let signs = [
        rx!(r"(?m)^\s{0,3}#{1,6}\s+\S"),
        rx!(r"\*\*[^*\n]+\*\*"),
        rx!(r"__[^_\n]+__"),
        rx!(r"(?m)^\s*[-*+]\s+\S"),
        rx!(r"(?m)^\s*\d+[.)]\s+\S"),
        rx!(r"\[[^\]\n]+\]\(https?:[^)\s]+\)"),
        rx!(r"`[^`\n]+`"),
        rx!(r"(?m)^\s*```"),
        rx!(r"(?m)^\s*>\s+\S"),
        rx!(r"(?m)^\s*(?:-{3,}|\*{3,}|_{3,})\s*$"),
        rx!(r"(?m)^\s*\|.+\|\s*$"),
    ];
    let hits = signs.iter().filter(|r| r.is_match(s)).count();
    hits >= 2 || rx!(r"(?m)\*\*[^*\n]+\*\*|^\s{0,3}#{1,6}\s|\[[^\]\n]+\]\(https?:|^\s*```").is_match(s)
}

/// `(^|[\s(])M([^M\s][^M\n]*?)M(?=[\s).,!?:;]|$)` -> `$1$2`, as JS's global replace does.
fn strip_emphasis(s: &str, m: char) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let follow_ok = |i: usize| i >= n || chars[i].is_whitespace() || ").,!?:;".contains(chars[i]);
    // Try a match whose opening marker is at `open`; returns (content end, closing index).
    let try_at = |open: usize| -> Option<usize> {
        if open >= n || chars[open] != m {
            return None;
        }
        let first = *chars.get(open + 1)?;
        if first == m || first.is_whitespace() {
            return None;
        }
        let mut j = open + 2;
        while j < n && chars[j] != m && chars[j] != '\n' {
            j += 1;
        }
        (j < n && chars[j] == m && follow_ok(j + 1)).then_some(j)
    };
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        // "^" alternative: only at the very start of the string.
        if i == 0 {
            if let Some(close) = try_at(0) {
                out.extend(&chars[1..close]);
                i = close + 1;
                continue;
            }
        }
        let c = chars[i];
        if c.is_whitespace() || c == '(' {
            if let Some(close) = try_at(i + 1) {
                out.push(c);
                out.extend(&chars[i + 2..close]);
                i = close + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

pub fn inline_plain(s: &str) -> String {
    let s = rx!(r"!\[([^\]]*)\]\([^)]*\)").replace_all(s, "${1}");
    let s = rx!(r"\[([^\]]+)\]\((https?:[^)\s]+)\)").replace_all(&s, |c: &regex::Captures| {
        let (t, u) = (&c[1], &c[2]);
        if t.trim() == u.trim() {
            u.to_string()
        } else {
            format!("{} ({})", t, u)
        }
    });
    let s = rx!(r"`([^`]+)`").replace_all(&s, "${1}");
    let s = rx!(r"\*\*\*([^*]+)\*\*\*").replace_all(&s, "${1}");
    let s = rx!(r"\*\*([^*]+)\*\*").replace_all(&s, "${1}");
    let s = rx!(r"__([^_]+)__").replace_all(&s, "${1}");
    let s = strip_emphasis(&s, '*');
    let s = strip_emphasis(&s, '_');
    rx!(r"~~([^~]+)~~").replace_all(&s, "${1}").into_owned()
}

#[derive(Debug, PartialEq)]
pub enum Block {
    Code(String),
    Blank,
    Heading(String),
    Rule,
    Table(Vec<Vec<String>>),
    List { ordered: bool, items: Vec<(usize, String)> },
    Quote(String),
    Para(Vec<String>),
}

/// Parse Markdown into the same simple blocks as the extension's `blocks()`.
pub fn blocks(md: &str) -> Vec<Block> {
    let norm = md.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = norm.split('\n').collect();
    let fence = rx!(r"^\s*```");
    let heading = rx!(r"^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$");
    let rule = rx!(r"^\s*(?:-{3,}|\*{3,}|_{3,})\s*$");
    let row = rx!(r"^\s*\|.+\|\s*$");
    let sep_cell = rx!(r"^:?-{2,}:?$");
    let item = rx!(r"^(\s*)([-*+]|\d+[.)])\s+(.*)$");
    let quote = rx!(r"^\s*>\s?(.*)$");
    let para_stop = rx!(r"^\s*(```|#{1,6}\s|[-*+]\s|\d+[.)]\s|>|\|)");
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if fence.is_match(line) {
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !fence.is_match(lines[i]) {
                body.push(lines[i]);
                i += 1;
            }
            i += 1;
            out.push(Block::Code(body.join("\n")));
            continue;
        }
        if line.trim().is_empty() {
            out.push(Block::Blank);
            i += 1;
            continue;
        }
        if let Some(c) = heading.captures(line) {
            out.push(Block::Heading(c[2].to_string()));
            i += 1;
            continue;
        }
        if rule.is_match(line) {
            out.push(Block::Rule);
            i += 1;
            continue;
        }
        if row.is_match(line) {
            let mut rows = Vec::new();
            while i < lines.len() && row.is_match(lines[i]) {
                let t = lines[i].trim();
                let t = t.strip_prefix('|').unwrap_or(t);
                let t = t.strip_suffix('|').unwrap_or(t);
                let cells: Vec<String> = t.split('|').map(|c| c.trim().to_string()).collect();
                if !cells.iter().all(|c| sep_cell.is_match(c)) {
                    rows.push(cells);
                }
                i += 1;
            }
            out.push(Block::Table(rows));
            continue;
        }
        if let Some(c) = item.captures(line) {
            let ordered = c[2].chars().any(|ch| ch.is_ascii_digit());
            let mut items = Vec::new();
            while i < lines.len() {
                let Some(c) = item.captures(lines[i]) else { break };
                if c[2].chars().any(|ch| ch.is_ascii_digit()) != ordered {
                    break;
                }
                let indent = c[1].replace('\t', "    ").chars().count();
                items.push(((indent / 2).min(3), c[3].to_string()));
                i += 1;
            }
            out.push(Block::List { ordered, items });
            continue;
        }
        if quote.is_match(line) {
            let mut body = Vec::new();
            while i < lines.len() {
                let Some(c) = quote.captures(lines[i]) else { break };
                body.push(c[1].to_string());
                i += 1;
            }
            out.push(Block::Quote(body.join("\n")));
            continue;
        }
        let mut para = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() && !para_stop.is_match(lines[i]) && !rule.is_match(lines[i]) {
            para.push(lines[i].to_string());
            i += 1;
        }
        if para.is_empty() {
            para.push(lines[i].to_string());
            i += 1;
        }
        out.push(Block::Para(para));
    }
    out
}

/// Clean plain text: what a person would type into a plain field.
pub fn to_plain(md: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let gap = |parts: &mut Vec<String>| {
        if parts.last().is_some_and(|l| !l.is_empty()) {
            parts.push(String::new());
        }
    };
    for b in blocks(md) {
        match b {
            Block::Blank | Block::Rule => gap(&mut parts),
            Block::Heading(t) => parts.push(inline_plain(&t)),
            Block::Code(t) => parts.push(t),
            Block::Quote(t) => parts.extend(t.split('\n').map(|l| {
                let q = format!("“{}”", inline_plain(l));
                if q == "“”" { String::new() } else { q }
            })),
            Block::List { ordered: false, items } => {
                parts.extend(items.iter().map(|(d, t)| format!("{}• {}", "  ".repeat(*d), inline_plain(t))))
            }
            Block::List { ordered: true, items } => {
                let mut n = 0;
                for (d, t) in items {
                    let marker = if d > 0 {
                        "-".to_string()
                    } else {
                        n += 1;
                        format!("{}.", n)
                    };
                    parts.push(format!("{}{} {}", "  ".repeat(d), marker, inline_plain(&t)));
                }
            }
            Block::Table(rows) => parts.extend(
                rows.iter().map(|r| r.iter().map(|c| inline_plain(c)).collect::<Vec<_>>().join(" | ")),
            ),
            Block::Para(lines) => parts.extend(lines.iter().map(|l| inline_plain(l))),
        }
    }
    let joined = parts.join("\n");
    rx!(r"\n{3,}").replace_all(&joined, "\n\n").trim().to_string()
}

/// Window titles of sites/apps whose text boxes ARE Markdown (mirrors the extension's
/// MARKDOWN_HOSTS, matched on the browser tab title the desktop can see).
fn title_speaks_markdown(title: &str) -> bool {
    let t = title.to_lowercase();
    ["github", "gitlab", "bitbucket", "discord", "stack overflow", "stack exchange", "super user", "server fault", "ask ubuntu", "dev community", "hackmd", "obsidian", "linear"]
        .iter()
        .any(|k| t.contains(k))
}

/// The text to type into the app `process_kind` (`classify_process`) / `window_title`:
/// Markdown becomes plain text, except in code editors, terminals, Markdown-native
/// sites, or when the text is code.
pub fn for_app(text: &str, process_kind: &str, window_title: &str) -> Option<String> {
    if matches!(process_kind, "editor" | "terminal") || title_speaks_markdown(window_title) {
        return None;
    }
    if !looks_like_markdown(text) {
        return None;
    }
    let plain = to_plain(text);
    (plain != text).then_some(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_matches_the_extension() {
        // Expected output produced by the extension's GhostTextFormat.toPlain.
        let md = "# Hi\n\n**Bold** _it_ [l](https://a.b)\n- x\n  - y\n1) one\n\n> q\n>\n| a | b |\n|--|--|\n|1|2|\n***\n```\ncode **x**\n```";
        assert_eq!(to_plain(md), "Hi\n\nBold it l (https://a.b)\n• x\n  • y\n1. one\n\n“q”\n\na | b\n1 | 2\n\ncode **x**");
    }

    #[test]
    fn emphasis_follows_js_rules() {
        assert_eq!(inline_plain("*a* and (*b*), x*y*z, *c*d"), "a and (b), x*y*z, *c*d");
        assert_eq!(inline_plain("_one_ two_three_"), "one two_three_");
        assert_eq!(inline_plain("### **Phase 1**"), "### Phase 1");
    }

    #[test]
    fn only_converts_markdown_for_plain_targets() {
        assert_eq!(for_app("### **Phase 1**\nDo it", "browser", "Slack").as_deref(), Some("Phase 1\nDo it"));
        assert!(for_app("### **Phase 1**", "editor", "x").is_none());
        assert!(for_app("### **Phase 1**", "browser", "Pull request · GitHub").is_none());
        assert!(for_app("Hello there, see you at 5.", "other", "").is_none());
        assert!(!looks_like_markdown("plain 1. text"));
    }
}
