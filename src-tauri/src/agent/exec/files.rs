// Mouse AI desktop agent - file tools.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

/// Approximate line-level diff stats (added, removed) between old and new file
/// content - write_file always overwrites the whole file rather than patching
/// it, so this is computed after the fact via a longest-common-subsequence
/// pass, same idea as `git diff --numstat`. Returns None for pathologically
/// large files rather than spend seconds on an O(n·m) pass no one asked for.
pub(crate) fn line_diff_stats(old: &str, new: &str) -> Option<(u32, u32)> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines.len().saturating_mul(new_lines.len()) > 4_000_000 {
        return None;
    }
    // Classic O(n·m) LCS length via a rolling two-row DP.
    let mut prev = vec![0u32; new_lines.len() + 1];
    let mut curr = vec![0u32; new_lines.len() + 1];
    for i in 1..=old_lines.len() {
        for j in 1..=new_lines.len() {
            curr[j] = if old_lines[i - 1] == new_lines[j - 1] {
                prev[j - 1] + 1
            } else {
                prev[j].max(curr[j - 1])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    let lcs_len = prev[new_lines.len()];
    let removed = old_lines.len() as u32 - lcs_len;
    let added = new_lines.len() as u32 - lcs_len;
    Some((added, removed))
}

/// Decode file bytes the way Windows tools actually write them: UTF-8 (with or
/// without BOM) or UTF-16 LE/BE (PowerShell 5.1's `>` and Out-File default) -
/// read_to_string used to fail on the latter with "stream did not contain valid UTF-8".
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    let utf16 = |le: bool, b: &[u8]| {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return utf16(true, &bytes[2..]);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return utf16(false, &bytes[2..]);
    }
    // BOM-less UTF-16LE: every other byte zero in the opening stretch.
    let probe = &bytes[..bytes.len().min(400)];
    if probe.len() >= 4 && probe.iter().skip(1).step_by(2).all(|b| *b == 0) {
        return utf16(true, bytes);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

pub(crate) fn read_file_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    if raw.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    let path = resolve_agent_path(raw);
    let shown = path.display().to_string();
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();

    // Office documents and spreadsheets: their text, not a UTF-8 error.
    let text = if ["docx", "docm", "pptx", "xlsx", "xlsm", "xls", "ods", "csv"].contains(&ext.as_str()) && ext != "csv" {
        match crate::documents::extract_text(&shown) {
            Ok(t) => t,
            Err(e) => return ToolOutcome::err(format!("could not read {}: {}", shown, e)),
        }
    } else {
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => return ToolOutcome::err(format!("{} is a folder - use list_directory.", shown)),
            Ok(m) if m.len() > 20 * 1024 * 1024 => {
                return ToolOutcome::err(format!("{} is {} MB - too large to read whole; use search_in_files or a command (Select-String / Get-Content -TotalCount).", shown, m.len() / (1024 * 1024)))
            }
            Err(e) => return ToolOutcome::err(format!("could not read {}: {}", shown, e)),
            _ => {}
        }
        match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.iter().take(8192).filter(|b| **b == 0).count() > 0 && !looks_utf16(&bytes) {
                    return ToolOutcome::err(format!("{} is a binary file (no text to read).", shown));
                }
                decode_text(&bytes)
            }
            Err(e) => return ToolOutcome::err(format!("could not read {}: {}", shown, e)),
        }
    };

    // Paging by line, so a long file can be read in parts instead of being cut at 20K chars.
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len();
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let limit = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);
    let paged = offset > 0 || limit.is_some();
    let body = if paged {
        let end = limit.map(|l| (offset + l).min(total_lines)).unwrap_or(total_lines);
        lines.get(offset.min(total_lines)..end).map(|s| s.join("\n")).unwrap_or_default()
    } else {
        text.clone()
    };
    let truncated = body.chars().count() > MAX_FILE_READ;
    let mut response = json!({
        "path": shown,
        "content": truncate(&body, MAX_FILE_READ),
        "total_lines": total_lines,
    });
    if paged {
        response["start_line"] = json!(offset + 1);
    }
    if truncated {
        response["note"] = json!("Content was cut. Read the rest with offset/limit (line numbers).");
    }
    ToolOutcome {
        response,
        image_data_url: None,
        log: format!("Read {} ({} lines)", shown, total_lines),
    }
}

pub(crate) fn looks_utf16(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])
        || (bytes.len() >= 4 && bytes[..bytes.len().min(400)].iter().skip(1).step_by(2).all(|b| *b == 0))
}

pub(crate) fn write_file_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let append = args.get("append").and_then(|a| a.as_bool()).unwrap_or(false);
    if raw.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    let path = resolve_agent_path(raw);
    // Captured before the write so the activity log's "+N -M" stat reflects
    // what actually changed - None (file didn't exist yet) means every line
    // of the new content counts as added.
    let old_content = std::fs::read(&path).ok().map(|b| decode_text(&b));
    let existed = old_content.is_some();
    let mut created_dir = None;
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolOutcome::err(format!("could not create folder {}: {}", parent.display(), e));
            }
            created_dir = Some(parent.display().to_string());
        }
    }
    let result = if append {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(content.as_bytes()))
    } else {
        std::fs::write(&path, content)
    };
    match result {
        Ok(()) => {
            // Verify on disk rather than trusting the call: the model reports what
            // THIS says, so it must describe the file that is really there.
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let scratch = path.starts_with(agent_scratch_dir());
            let action = if append { "Appended to" } else if existed { "Overwrote" } else { "Created" };
            let mut msg = format!("{} {} - now {} bytes on disk.", action, path.display(), size);
            if let Some(dir) = &created_dir {
                msg.push_str(&format!(" (Created the folder {}.)", dir));
            }
            if scratch {
                msg.push_str(" This is your private scratch folder (deleted after the run) - write to an absolute path the user can see if they should keep it.");
            } else {
                msg.push_str(" Tell the user this exact path.");
            }
            let mut log = format!("{} {} ({} bytes)", action, path.display(), size);
            let new_content = if append { format!("{}{}", old_content.as_deref().unwrap_or(""), content) } else { content.to_string() };
            if let Some((added, removed)) = line_diff_stats(old_content.as_deref().unwrap_or(""), &new_content) {
                log.push_str(&format!("\u{1}DIFF {} {}", added, removed));
            }
            ToolOutcome {
                response: json!({ "result": msg, "path": path.display().to_string(), "bytes": size, "created": !existed }),
                image_data_url: None,
                log,
            }
        }
        Err(e) => ToolOutcome::err(format!("could not write {}: {}", path.display(), e)),
    }
}

/// Markdown → a real Word .docx at the resolved path (see skills::create_docx).
pub(crate) fn create_document_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("").trim();
    let markdown = args.get("markdown").and_then(|m| m.as_str()).unwrap_or("");
    if raw.is_empty() || markdown.trim().is_empty() {
        return ToolOutcome::err("create_document needs a path and the markdown content");
    }
    let mut path = resolve_agent_path(raw);
    if path.extension().map(|e| !e.eq_ignore_ascii_case("docx")).unwrap_or(true) {
        path.set_extension("docx");
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return ToolOutcome::err(format!("could not create folder {}: {}", parent.display(), e));
        }
    }
    let bytes = match crate::skills::create_docx(markdown) {
        Ok(b) => b,
        Err(e) => return ToolOutcome::err(format!("could not build the document: {}", e)),
    };
    let existed = path.exists();
    match std::fs::write(&path, &bytes) {
        Ok(()) => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let msg = format!(
                "{} Word document {} ({} bytes on disk). Tell the user this exact path.",
                if existed { "Overwrote" } else { "Created" },
                path.display(),
                size
            );
            ToolOutcome {
                response: json!({ "result": msg, "path": path.display().to_string(), "bytes": size }),
                image_data_url: None,
                log: format!("Created {} ({} bytes)", path.display(), size),
            }
        }
        Err(e) => ToolOutcome::err(format!("could not write {}: {} (is it open in Word?)", path.display(), e)),
    }
}

pub(crate) fn delete_file_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    if raw.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    let path = resolve_agent_path(raw);
    // Same "captured before it's gone" idea as write_file_tool, so a delete
    // shows up as "-N" lines removed instead of silently not counting.
    let old_content = std::fs::read_to_string(&path).ok();
    match std::fs::remove_file(&path) {
        Ok(()) => {
            let msg = format!("Deleted {}.", path.display());
            let mut log = msg.clone();
            if let Some(old) = old_content {
                if let Some((_, removed)) = line_diff_stats(&old, "") {
                    log.push_str(&format!("\u{1}DIFF 0 {}", removed));
                }
            }
            ToolOutcome { response: json!({ "result": msg }), image_data_url: None, log }
        }
        Err(e) => ToolOutcome::err(format!("could not delete {}: {}", path.display(), e)),
    }
}

pub(crate) fn list_directory_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    if raw.is_empty() {
        return ToolOutcome::err("no path provided");
    }
    let path = resolve_agent_path(raw);
    match std::fs::read_dir(&path) {
        Ok(rd) => {
            let mut items: Vec<String> = rd
                .flatten()
                .map(|e| {
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    format!("{}{}", e.file_name().to_string_lossy(), if is_dir { "/" } else { "" })
                })
                .collect();
            items.sort();
            let count = items.len();
            ToolOutcome {
                response: json!({ "entries": items, "count": count }),
                image_data_url: None,
                log: format!("{} items in {}", count, path.display()),
            }
        }
        Err(e) => ToolOutcome::err(format!("could not list {}: {}", path.display(), e)),
    }
}

pub(crate) async fn find_files_tool(args: &Value) -> ToolOutcome {
    let needle = args
        .get("name_contains")
        .and_then(|q| q.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if needle.is_empty() {
        return ToolOutcome::err("name_contains is required");
    }
    let root = args
        .get("root")
        .and_then(|r| r.as_str())
        .filter(|r| !r.trim().is_empty())
        .map(|r| r.to_string())
        .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_else(|| "C:\\".to_string());

    let result = tokio::task::spawn_blocking(move || find_files(&root, &needle)).await;
    match result {
        Ok(matches) => {
            let count = matches.len();
            ToolOutcome {
                response: json!({ "matches": matches, "count": count }),
                image_data_url: None,
                log: format!("{} match(es)", count),
            }
        }
        Err(e) => ToolOutcome::err(format!("search task failed: {}", e)),
    }
}

// Bounded recursive filename search: caps the number of hits and the number of
// directories visited so a search under a huge tree can't run away or hang the
// agent. Skips a few noisy/huge system dirs by name.
pub(crate) fn find_files(root: &str, needle: &str) -> Vec<String> {
    const MAX_MATCHES: usize = 60;
    const MAX_VISITED: usize = 40_000;
    let skip = ["node_modules", "$recycle.bin", "windows", "appdata\\local\\temp", ".git", "target"];

    let mut matches = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    let mut visited = 0usize;

    while let Some(dir) = stack.pop() {
        if matches.len() >= MAX_MATCHES || visited >= MAX_VISITED {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            visited += 1;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_lowercase().contains(needle) {
                matches.push(entry.path().to_string_lossy().to_string());
                if matches.len() >= MAX_MATCHES {
                    break;
                }
            }
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let lower = entry.path().to_string_lossy().to_lowercase();
                if !skip.iter().any(|s| lower.contains(s)) {
                    stack.push(entry.path());
                }
            }
        }
    }
    matches
}

pub(crate) fn edit_file_tool(args: &Value) -> ToolOutcome {
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let old_text = args.get("old_text").and_then(|p| p.as_str()).unwrap_or("");
    let new_text = args.get("new_text").and_then(|p| p.as_str()).unwrap_or("");
    let replace_all = args.get("replace_all").and_then(|p| p.as_bool()).unwrap_or(false);
    if raw.is_empty() || old_text.is_empty() {
        return ToolOutcome::err("edit_file needs 'path' and a non-empty 'old_text'");
    }
    let path = resolve_agent_path(raw);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return ToolOutcome::err(format!("could not read {}: {}", path.display(), e)),
    };
    // Files saved on Windows often use CRLF while the model writes LF.
    let (old_norm, new_norm) = if content.contains("\r\n") && !old_text.contains("\r\n") {
        (old_text.replace('\n', "\r\n"), new_text.replace('\n', "\r\n"))
    } else {
        (old_text.to_string(), new_text.to_string())
    };
    let count = content.matches(&old_norm).count();
    if count == 0 {
        return ToolOutcome::err(format!(
            "old_text was not found in {} - read_file it again and copy the snippet exactly (whitespace matters).",
            path.display()
        ));
    }
    if count > 1 && !replace_all {
        return ToolOutcome::err(format!(
            "old_text appears {} times in {} - include more surrounding lines to make it unique, or set replace_all.",
            count,
            path.display()
        ));
    }
    let updated = if replace_all { content.replace(&old_norm, &new_norm) } else { content.replacen(&old_norm, &new_norm, 1) };
    match std::fs::write(&path, &updated) {
        Ok(()) => {
            let n = if replace_all { count } else { 1 };
            let msg = format!("Edited {} ({} replacement{}).", path.display(), n, if n == 1 { "" } else { "s" });
            let mut log = msg.clone();
            if let Some((added, removed)) = line_diff_stats(&content, &updated) {
                log.push_str(&format!("\u{1}DIFF {} {}", added, removed));
            }
            ToolOutcome { response: json!({ "result": msg }), image_data_url: None, log }
        }
        Err(e) => ToolOutcome::err(format!("could not write {}: {}", path.display(), e)),
    }
}

pub(crate) async fn search_in_files_tool(args: &Value) -> ToolOutcome {
    let folder = args.get("folder").and_then(|p| p.as_str()).unwrap_or("").trim().to_string();
    let query = args.get("query").and_then(|p| p.as_str()).unwrap_or("").trim().to_lowercase();
    let ext = args
        .get("extension")
        .and_then(|p| p.as_str())
        .map(|e| e.trim().trim_start_matches("*.").trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty());
    if folder.is_empty() || query.is_empty() {
        return ToolOutcome::err("search_in_files needs 'folder' and 'query'");
    }
    let root = resolve_agent_path(&folder);
    let result = tokio::task::spawn_blocking(move || {
        const SKIP: [&str; 8] = ["node_modules", ".git", "target", "dist", "build", ".next", "__pycache__", ".venv"];
        const MAX_HITS: usize = 80;
        const MAX_FILES: usize = 20_000;
        let mut hits = Vec::new();
        let mut visited = 0usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let path = entry.path();
                let Ok(ft) = entry.file_type() else { continue };
                let name = entry.file_name().to_string_lossy().to_string();
                if ft.is_dir() {
                    if !SKIP.contains(&name.as_str()) {
                        stack.push(path);
                    }
                    continue;
                }
                visited += 1;
                if visited > MAX_FILES || hits.len() >= MAX_HITS {
                    return (hits, true);
                }
                if let Some(want) = &ext {
                    if path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() != Some(want.as_str()) {
                        continue;
                    }
                }
                if entry.metadata().map(|m| m.len() > 1_500_000).unwrap_or(true) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue }; // binary / non-UTF-8
                for (i, line) in text.lines().enumerate() {
                    if line.to_lowercase().contains(&query) {
                        let shown: String = line.trim().chars().take(200).collect();
                        hits.push(format!("{}:{}: {}", path.display(), i + 1, shown));
                        if hits.len() >= MAX_HITS {
                            break;
                        }
                    }
                }
            }
        }
        (hits, false)
    })
    .await;
    match result {
        Ok((hits, capped)) if hits.is_empty() => ToolOutcome::text(if capped {
            "No matches in the files scanned - narrow the folder or add an extension filter."
        } else {
            "No matches."
        }),
        Ok((hits, capped)) => {
            let mut out = hits.join("\n");
            if capped {
                out.push_str("\n…(stopped early - narrow the search for more)");
            }
            ToolOutcome {
                log: format!("{} match{}", hits.len(), if hits.len() == 1 { "" } else { "es" }),
                response: json!({ "result": out }),
                image_data_url: None,
            }
        }
        Err(e) => ToolOutcome::err(format!("search failed: {}", e)),
    }
}
