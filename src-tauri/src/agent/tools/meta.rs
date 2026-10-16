// Kudu Desktop agent - tool classes, failure detection, recovery hints, labels.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

/// Tools whose success means something on the PC (or a website) actually
/// changed - the evidence a "created / fixed / sent" claim needs behind it.
pub(crate) fn is_effect_tool(name: &str) -> bool {
    matches!(
        name,
        "write_file"
            | "edit_file"
            | "delete_file"
            | "download_file"
            | "run_command"
            | "click_element" | "set_value" | "click"
            | "double_click"
            | "type_text"
            | "press_key"
            | "open_url"
            | "open_app"
            | "set_clipboard"
            | "delegate_to_browser"
            | "check_browser_task"
            | "create_document"
    )
}

/// run_command calls that only look (list, read, status) - they don't change anything.
pub(crate) fn is_read_only_command(name: &str, args: &Value) -> bool {
    if name != "run_command" {
        return false;
    }
    let cmd = args.get("command").and_then(|c| c.as_str()).unwrap_or("").trim().to_lowercase();
    let first = cmd.split(|c: char| c.is_whitespace() || c == ';' || c == '|').next().unwrap_or("");
    let looks = [
        "get-childitem", "gci", "ls", "dir", "get-content", "gc", "cat", "type", "test-path", "get-item",
        "select-string", "get-command", "where.exe", "where", "get-process", "get-date", "echo", "write-output",
        "resolve-path", "get-location", "pwd", "whoami", "hostname", "systeminfo",
        // POSIX
        "find", "grep", "rg", "head", "tail", "wc", "which", "ps", "date", "uname", "df", "du", "stat",
        "file", "env", "printenv", "id", "uptime", "tree", "less", "more", "sw_vers", "lsb_release",
    ];
    if looks.contains(&first) {
        return true;
    }
    ["git status", "git log", "git diff", "git show", "git branch", "node -v", "node --version", "npm -v", "python --version", "py --version"]
        .iter()
        .any(|p| cmd.starts_with(p))
}

/// Tools whose value is the information they return (new info = progress).
pub(crate) fn is_data_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "list_directory"
            | "find_files"
            | "search_in_files"
            | "web_search"
            | "web_fetch"
            | "read_app_content"
            | "get_screen_context"
            | "list_windows"
            | "get_clipboard"
            | "delegate_to_browser"
            | "check_browser_task"
            | "run_command"
    )
}

/// Legitimately repetitive tools - left to the stall limit, never counted as a loop.
pub(crate) fn is_loop_exempt(name: &str) -> bool {
    matches!(name, "wait" | "check_browser_task" | "request_confirmation" | "update_plan")
}

/// The text a tool reported (error message, command output or result).
pub(crate) fn response_text(response: &Value) -> String {
    if let Some(v) = response.get("error") {
        return v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
    }
    // run_command: stderr carries the verdict, stdout the detail.
    if response.get("stdout").is_some() || response.get("stderr").is_some() {
        let stderr = response.get("stderr").and_then(|v| v.as_str()).unwrap_or("").trim();
        let stdout = response.get("stdout").and_then(|v| v.as_str()).unwrap_or("").trim();
        return match (stderr.is_empty(), stdout.is_empty()) {
            (false, false) => format!("{}\n{}", stderr, stdout),
            (false, true) => stderr.to_string(),
            (true, false) => stdout.to_string(),
            (true, true) => "(no output)".to_string(),
        };
    }
    for key in ["output", "result"] {
        if let Some(v) = response.get(key) {
            return v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
        }
    }
    response.to_string()
}

/// A tool "failed" if it returned an error - or, for run_command, if the
/// command itself exited non-zero (run_powershell appends `[exit code N]`).
pub(crate) fn tool_failed(name: &str, response: &Value) -> bool {
    if response.get("error").is_some() {
        return true;
    }
    if name == "run_command" {
        // Structured result: `ok` is false on a non-zero exit or a timeout.
        if let Some(ok) = response.get("ok").and_then(|v| v.as_bool()) {
            return !ok;
        }
        if response.get("started").and_then(|v| v.as_bool()) == Some(true) {
            return false;
        }
        if let Some(code) = response.get("exit_code").and_then(|c| c.as_i64()) {
            return code != 0;
        }
        if let Some(out) = response.get("output").and_then(|o| o.as_str()) {
            let tail = out.trim_end();
            if let Some(idx) = tail.rfind("[exit code ") {
                return tail.ends_with(']') && !tail[idx..].starts_with("[exit code 0]");
            }
        }
    }
    false
}

/// Recovery advice attached to a failed result, keyed off the error's shape.
/// The model reads it right where it reads the failure - which is where advice
/// changes what it does next.
pub(crate) fn recovery_hint(name: &str, error: &str) -> Option<&'static str> {
    let e = error.to_lowercase();
    if e.contains("refused: this looks like") {
        return Some("This command is blocked as destructive. Use a narrower command that targets exactly the files meant, or ask the user to do it themselves.");
    }
    if e.contains("is not recognized as the name of a cmdlet")
        || e.contains("is not recognized as an internal or external command")
        || e.contains("commandnotfoundexception")
    {
        return Some("That program isn't installed or isn't on PATH. Check with `Get-Command <name>` / `where.exe <name>`, call it by its full path, or use an alternative (npx <tool>, py -m <module>, a winget install if the user wants it).");
    }
    if e.contains("is not a valid statement separator") || (e.contains("unexpected token") && e.contains("&&")) {
        return Some("This is Windows PowerShell 5.1: '&&' and '||' don't exist. Use ';' between commands, and check a native command with `if ($LASTEXITCODE -ne 0) { ... }`.");
    }
    if e.contains("unexpected token") || e.contains("missing closing") || e.contains("missing terminator") {
        return Some("PowerShell syntax error. Quote paths with spaces, run an exe at a quoted path with the call operator (& \"C:\\path with spaces\\tool.exe\" args), escape $ inside double quotes (or use single quotes), and use PowerShell - not cmd or bash - syntax.");
    }
    if e.contains("os error 206") || e.contains("filename or extension is too long") {
        return Some("The command line is too long. write_file the content (or a .ps1 script) first, then run that file.");
    }
    if e.contains("timed out") {
        return Some("It ran past the timeout. For a slow-but-finite command pass a larger timeout_seconds (max 300); for servers/watchers use background=true and read_file the log it writes. It may have partly run - check the current state before re-running.");
    }
    if e.contains("cwd \"") && e.contains("doesn't exist") {
        return Some("Pass an existing ABSOLUTE folder as cwd - the project folder from the task's [Project folder: …] prefix, or verify the path with list_directory first.");
    }
    if e.contains("old_text was not found") {
        return Some("read_file the file again and copy old_text exactly from it (indentation included) - or use a smaller snippet that is unique in the file.");
    }
    if e.contains("old_text appears") {
        return Some("Include more surrounding lines so old_text is unique, or set replace_all=true if every occurrence should change.");
    }
    if e.contains("being used by another process") || e.contains("os error 32") {
        return Some("The file is locked by another process (often a running server, build or editor). Stop that process or work on a copy.");
    }
    if e.contains("access is denied") || e.contains("permission denied") || e.contains("os error 5") || e.contains("unauthorizedaccess") {
        return Some("Permission problem: the file may be read-only, open elsewhere, or need admin rights. Use a user-writable location or tell the user what needs elevated access.");
    }
    if e.contains("cannot find path")
        || e.contains("cannot find the path")
        || e.contains("cannot find the file")
        || e.contains("no such file")
        || e.contains("os error 2")
        || e.contains("os error 3")
        || e.contains("does not exist")
    {
        return Some("That path doesn't exist. Check the parent folder with list_directory, or locate the file with find_files (by name) / search_in_files (by content). Use absolute paths under the project folder - relative paths resolve to your private scratch folder.");
    }
    if e.contains("no browser extension is connected") {
        return Some("No browser extension is connected. For read-only info use web_search/web_fetch; to act on a site use open_url, then click/type and verify each step from the screenshots.");
    }
    if e.contains("unknown tool") {
        return Some("Use only the tools you were given.");
    }
    match name {
        "run_command" => Some("The command failed. Read the error output, fix the cause (missing dependency, wrong folder, PowerShell vs. bash syntax, bad arguments) and then re-run - don't repeat it unchanged."),
        "web_search" | "web_fetch" => Some("Try once with a different query or URL. If a site blocks fetching, delegate_to_browser can read it in a real browser."),
        "download_file" => Some("Check the URL is a direct file link (web_search for the right one), or use run_command with curl.exe -L as a fallback."),
        "click" | "double_click" | "scroll" | "type_text" | "press_key" | "focus_window" => Some("Take a fresh capture_screen, re-locate the target, or reach the same result another way (keyboard shortcut, focus_window, a command, a direct URL)."),
        _ => None,
    }
}

// ── Completion check ─────────────────────────────────────────────
// "Done!" used to be accepted at face value - a model whose last command
// failed, or that never actually wrote/clicked anything, could still close the
// task with a success message. Before accepting a final answer it's held to
// evidence (at most MAX_COMPLETION_REVIEWS times, only while budget remains).

pub(crate) fn tool_label(name: &str, args: &Value) -> String {
    match name {
        "run_command" => format!("$ {}", args.get("command").and_then(|c| c.as_str()).unwrap_or("")),
        "read_file" => format!("Reading {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "edit_file" => format!("Editing {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "search_in_files" => format!("Searching for \"{}\"", truncate(args.get("query").and_then(|c| c.as_str()).unwrap_or(""), 60)),
        "get_clipboard" => "Reading the clipboard".to_string(),
        "set_clipboard" => "Copying to the clipboard".to_string(),
        "download_file" => format!("Downloading to {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "notify" => "Notifying you".to_string(),
        "write_file" => format!("Writing {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "delete_file" => format!("Deleting {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "list_directory" => format!("Listing {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("")),
        "find_files" => format!("Finding files: {}", args.get("name_contains").and_then(|c| c.as_str()).unwrap_or("")),
        "web_search" => format!("Searching: {}", args.get("query").and_then(|c| c.as_str()).unwrap_or("")),
        "web_fetch" => format!("Fetching {}", args.get("url").and_then(|c| c.as_str()).unwrap_or("")),
        "watch_video" => format!("Watching the video: {}", args.get("url").and_then(|c| c.as_str()).unwrap_or("")),
        "capture_screen" => "Looking at the screen".to_string(),
        "get_screen_context" => "Reading screen context".to_string(),
        "list_windows" => "Listing open windows".to_string(),
        "focus_window" => format!("Focusing window: {}", args.get("title_contains").and_then(|c| c.as_str()).unwrap_or("")),
        "read_app_content" => "Reading app content".to_string(),
        "click" => format!(
            "Clicking ({}, {})",
            args.get("x").and_then(|v| v.as_i64()).unwrap_or(0),
            args.get("y").and_then(|v| v.as_i64()).unwrap_or(0)
        ),
        "double_click" => format!(
            "Double-clicking ({}, {})",
            args.get("x").and_then(|v| v.as_i64()).unwrap_or(0),
            args.get("y").and_then(|v| v.as_i64()).unwrap_or(0)
        ),
        "scroll" => format!("Scrolling {}", args.get("amount").and_then(|v| v.as_i64()).unwrap_or(0)),
        "type_text" => format!("Typing: {}", truncate(args.get("text").and_then(|c| c.as_str()).unwrap_or(""), 60)),
        "press_key" => format!("Pressing {}", args.get("keys").and_then(|c| c.as_str()).unwrap_or("")),
        "open_url" => format!("Opening {}", args.get("url").and_then(|c| c.as_str()).unwrap_or("")),
        "open_app" => format!("Opening {}", args.get("target").and_then(|c| c.as_str()).unwrap_or("")),
        "wait" => format!("Waiting {}s", args.get("seconds").and_then(|v| v.as_f64()).unwrap_or(0.0)),
        "delegate_to_browser" => format!(
            "🌐 Browser: {}",
            truncate(args.get("instruction").and_then(|c| c.as_str()).unwrap_or(""), 70)
        ),
        "check_browser_task" => "Collecting browser result".to_string(),
        "use_skill" => format!("Using the {} playbook", args.get("name").and_then(|c| c.as_str()).unwrap_or("")),
        "create_document" => format!("Creating {}", args.get("path").and_then(|c| c.as_str()).unwrap_or("a Word document")),
        other => other.to_string(),
    }
}
