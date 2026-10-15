// Mouse AI Desktop - Integrated terminal backend, IDE dock's Terminal tab.
// Real PTYs (not piped subprocesses) via `portable-pty` - gives interactive
// programs (vim, npm's progress bars, prompts) a proper terminal to render
// into, which a plain stdout pipe can't do. Several sessions can run at once
// (one per terminal tab, keyed by an id the frontend picks); output streams
// to the frontend as "pty-output" { id, data } events, input flows back
// through `pty_write`.
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

struct Session {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Default)]
pub struct PtyState {
    sessions: Mutex<HashMap<String, Session>>,
}

/// Longest prefix of `buf` that is complete UTF-8; the rest (a multi-byte
/// character split across reads) is carried into the next read instead of
/// being turned into replacement characters.
fn split_utf8(buf: &[u8]) -> usize {
    match std::str::from_utf8(buf) {
        Ok(_) => buf.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => buf.len(), // genuinely invalid bytes: emit lossily, don't stall
    }
}

#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    state: State<'_, PtyState>,
    id: String,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    // Re-spawning an id (Restart) replaces the old session.
    if let Some(mut old) = state.sessions.lock().unwrap().remove(&id) {
        let _ = old.killer.kill();
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("Could not open a terminal: {}", e))?;

    let shell = if cfg!(windows) {
        "powershell.exe".to_string()
    } else {
        std::env::var("SHELL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "/bin/bash".to_string())
    };
    let mut cmd = CommandBuilder::new(shell.as_str());
    if cfg!(windows) {
        cmd.arg("-NoLogo");
    } else {
        cmd.arg("-l"); // login shell: the user's PATH (Homebrew, nvm, cargo)
    }
    if let Some(dir) = cwd.as_ref().filter(|d| std::path::Path::new(d).is_dir()) {
        cmd.cwd(dir);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Could not start the shell: {}", e))?;
    // The slave side belongs to the child now; drop our handle so the master
    // side gets EOF once the child exits instead of hanging open forever.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Could not attach to the terminal: {}", e))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Could not attach to the terminal: {}", e))?;
    let killer = child.clone_killer();

    state
        .sessions
        .lock()
        .unwrap()
        .insert(id.clone(), Session { writer, master: pair.master, killer });

    let app_handle = app.clone();
    let reader_id = id.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16384];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    carry.extend_from_slice(&buf[..n]);
                    let cut = split_utf8(&carry);
                    if cut == 0 {
                        continue;
                    }
                    let text = String::from_utf8_lossy(&carry[..cut]).into_owned();
                    carry.drain(..cut);
                    if app_handle.emit("pty-output", json!({ "id": reader_id, "data": text })).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = app_handle.emit("pty-exit", json!({ "id": reader_id }));
    });

    // Reap the child on its own thread so a finished shell doesn't linger as
    // a zombie process; this thread just blocks until that happens.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

#[tauri::command]
pub fn pty_write(state: State<'_, PtyState>, id: String, data: String) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    match sessions.get_mut(&id) {
        Some(s) => s.writer.write_all(data.as_bytes()).map_err(|e| e.to_string()),
        None => Err("That terminal isn't running.".to_string()),
    }
}

#[tauri::command]
pub fn pty_resize(state: State<'_, PtyState>, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    match sessions.get(&id) {
        Some(s) => s
            .master
            .resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string()),
        None => Ok(()), // Not running (yet) - a resize then is a no-op, not an error.
    }
}

/// Ends a terminal session (closing its tab).
#[tauri::command]
pub fn pty_kill(state: State<'_, PtyState>, id: String) {
    if let Some(mut s) = state.sessions.lock().unwrap().remove(&id) {
        let _ = s.killer.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::split_utf8;

    #[test]
    fn keeps_split_multibyte_chars_for_next_read() {
        let s = "ok ✓".as_bytes();
        // Cut in the middle of the 3-byte check mark.
        let partial = &s[..s.len() - 1];
        assert_eq!(split_utf8(partial), 3);
        assert_eq!(split_utf8(s), s.len());
    }
}
