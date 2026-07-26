// Mouse AI Desktop — Integrated terminal backend, IDE dock's Terminal tab.
// A real PTY (not a piped subprocess) via `portable-pty` — gives interactive
// programs (vim, npm's progress bars, prompts) a proper terminal to render
// into, which a plain stdout pipe can't do. One session per app (matches the
// single Terminal tab in the dock); output streams to the frontend as
// "pty-output" events, input flows back through `pty_write`.
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

pub struct PtyState {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
}

impl Default for PtyState {
    fn default() -> Self {
        Self {
            writer: Mutex::new(None),
            master: Mutex::new(None),
        }
    }
}

#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    state: State<'_, PtyState>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("Could not open a terminal: {}", e))?;

    let shell = if cfg!(windows) { "powershell.exe" } else { "bash" };
    let mut cmd = CommandBuilder::new(shell);
    if let Some(dir) = &cwd {
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

    *state.writer.lock().unwrap() = Some(writer);
    *state.master.lock().unwrap() = Some(pair.master);

    let app_handle = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if app_handle.emit("pty-output", text).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = app_handle.emit("pty-exit", ());
    });

    // Reap the child on its own thread so a finished shell doesn't linger as
    // a zombie process; this thread just blocks until that happens.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

#[tauri::command]
pub fn pty_write(state: State<'_, PtyState>, data: String) -> Result<(), String> {
    let mut guard = state.writer.lock().unwrap();
    match guard.as_mut() {
        Some(writer) => writer.write_all(data.as_bytes()).map_err(|e| e.to_string()),
        None => Err("No terminal session is running.".to_string()),
    }
}

#[tauri::command]
pub fn pty_resize(state: State<'_, PtyState>, cols: u16, rows: u16) -> Result<(), String> {
    let guard = state.master.lock().unwrap();
    match guard.as_ref() {
        Some(master) => master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string()),
        None => Ok(()), // Nothing running yet — a resize before spawn is a no-op, not an error.
    }
}
