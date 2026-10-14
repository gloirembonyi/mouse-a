// Mouse AI desktop agent - run_command: PowerShell with timeouts, cancellation and output capture.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// A single foreground command can't hold the whole task hostage.
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);

// A model-requested timeout_seconds is honored up to this - long enough for a
// real build/install, still bounded so a stuck command can't wedge the task
// indefinitely (background:true remains the right tool for anything longer).
pub(crate) const MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Refuse obviously catastrophic shell commands unless the user has opted into
/// running them. This is a coarse safety net, not a sandbox - it blocks the
/// handful of one-liners that could wipe the machine, and lets everything else
/// through (this is the user's own PC, acting on their own request).
pub(crate) fn is_destructive(command: &str) -> Option<&'static str> {
    let c = command.to_lowercase();
    // Words, not substrings: "format " used to block `Get-Date -Format 'yyyy'` and
    // `npm run format`, and "shutdown" any text mentioning the word.
    let words: Vec<&str> = c
        .split(|ch: char| ch.is_whitespace() || ch == ';' || ch == '|' || ch == '&' || ch == '(' || ch == ')')
        .filter(|w| !w.is_empty())
        .collect();
    let follows = |cmd: &str, pred: &dyn Fn(&str) -> bool| {
        words.windows(2).any(|w| w[0] == cmd && pred(w[1]))
    };
    let is_drive_root = |w: &str| {
        let w = w.trim_matches(|ch| ch == '"' || ch == '\'');
        w.len() <= 3 && w.as_bytes().first().map(|b| b.is_ascii_alphabetic()).unwrap_or(false) && w.get(1..2) == Some(":")
    };
    if follows("format", &|w: &str| is_drive_root(w)) {
        return Some("disk format");
    }
    if words.iter().any(|w| matches!(*w, "stop-computer" | "restart-computer" | "diskpart" | "mkfs"))
        || follows("shutdown", &|w: &str| w.starts_with('/') || w.starts_with('-'))
        || follows("shutdown.exe", &|w: &str| w.starts_with('/') || w.starts_with('-'))
    {
        return Some("shutting down / repartitioning the machine");
    }
    if c.contains("cipher /w") {
        return Some("disk wipe");
    }
    // Wiping a whole drive, Windows, or the user's home - not a folder inside a project.
    let wipes_root = |target: &str| {
        let t = target.trim_matches(|ch| ch == '"' || ch == '\'').trim_end_matches(['\\', '/']);
        is_drive_root(t) || t.ends_with(":\\windows") || t == "~" || t == "$home" || t == "$env:userprofile" || t == "/"
    };
    for (i, w) in words.iter().enumerate() {
        let recursive_rm = matches!(*w, "rd" | "rmdir" | "remove-item" | "rm" | "del" | "ri");
        if recursive_rm {
            if words[i + 1..].iter().take(6).any(|t| wipes_root(t)) {
                return Some("recursive delete of a drive, Windows, or your home folder");
            }
        }
    }
    None
}

pub(crate) async fn run_command_tool(source: &str, args: &Value) -> ToolOutcome {
    let command = args.get("command").and_then(|c| c.as_str()).unwrap_or("").trim().to_string();
    if command.is_empty() {
        return ToolOutcome::err("no command provided");
    }
    if let Some(label) = is_destructive(&command) {
        return ToolOutcome::err(format!(
            "Refused: this looks like {}. I won't run destructive commands automatically.",
            label
        ));
    }
    let background = args.get("background").and_then(|b| b.as_bool()).unwrap_or(false);
    // No cwd → the agent's own scratch dir, never the host process's actual
    // cwd (which under `tauri dev` is the app's own source tree - that's how
    // throwaway scripts used to end up committed alongside real project
    // files). A real project/folder task should always pass an explicit cwd.
    let cwd = args
        .get("cwd")
        .and_then(|c| c.as_str())
        .filter(|c| !c.trim().is_empty())
        // Same resolution as the file tools: ~, %VAR%, Documents\… and friends.
        .map(|c| resolve_agent_path(c).to_string_lossy().into_owned())
        .unwrap_or_else(|| agent_scratch_dir().to_string_lossy().into_owned());
    let cwd = Some(cwd);
    let timeout = args
        .get("timeout_seconds")
        .and_then(|t| t.as_u64())
        .map(Duration::from_secs)
        .map(|d| d.clamp(Duration::from_secs(1), MAX_COMMAND_TIMEOUT))
        .unwrap_or(COMMAND_TIMEOUT);
    let cmd = command.clone();
    let src = source.to_string();

    let result = tokio::task::spawn_blocking(move || {
        run_powershell_cancellable(&cmd, background, cwd.as_deref(), timeout, &|| cancel_pending(&src))
    })
    .await;
    match result {
        Ok(Ok(CommandRun::Background { pid, log_path })) => ToolOutcome {
            response: json!({
                "started": true,
                "pid": pid,
                "log_path": log_path,
                "note": "Runs detached. read_file the log after a short wait to confirm it actually started (and on which port) before relying on it.",
                "stop_hint": format!("Stop-Process -Id {} (or taskkill /T /F /PID {} for its whole process tree)", pid, pid),
            }),
            image_data_url: None,
            log: format!("started in background: {}", command),
        },
        // Structured, like a shell's own contract: separate streams, the exit code as a
        // number, a timeout as data (not an error that loses the output), and an explicit
        // truncation flag. tool_failed() reads `ok`; the model reads stderr first.
        Ok(Ok(CommandRun::Finished(r))) => {
            let ok = r.exit_code == Some(0) && !r.timed_out;
            let mut resp = json!({
                "ok": ok,
                // Head + tail, never head-only: the error and the failing test live at
                // the END of a build's output, and cutting them used to hide failures.
                "stdout": clip_middle(&r.stdout, MAX_TOOL_OUTPUT),
                "stderr": clip_middle(&r.stderr, MAX_TOOL_OUTPUT / 2),
                "exit_code": r.exit_code,
                "timed_out": r.timed_out,
                "truncated": r.stdout_truncated || r.stderr_truncated,
                "duration_ms": r.duration_ms,
            });
            if r.timed_out {
                resp["note"] = json!(format!(
                    "The command was ended after {}s with its process tree. Pass a longer timeout_seconds (up to 300) for a slow-but-finite command, or background=true for a server/watcher.",
                    timeout.as_secs()
                ));
            } else if !ok {
                resp["note"] = json!("Non-zero exit code - read stderr (then stdout) for the cause and fix it; do not re-run the same command unchanged.");
            }
            if r.stdout_truncated || r.stderr_truncated {
                resp["truncation_note"] = json!("The middle of the output was dropped (16 KiB per stream kept). Narrow the command (Select-Object -First/-Last, a filter, or write to a log file and read_file it) instead of re-running it.");
            }
            let head = if r.stderr.trim().is_empty() { r.stdout.clone() } else { format!("{}\n{}", r.stderr.trim_end(), r.stdout) };
            let mut log = truncate(head.trim(), 300);
            if r.timed_out {
                log.push_str(" [timed out]");
            } else if let Some(code) = r.exit_code.filter(|c| *c != 0) {
                log.push_str(&format!(" [exit {}]", code));
            }
            if log.trim().is_empty() {
                log = "(no output)".to_string();
            }
            ToolOutcome { response: resp, image_data_url: None, log }
        }
        Ok(Err(e)) => ToolOutcome::err(e),
        Err(e) => ToolOutcome::err(format!("task failed: {}", e)),
    }
}

/// What a finished (foreground) command produced. Output is capped per stream
/// (see `wait_with_timeout`), so a runaway command cannot eat the process's memory.
#[derive(Debug, Clone)]
pub(crate) struct CommandResult {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) duration_ms: u64,
}

#[derive(Debug)]
pub(crate) enum CommandRun {
    Background { pid: u32, log_path: String },
    Finished(CommandResult),
}

/// Legacy single-string view of a run (the test suite and the log line use it):
/// stdout, then `[stderr] …`, then `[exit code N]` on failure; a timeout is an Err.
#[cfg(test)]
pub(crate) fn command_run_to_text(run: CommandRun, timeout: Duration) -> Result<String, String> {
    match run {
        CommandRun::Background { pid, log_path } => Ok(format!(
            "Started in the background (pid {}). Output is being written to {} - read_file it to check progress. \
Stop it later with: Stop-Process -Id {} (or taskkill /T /F /PID {} for its whole process tree).",
            pid, log_path, pid, pid
        )),
        CommandRun::Finished(r) => {
            let mut combined = String::new();
            if !r.stdout.trim().is_empty() {
                combined.push_str(r.stdout.trim_end());
            }
            if !r.stderr.trim().is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str("[stderr] ");
                combined.push_str(r.stderr.trim_end());
            }
            if r.timed_out {
                return Err(format!(
                    "command timed out after {}s and was ended (with its child processes). Pass a longer timeout_seconds for a slow-but-finite command, or background=true for a long-running/indefinite one. Output so far:\n{}",
                    timeout.as_secs(),
                    if combined.is_empty() { "(none)".to_string() } else { clip_middle(&combined, 4000) }
                ));
            }
            if combined.is_empty() {
                combined.push_str("(command completed with no output)");
            }
            match r.exit_code {
                Some(0) | None => {}
                Some(code) => combined.push_str(&format!("\n[exit code {}]", code)),
            }
            Ok(combined)
        }
    }
}

/// `[exit code N]` at the end of run_powershell's output → N (0 when absent).
#[cfg(test)]
pub(crate) fn exit_code_of(out: &str) -> i64 {
    let tail = out.trim_end();
    if !tail.ends_with(']') {
        return 0;
    }
    tail.rfind("[exit code ")
        .and_then(|i| tail[i + 11..tail.len() - 1].trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// The shell prelude every agent command runs inside.
///  - UTF-8 WITHOUT a BOM in and out (the BOM variant leaked U+FEFF into output),
///    and `>`/Out-File/Set-Content default to UTF-8 instead of 5.1's UTF-16, so
///    read_file can read what the command wrote.
///  - PATH re-read from the registry: something installed earlier in the run
///    (winget install node) is usable without restarting the app.
///  - The command runs in a script block, and the process exits with the real
///    status: the last native exit code, or 1 if the last cmdlet failed. Before,
///    `npm run build; Write-Output done` exited 0 and the failure was invisible.
pub(crate) fn powershell_prelude(command: &str) -> String {
    format!(
        "$ProgressPreference='SilentlyContinue'; $ErrorActionPreference='Continue'; \
$__u = New-Object System.Text.UTF8Encoding $false; [Console]::OutputEncoding=$__u; $OutputEncoding=$__u; \
$PSDefaultParameterValues['Out-File:Encoding']='utf8'; $PSDefaultParameterValues['Set-Content:Encoding']='utf8'; \
$PSDefaultParameterValues['Add-Content:Encoding']='utf8'; \
try {{ $env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User') }} catch {{}}; \
$global:LASTEXITCODE = 0; & {{\n{}\n}}; if ($LASTEXITCODE) {{ exit $LASTEXITCODE }} elseif (-not $?) {{ exit 1 }}",
        command
    )
}

pub(crate) fn hidden_powershell(command: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &powershell_prelude(command)]);
    // No hidden prompts that would sit until the timeout: git asking for a
    // password, npx asking "Ok to proceed?", Python crashing on non-ASCII output.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("npm_config_yes", "true")
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("NO_COLOR", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW - no console flash per command
    }
    cmd
}

#[cfg(test)]
pub(crate) fn run_powershell(command: &str, background: bool, cwd: Option<&str>, timeout: Duration) -> Result<String, String> {
    run_powershell_cancellable(command, background, cwd, timeout, &|| false).and_then(|run| command_run_to_text(run, timeout))
}


pub(crate) fn run_powershell_cancellable(
    command: &str,
    background: bool,
    cwd: Option<&str>,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<CommandRun, String> {
    use std::process::Stdio;

    if let Some(dir) = cwd {
        if !std::path::Path::new(dir).is_dir() {
            return Err(format!("cwd \"{}\" doesn't exist or isn't a folder.", dir));
        }
    }

    if background {
        // Detached, but its output goes to a log file the agent can read_file
        // later (e.g. to see whether a dev server actually started). Logs live
        // outside the scratch folder so the next run's cleanup can't wipe them.
        let log_dir = std::env::temp_dir().join("mouse-ai-agent-logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log = log_dir.join(format!("bg-{}.log", uuid::Uuid::new_v4().simple()));
        let file = std::fs::File::create(&log).map_err(|e| format!("couldn't create log: {}", e))?;
        let err_file = file.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = hidden_powershell(command);
        cmd.stdout(Stdio::from(file)).stderr(Stdio::from(err_file)).stdin(Stdio::null());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let child = cmd.spawn().map_err(|e| format!("failed to start: {}", e))?;
        return Ok(CommandRun::Background { pid: child.id(), log_path: log.display().to_string() });
    }

    let mut spawn_cmd = hidden_powershell(command);
    spawn_cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    if let Some(dir) = cwd {
        spawn_cmd.current_dir(dir);
    }
    let started_at = std::time::Instant::now();
    let child = spawn_cmd.spawn().map_err(|e| format!("failed to start: {}", e))?;

    // Enforce a wall-clock timeout so one hung command can't stall the agent.
    let (status, (stdout, stdout_truncated), (stderr, stderr_truncated), ended) =
        wait_with_timeout(child, timeout, cancelled)?;
    if matches!(ended, WaitEnd::Cancelled) {
        return Err("Stopped at your request (the command and its child processes were ended).".to_string());
    }
    Ok(CommandRun::Finished(CommandResult {
        stdout,
        stderr,
        exit_code: status.and_then(|s| s.code()),
        timed_out: matches!(ended, WaitEnd::TimedOut),
        stdout_truncated,
        stderr_truncated,
        duration_ms: started_at.elapsed().as_millis() as u64,
    }))
}

pub(crate) enum WaitEnd {
    Exited,
    TimedOut,
    Cancelled,
}

/// End a process AND everything it started. Killing only powershell.exe left
/// npm/node/git children holding the output pipes open, and the agent then hung
/// forever waiting for EOF.
pub(crate) fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(0x0800_0000)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for the child while DRAINING its pipes on background threads.
/// Reading only after exit (the old way) deadlocks as soon as a command
/// prints more than the pipe buffer (npm install, builds, big listings):
/// the child blocks writing, never exits, and "times out". The drains are
/// collected with a deadline, so a grandchild that inherited a pipe can't
/// block the agent after the command itself has finished.
/// Per-stream cap: the first STREAM_HEAD bytes and the last STREAM_TAIL bytes are
/// kept (16 KiB together), the middle is dropped with a marker. A command that prints
/// megabytes (a verbose build, `Get-ChildItem -Recurse C:\`) therefore costs a bounded
/// amount of memory and a bounded tool result - and the verdict at the END survives.
pub(crate) const STREAM_HEAD: usize = 4 * 1024;
pub(crate) const STREAM_TAIL: usize = 12 * 1024;

pub(crate) fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Option<std::process::ExitStatus>, (String, bool), (String, bool), WaitEnd), String> {
    use std::io::Read;
    use std::time::Instant;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel::<(String, bool)>();
        std::thread::spawn(move || {
            let mut head: Vec<u8> = Vec::with_capacity(STREAM_HEAD);
            let mut tail: Vec<u8> = Vec::with_capacity(STREAM_TAIL + 8192);
            let mut total: usize = 0;
            if let Some(mut p) = pipe {
                let mut buf = [0u8; 8192];
                loop {
                    let n = match p.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    total += n;
                    let mut chunk = &buf[..n];
                    if head.len() < STREAM_HEAD {
                        let take = (STREAM_HEAD - head.len()).min(chunk.len());
                        head.extend_from_slice(&chunk[..take]);
                        chunk = &chunk[take..];
                    }
                    if !chunk.is_empty() {
                        tail.extend_from_slice(chunk);
                        if tail.len() > STREAM_TAIL {
                            let excess = tail.len() - STREAM_TAIL;
                            tail.drain(..excess);
                        }
                    }
                }
            }
            let truncated = total > STREAM_HEAD + STREAM_TAIL;
            let mut text = String::from_utf8_lossy(&head).into_owned();
            if truncated {
                text.push_str(&format!(
                    "\n…[{} bytes omitted - output capped at {} KiB per stream]…\n",
                    total - head.len() - tail.len(),
                    (STREAM_HEAD + STREAM_TAIL) / 1024
                ));
            }
            text.push_str(&String::from_utf8_lossy(&tail));
            let _ = tx.send((text, truncated));
        });
        rx
    };
    let out_rx = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err_rx = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));

    let start = Instant::now();
    let (status, ended) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), WaitEnd::Exited),
            Ok(None) if cancelled() => {
                kill_tree(&mut child);
                break (None, WaitEnd::Cancelled);
            }
            Ok(None) if start.elapsed() >= timeout => {
                kill_tree(&mut child);
                break (None, WaitEnd::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(60)),
            Err(e) => return Err(format!("wait failed: {}", e)),
        }
    };
    let grace = Duration::from_secs(3);
    let held = "(output stream still held open by a background child process)";
    let stdout = out_rx.recv_timeout(grace).unwrap_or_else(|_| (held.to_string(), false));
    let stderr = err_rx.recv_timeout(Duration::from_millis(500)).unwrap_or_default();
    Ok((status, stdout, stderr, ended))
}

#[cfg(test)]
mod run_command_tests {
    use super::*;

    /// More output than a pipe buffer must not deadlock (it used to hang
    /// until the timeout).
    #[test]
    pub(crate) fn large_output_does_not_deadlock() {
        let out = run_powershell("1..20000 | ForEach-Object { \"line $_ of output\" }", false, None, Duration::from_secs(60))
            .expect("command should finish");
        assert!(out.contains("line 20000 of output"), "tail missing: {}", &out[out.len().saturating_sub(200)..]);
    }

    #[test]
    pub(crate) fn reports_nonzero_exit_code() {
        let out = run_powershell("Write-Output 'oops'; exit 3", false, None, Duration::from_secs(30)).unwrap();
        assert!(out.contains("[exit code 3]"), "{}", out);
    }

    /// A failing native command followed by more output used to exit 0.
    #[test]
    pub(crate) fn failure_before_last_statement_is_reported() {
        let out = run_powershell("cmd /c exit 4; Write-Output 'after'", false, None, Duration::from_secs(30)).unwrap();
        assert!(out.contains("[exit code 4]"), "{}", out);
        assert_eq!(exit_code_of(&out), 4);
    }

    /// A timeout must end the child processes too, and return promptly.
    #[test]
    pub(crate) fn timeout_kills_the_process_tree() {
        let started = std::time::Instant::now();
        let err = run_powershell("Start-Process -NoNewWindow -Wait ping -ArgumentList '-n','30','127.0.0.1'", false, None, Duration::from_secs(2))
            .unwrap_err();
        assert!(err.contains("timed out"), "{}", err);
        assert!(started.elapsed() < Duration::from_secs(12), "took {:?}", started.elapsed());
    }

    #[test]
    pub(crate) fn cancel_stops_a_running_command() {
        let started = std::time::Instant::now();
        let cancel_at = started + Duration::from_millis(800);
        let err = run_powershell_cancellable("Start-Sleep -Seconds 20", false, None, Duration::from_secs(60), &|| std::time::Instant::now() > cancel_at)
            .unwrap_err();
        assert!(err.contains("Stopped"), "{}", err);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    pub(crate) fn clip_keeps_the_tail() {
        let long = format!("{}\nFATAL: build failed\n[exit code 2]", "x".repeat(20_000));
        let clipped = clip_middle(&long, 6000);
        assert!(clipped.contains("FATAL: build failed") && clipped.ends_with("[exit code 2]"));
    }

    #[test]
    pub(crate) fn paths_resolve_to_real_folders() {
        let docs = dirs::document_dir().unwrap();
        assert_eq!(resolve_agent_path("Documents\\notes.md"), docs.join("notes.md"));
        assert_eq!(resolve_agent_path("~"), dirs::home_dir().unwrap());
        let via_env = resolve_agent_path("%USERPROFILE%\\x.txt");
        assert!(via_env.is_absolute() && !via_env.to_string_lossy().contains('%'), "{:?}", via_env);
        assert!(resolve_agent_path("helper.py").starts_with(agent_scratch_dir()));
    }

    #[test]
    pub(crate) fn destructive_filter_blocks_wipes_not_everyday_commands() {
        for ok in [
            "Get-Date -Format 'yyyy-MM-dd'",
            "npm run format",
            "rd /s /q node_modules",
            "Remove-Item -Recurse -Force .\\dist",
            "Write-Output 'shutdown later'",
        ] {
            assert!(is_destructive(ok).is_none(), "wrongly blocked: {}", ok);
        }
        for bad in ["format C:", "Remove-Item -Recurse -Force C:\\", "rd /s /q C:\\", "shutdown /s /t 0", "Stop-Computer", "rm -rf ~"] {
            assert!(is_destructive(bad).is_some(), "not blocked: {}", bad);
        }
    }

    #[test]
    pub(crate) fn keeps_non_ascii_output() {
        let out = run_powershell("Write-Output 'Muraho - café ✓'", false, None, Duration::from_secs(30)).unwrap();
        assert!(out.contains("café ✓"), "{}", out);
    }
}
