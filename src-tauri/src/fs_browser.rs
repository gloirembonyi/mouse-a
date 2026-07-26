// Mouse AI Desktop — File browser + editor backend commands.
// Backs the IDE dock's Editor tab: a lazy-loading file tree plus read/write
// for the CodeMirror pane. Deliberately separate from agent.rs's read_file/
// write_file/list_directory tool functions — those take raw JSON tool-call
// args and return agent-facing summaries, while these return typed data the
// editor UI can render directly (entries with is_dir, etc.).
use serde::Serialize;

#[derive(Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

// Directories that would otherwise dominate every tree with noise.
const SKIP_DIRS: [&str; 6] = ["node_modules", ".git", "target", "dist", "dist-firefox", "__pycache__"];

#[tauri::command]
pub fn fs_home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine your home directory.".to_string())
}

#[tauri::command]
pub fn fs_list_dir(path: String) -> Result<Vec<FsEntry>, String> {
    let rd = std::fs::read_dir(&path).map_err(|e| format!("Could not open {}: {}", path, e))?;
    let mut entries: Vec<FsEntry> = rd
        .flatten()
        .filter_map(|e| {
            let is_dir = e.file_type().ok()?.is_dir();
            let name = e.file_name().to_string_lossy().to_string();
            if is_dir && SKIP_DIRS.contains(&name.as_str()) {
                return None;
            }
            Some(FsEntry {
                path: e.path().to_string_lossy().to_string(),
                name,
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(entries)
}

#[tauri::command]
pub fn fs_read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("Could not read {}: {}", path, e))
}

#[tauri::command]
pub fn fs_write_text_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| format!("Could not write {}: {}", path, e))
}

#[tauri::command]
pub fn fs_parent_dir(path: String) -> Option<String> {
    std::path::Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .filter(|p| !p.is_empty())
}

// Bounded so a huge tree (or a root someone points at their whole C:\) can't
// hang the UI — stops after visiting MAX_VISITED entries or once `limit`
// matches are found, whichever comes first. Backs the chat composer's @file
// tag picker (live-filter-as-you-type), so this needs to feel instant.
const MAX_VISITED: usize = 20_000;
const MAX_DEPTH: usize = 8;

#[tauri::command]
pub fn fs_search_files(root: String, query: String, limit: Option<u32>) -> Result<Vec<FsEntry>, String> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let cap = limit.unwrap_or(30).clamp(1, 100) as usize;
    let mut results = Vec::new();
    let mut visited = 0usize;
    search_dir(std::path::Path::new(&root), &needle, 0, &mut visited, cap, &mut results);
    Ok(results)
}

fn search_dir(dir: &std::path::Path, needle: &str, depth: usize, visited: &mut usize, cap: usize, out: &mut Vec<FsEntry>) {
    if out.len() >= cap || *visited >= MAX_VISITED || depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        if out.len() >= cap || *visited >= MAX_VISITED {
            return;
        }
        *visited += 1;
        let Ok(file_type) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = file_type.is_dir();
        if is_dir && SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        if name.to_lowercase().contains(needle) {
            out.push(FsEntry { path: entry.path().to_string_lossy().to_string(), name: name.clone(), is_dir });
        }
        if is_dir {
            search_dir(&entry.path(), needle, depth + 1, visited, cap, out);
        }
    }
}
