// Kudu Desktop - files handed over by the browser agent ("kudu-file" blocks).
//
// The extension's agent writes documents into its private sandbox workspace, which
// nobody can open. It now appends every file it wrote to its final answer as a fenced
// block (shared contract with Kudu Connect - do not change it):
//
//     ```kudu-file
//     {"name":"x.md","path":"x.md","mime":"text/markdown","bytes":5123,"b64":"<base64 UTF-8>"}
//     ```
//
// `b64` is omitted (and `"truncated":true` set) for files over 256 KB.
//
// Here, a `delegate_to_browser` result is split three ways:
//   - each decoded file is saved for real (active project folder, else Documents\Kudu),
//   - the model sees a one-line placeholder with the saved path (never base64),
//   - the raw blocks are kept per run and appended to the run's final answer, so the
//     chat renders them as file cards (Preview / Download / Open).
// The frontend side of the contract is `frontend/src/lib/kuduFiles.ts`.

use base64::Engine;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Deserialize, Debug, Clone, Default)]
pub struct KuduFile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub bytes: u64,
    /// Absent (with `"truncated": true`) for files over 256 KB; `mime` is not needed here.
    #[serde(default)]
    pub b64: Option<String>,
}

/// One fenced block found in a text: its byte range (whole fence, including the
/// trailing newline when present) and its parsed payload (None = malformed JSON).
#[derive(Debug)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    pub file: Option<KuduFile>,
}

/// Every closed ```kudu-file fence in `text`, in order.
pub fn find_blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    // (byte offset of line start, line without its newline, offset after the line)
    let mut lines = Vec::new();
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let next = pos + line.len();
        lines.push((pos, line.trim_end_matches(['\n', '\r']), next));
        pos = next;
    }
    let mut i = 0;
    while i < lines.len() {
        let (start, line, _) = lines[i];
        let trimmed = line.trim_start();
        let ticks = trimmed.chars().take_while(|c| *c == '`').count();
        if ticks >= 3 && trimmed[ticks..].trim() == "kudu-file" {
            let mut body = String::new();
            let mut closed = None;
            for (j, (_, l, end)) in lines.iter().enumerate().skip(i + 1) {
                let t = l.trim();
                if t.len() >= ticks && t.chars().all(|c| c == '`') {
                    closed = Some((j, *end));
                    break;
                }
                body.push_str(l.trim());
            }
            if let Some((j, end)) = closed {
                let file = serde_json::from_str::<KuduFile>(&body)
                    .ok()
                    .filter(|f| !f.name.trim().is_empty() || !f.path.trim().is_empty());
                out.push(Block { start, end, file });
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// A file name safe to create on any OS: last path segment, no reserved chars.
pub fn safe_file_name(name: &str, fallback_path: &str) -> String {
    let raw = if name.trim().is_empty() { fallback_path } else { name };
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let cleaned: String = last
        .chars()
        .map(|c| if "<>:\"|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if cleaned.is_empty() {
        "kudu-file.txt".to_string()
    } else {
        cleaned.chars().take(150).collect()
    }
}

/// `dir/name`, or `dir/stem-1.ext`, `dir/stem-2.ext`… - never overwrites.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..10_000 {
        let p = dir.join(format!("{}-{}{}", stem, n, ext));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{}-{}{}", stem, uuid::Uuid::new_v4(), ext))
}

/// Documents\Kudu (falls back to ~/Documents/Kudu, then the temp dir).
pub fn kudu_documents_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Documents")))
        .unwrap_or_else(std::env::temp_dir)
        .join("Kudu")
}

fn decode_b64(b64: &str) -> Result<Vec<u8>, String> {
    let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(clean.as_bytes())
        .map_err(|e| format!("invalid base64: {}", e))
}

/// Save decoded bytes as `name` inside `dir` (created), never overwriting.
pub fn save_bytes(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {}", dir.display(), e))?;
    let path = unique_path(dir, &safe_file_name(name, ""));
    std::fs::write(&path, bytes).map_err(|e| format!("Could not write {}: {}", path.display(), e))?;
    Ok(path)
}

pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// The text with each kudu-file block replaced by `placeholder(file)`; malformed
/// blocks become a short note (their body is never passed on).
fn replace_blocks(text: &str, blocks: &[Block], mut placeholder: impl FnMut(Option<&KuduFile>) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for b in blocks {
        out.push_str(&text[last..b.start]);
        out.push_str(&placeholder(b.file.as_ref()));
        out.push('\n');
        last = b.end;
    }
    out.push_str(&text[last..]);
    out
}

/// For places that only show a short summary (the Agents board): blocks become
/// `[file: name, size]`, nothing is saved.
pub fn summarize(text: &str) -> String {
    let blocks = find_blocks(text);
    if blocks.is_empty() {
        return text.to_string();
    }
    replace_blocks(text, &blocks, |f| match f {
        Some(f) => format!("[file: {}, {}]", safe_file_name(&f.name, &f.path), human_size(f.bytes)),
        None => "[file: unreadable attachment]".to_string(),
    })
}

// ── Per-run state: where to save, and the blocks to show with the final answer ──

struct RunFiles {
    project: Option<String>,
    blocks: Vec<String>,
}

fn runs() -> &'static Mutex<HashMap<String, RunFiles>> {
    static RUNS: OnceLock<Mutex<HashMap<String, RunFiles>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Called when an agent run starts: remembers its project folder (if any).
pub fn begin_run(source: &str, project: Option<String>) {
    runs()
        .lock()
        .unwrap()
        .insert(source.to_string(), RunFiles { project, blocks: Vec::new() });
}

/// Called when the run ends: the raw blocks to append to its final answer.
pub fn end_run(source: &str) -> Vec<String> {
    runs().lock().unwrap().remove(source).map(|r| r.blocks).unwrap_or_default()
}

/// Append the run's file blocks (those not already quoted) to its final answer.
pub fn attach_to_answer(answer: String, blocks: Vec<String>) -> String {
    let missing: Vec<String> = blocks.into_iter().filter(|b| !answer.contains(b.trim())).collect();
    if missing.is_empty() {
        return answer;
    }
    let mut out = answer.trim_end().to_string();
    for b in missing {
        out.push_str("\n\n");
        out.push_str(b.trim_end());
    }
    out.push('\n');
    out
}

/// A delegate_to_browser / check_browser_task result → the text the model sees.
/// Saves every decoded file, stashes the raw blocks for the chat, and replaces each
/// block with `[file: name, size - saved to <path>]`.
pub fn take_browser_result(source: &str, text: &str) -> String {
    let blocks = find_blocks(text);
    if blocks.is_empty() {
        return text.to_string();
    }
    let project = runs().lock().unwrap().get(source).and_then(|r| r.project.clone());
    let dir = project
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(kudu_documents_dir);
    let mut raw_blocks = Vec::new();
    let mut i = 0;
    let model_text = replace_blocks(text, &blocks, |f| {
        let b = &blocks[i];
        i += 1;
        let Some(f) = f else {
            return "[file: unreadable attachment from the browser - ignored]".to_string();
        };
        raw_blocks.push(text[b.start..b.end].to_string());
        let name = safe_file_name(&f.name, &f.path);
        let size = human_size(f.bytes);
        match f.b64.as_deref().filter(|s| !s.trim().is_empty()) {
            None => format!(
                "[file: {}, {} - too large to transfer; it stays in the browser's workspace]",
                name, size
            ),
            Some(b64) => match decode_b64(b64).and_then(|bytes| save_bytes(&dir, &name, &bytes)) {
                Ok(path) => format!("[file: {}, {} - saved to {}]", name, size, path.display()),
                Err(e) => format!("[file: {}, {} - could not be saved: {}]", name, size, e),
            },
        }
    });
    if let Some(run) = runs().lock().unwrap().get_mut(source) {
        run.blocks.extend(raw_blocks);
    }
    model_text
}

// ── UI commands (file cards in the chat) ──

/// Save a card's file into `dir` (or Documents\Kudu) without overwriting; returns the path.
#[tauri::command]
pub fn kudu_file_save(name: String, b64: String, dir: Option<String>) -> Result<String, String> {
    let bytes = decode_b64(&b64)?;
    let dir = dir
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(kudu_documents_dir);
    save_bytes(&dir, &name, &bytes).map(|p| p.to_string_lossy().to_string())
}

/// Write base64 bytes to an exact path the user picked in the save dialog.
#[tauri::command]
pub fn fs_write_base64(path: String, b64: String) -> Result<(), String> {
    let bytes = decode_b64(&b64)?;
    std::fs::write(&path, bytes).map_err(|e| format!("Could not write {}: {}", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
    }

    #[test]
    fn finds_and_replaces_blocks() {
        let text = format!(
            "I wrote it.\n\n```kudu-file\n{{\"name\":\"a.md\",\"path\":\"a.md\",\"mime\":\"text/markdown\",\"bytes\":5,\"b64\":\"{}\"}}\n```\nbye\n```kudu-file\nnot json\n```\n",
            b64("hello")
        );
        let blocks = find_blocks(&text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].file.as_ref().unwrap().name, "a.md");
        assert!(blocks[1].file.is_none());
        let s = summarize(&text);
        assert!(s.contains("[file: a.md, 5 B]"));
        assert!(!s.contains("b64"));
        assert!(s.contains("bye"));
    }

    #[test]
    fn unclosed_fence_is_left_alone() {
        assert!(find_blocks("```kudu-file\n{\"name\":\"a\"}\n").is_empty());
    }

    #[test]
    fn saves_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("kudu-files-test-{}", uuid::Uuid::new_v4()));
        let a = save_bytes(&dir, "x.md", b"1").unwrap();
        let b = save_bytes(&dir, "x.md", b"2").unwrap();
        assert_ne!(a, b);
        assert!(b.to_string_lossy().ends_with("x-1.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn browser_result_is_saved_and_stripped() {
        let dir = std::env::temp_dir().join(format!("kudu-files-run-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        begin_run("test:1", Some(dir.to_string_lossy().to_string()));
        let text = format!(
            "Done.\n```kudu-file\n{{\"name\":\"r.md\",\"bytes\":2,\"b64\":\"{}\"}}\n```\n",
            b64("hi")
        );
        let model = take_browser_result("test:1", &text);
        assert!(model.contains("saved to"));
        assert!(!model.contains(&b64("hi")));
        assert_eq!(std::fs::read_to_string(dir.join("r.md")).unwrap(), "hi");
        let blocks = end_run("test:1");
        assert_eq!(blocks.len(), 1);
        let answer = attach_to_answer("All done.".into(), blocks);
        assert!(answer.contains("```kudu-file"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_file_name("../a/b:c.md", ""), "b_c.md");
        assert_eq!(safe_file_name("", "dir/x.csv"), "x.csv");
        assert_eq!(safe_file_name("..", ""), "kudu-file.txt");
    }
}
