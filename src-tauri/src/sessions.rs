//! Saved Kudu Pilot sessions: one JSON file per call in `<config>/mouse-ai/sessions/`.
//!
//! The front end owns the shape (transcript, answer cards, notes); this module is a small,
//! crash-safe store: atomic writes (temp file + rename), id validation so a session id can
//! never name a path, and listing that tolerates a corrupt file. Audio is never stored.

use serde_json::{json, Value};
use std::path::PathBuf;

const MAX_SESSIONS: usize = 300;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

fn dir() -> Result<PathBuf, String> {
    let d = dirs::config_dir().ok_or("No config directory available")?.join("mouse-ai").join("sessions");
    std::fs::create_dir_all(&d).map_err(|e| format!("Cannot create the sessions folder: {}", e))?;
    Ok(d)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn path_for(id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Invalid session id.".to_string());
    }
    Ok(dir()?.join(format!("{}.json", id)))
}

fn read(path: &PathBuf) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

/// Create or replace a session. Written to a temp file first so a crash mid-write can never
/// leave a half-written (unreadable) session behind.
#[tauri::command]
pub fn session_save(id: String, session: Value) -> Result<(), String> {
    save_value(&id, &session)
}

pub(crate) fn save_value(id: &str, session: &Value) -> Result<(), String> {
    let path = path_for(id)?;
    let json = serde_json::to_string(session).map_err(|e| e.to_string())?;
    if json.len() > MAX_FILE_BYTES {
        return Err("This session is too large to save.".to_string());
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("Failed to save the session: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("Failed to save the session: {}", e))?;
    prune();
    Ok(())
}

/// Newest first. Summaries only (no transcript), so the list stays cheap with hundreds of calls.
#[tauri::command]
pub fn session_list() -> Result<Vec<Value>, String> {
    let mut out: Vec<Value> = Vec::new();
    for entry in std::fs::read_dir(dir()?).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(v) = read(&path) else { continue };
        let transcript = v.get("transcript").and_then(|t| t.as_array());
        let preview = transcript
            .and_then(|t| t.iter().find(|e| text(e, "speaker") == "them"))
            .map(|e| text(e, "text").chars().take(140).collect::<String>())
            .unwrap_or_default();
        out.push(json!({
            "id": text(&v, "id"),
            "title": text(&v, "title"),
            "mode": text(&v, "mode"),
            "started_ms": v.get("started_ms").cloned().unwrap_or(Value::Null),
            "updated_ms": v.get("updated_ms").cloned().unwrap_or(json!(0)),
            "lines": transcript.map(|t| t.len()).unwrap_or(0),
            "answers": v.get("cards").and_then(|c| c.as_array()).map(|c| c.len()).unwrap_or(0),
            "preview": preview,
            "ended": v.get("ended").and_then(|x| x.as_bool()).unwrap_or(true),
        }));
    }
    out.sort_by_key(|v| std::cmp::Reverse(v.get("updated_ms").and_then(|x| x.as_i64()).unwrap_or(0)));
    Ok(out)
}

#[tauri::command]
pub fn session_get(id: String) -> Result<Value, String> {
    read(&path_for(&id)?).ok_or_else(|| "That session could not be found.".to_string())
}

#[tauri::command]
pub fn session_delete(id: String) -> Result<(), String> {
    let path = path_for(&id)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to delete the session: {}", e)),
    }
}

/// Drop the oldest sessions beyond MAX_SESSIONS so the folder cannot grow without bound.
fn prune() {
    let Ok(d) = dir() else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= MAX_SESSIONS {
        return;
    }
    files.sort_by_key(|(t, _)| *t);
    let extra = files.len() - MAX_SESSIONS;
    for (_, p) in files.into_iter().take(extra) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_cannot_name_paths() {
        assert!(valid_id("abc-123_X"));
        assert!(!valid_id(""));
        assert!(!valid_id("../evil"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id("a\\b"));
        assert!(!valid_id(&"x".repeat(65)));
    }

    #[test]
    fn save_get_list_delete_roundtrip() {
        let id = format!("test-{}", std::process::id());
        let session = json!({
            "id": id, "title": "Test call", "mode": "interview", "started_ms": 1, "updated_ms": 5,
            "transcript": [{ "id": 1, "speaker": "them", "text": "How would you scale this?", "at_ms": 2 }],
            "cards": [{ "id": "c1", "text": "Say: shard it." }],
        });
        session_save(id.clone(), session.clone()).unwrap();
        assert_eq!(session_get(id.clone()).unwrap()["title"], "Test call");
        let listed = session_list().unwrap();
        let mine = listed.iter().find(|v| v["id"] == json!(id)).expect("listed");
        assert_eq!(mine["lines"], 1);
        assert_eq!(mine["answers"], 1);
        assert!(mine["preview"].as_str().unwrap().starts_with("How would you"));
        session_delete(id.clone()).unwrap();
        assert!(session_get(id).is_err());
    }
}
