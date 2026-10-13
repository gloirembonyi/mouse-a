// Mouse AI Desktop - File browser + editor backend commands.
// Backs the IDE dock's Editor tab: a lazy-loading file tree plus read/write
// for the CodeMirror pane. Deliberately separate from agent.rs's read_file/
// write_file/list_directory tool functions - those take raw JSON tool-call
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

/// Decide whether a directory entry belongs in the tree, and whether it's a
/// folder. Windows home folders are full of entries nobody can open - legacy
/// junctions like "My Documents"/"NetHood" (hidden+system reparse points that
/// deny access) and registry hives like NTUSER.DAT - which used to show up as
/// files that failed with "Access is denied". Those are skipped, and symlinks
/// / junctions are resolved so a linked folder shows as a folder.
fn classify(entry: &std::fs::DirEntry) -> Option<bool> {
    let meta = entry.metadata().ok()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        const REPARSE_POINT: u32 = 0x400;
        let attrs = meta.file_attributes();
        if attrs & (HIDDEN | SYSTEM) != 0 {
            return None;
        }
        if attrs & REPARSE_POINT != 0 {
            // Follow the link; an unreachable target isn't worth listing.
            let target = std::fs::metadata(entry.path()).ok()?;
            if target.is_dir() && std::fs::read_dir(entry.path()).is_err() {
                return None;
            }
            return Some(target.is_dir());
        }
    }
    if meta.file_type().is_symlink() {
        return std::fs::metadata(entry.path()).ok().map(|m| m.is_dir());
    }
    Some(meta.is_dir())
}

#[tauri::command]
pub fn fs_list_dir(path: String) -> Result<Vec<FsEntry>, String> {
    let rd = std::fs::read_dir(&path).map_err(|e| format!("Could not open {}: {}", path, e))?;
    let mut entries: Vec<FsEntry> = rd
        .flatten()
        .filter_map(|e| {
            let is_dir = classify(&e)?;
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
// hang the UI - stops after visiting MAX_VISITED entries or once `limit`
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
        let Some(is_dir) = classify(&entry) else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
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

// ── File info, previews and file management (editor dock) ──────────

#[derive(Serialize)]
pub struct FileInfo {
    pub size: u64,
    /// "text" | "image" | "pdf" | "binary", plus the read-only viewer kinds
    /// "docx" | "pptx" | "sheet" | "audio" | "video". CSV/HTML/JSON stay
    /// "text" (they're editable) - the editor picks their view by extension.
    pub kind: String,
    /// Text file uses CRLF line endings (the editor normalizes to LF, so it
    /// converts back on save instead of silently rewriting every line).
    pub crlf: bool,
}

const IMAGE_EXTS: [&str; 9] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "avif"];
// Played straight from disk through the asset protocol, never via IPC.
const AUDIO_EXTS: [&str; 9] = ["mp3", "wav", "ogg", "oga", "m4a", "flac", "aac", "opus", "weba"];
const VIDEO_EXTS: [&str; 6] = ["mp4", "webm", "mov", "mkv", "m4v", "ogv"];

#[tauri::command]
pub fn fs_file_info(path: String) -> Result<FileInfo, String> {
    let meta = std::fs::metadata(&path).map_err(|e| format!("Could not open {}: {}", path, e))?;
    if meta.is_dir() {
        return Err(format!("{} is a folder", path));
    }
    let ext = std::path::Path::new(&path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return Ok(FileInfo { size: meta.len(), kind: "image".into(), crlf: false });
    }
    let viewer_kind = match ext.as_str() {
        "pdf" => Some("pdf"),
        e if crate::documents::DOCX_EXTS.contains(&e) => Some("docx"),
        e if crate::documents::PPTX_EXTS.contains(&e) => Some("pptx"),
        e if crate::documents::SHEET_EXTS.contains(&e) => Some("sheet"),
        e if AUDIO_EXTS.contains(&e) => Some("audio"),
        e if VIDEO_EXTS.contains(&e) => Some("video"),
        _ => None,
    };
    if let Some(kind) = viewer_kind {
        return Ok(FileInfo { size: meta.len(), kind: kind.into(), crlf: false });
    }
    use std::io::Read;
    let mut head = vec![0u8; 8192];
    let n = std::fs::File::open(&path)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|e| format!("Could not read {}: {}", path, e))?;
    head.truncate(n);
    // NUL bytes or lots of invalid UTF-8 = not something to edit as text.
    // (A multi-byte char cut at the 8 KB boundary is only one bad sequence.)
    let invalid = String::from_utf8_lossy(&head).matches('\u{FFFD}').count();
    let binary = head.contains(&0) || invalid > 2;
    Ok(FileInfo {
        size: meta.len(),
        kind: if binary { "binary".into() } else { "text".into() },
        crlf: head.windows(2).any(|w| w == b"\r\n"),
    })
}

/// Image/PDF preview as a data URL (capped so a huge file can't stall the UI).
#[tauri::command]
pub fn fs_read_data_url(path: String) -> Result<String, String> {
    use base64::Engine;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > 25 * 1024 * 1024 {
        return Err("Too large to preview (over 25 MB).".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("Could not read {}: {}", path, e))?;
    let ext = std::path::Path::new(&path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    };
    Ok(format!("data:{};base64,{}", mime, base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[tauri::command]
pub fn fs_create_file(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if p.exists() {
        return Err("A file or folder with that name already exists.".into());
    }
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, "").map_err(|e| format!("Could not create {}: {}", path, e))
}

#[tauri::command]
pub fn fs_create_dir(path: String) -> Result<(), String> {
    if std::path::Path::new(&path).exists() {
        return Err("A file or folder with that name already exists.".into());
    }
    std::fs::create_dir_all(&path).map_err(|e| format!("Could not create {}: {}", path, e))
}

#[tauri::command]
pub fn fs_rename(from: String, to: String) -> Result<(), String> {
    if std::path::Path::new(&to).exists() {
        return Err("A file or folder with that name already exists.".into());
    }
    std::fs::rename(&from, &to).map_err(|e| format!("Could not rename: {}", e))
}

/// Moves to the Recycle Bin (undoable from Explorer) instead of deleting
/// outright.
#[tauri::command]
pub fn fs_delete(path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).exists() {
        return Err("It no longer exists.".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let quoted = path.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName Microsoft.VisualBasic; $p = '{q}'; \
if (Test-Path -LiteralPath $p -PathType Container) {{ [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($p, 'OnlyErrorDialogs', 'SendToRecycleBin') }} \
else {{ [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($p, 'OnlyErrorDialogs', 'SendToRecycleBin') }}",
            q = quoted
        );
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("Could not delete: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let p = std::path::Path::new(&path);
        let r = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
        r.map_err(|e| e.to_string())
    }
}

/// Shows the item selected in File Explorer.
#[tauri::command]
pub fn fs_reveal(path: String) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Opens with the default app (PDFs, Office docs, images…).
#[tauri::command]
pub fn fs_open_default(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(path, None::<&str>).map_err(|e| e.to_string())
}
