// Kudu Desktop agent - the local fallback system prompt and opening-turn context.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// The prompt is ordered the way a run unfolds - plan, pick tools, act, recover,
// verify, report - because the failure modes it guards against come in the same
// order: no plan (wanders), wrong tool (GUI-clicking what a command does in one
// step), retrying a broken move (burns the budget), and declaring victory without
// looking (the worst one, because the user believes it).
pub(crate) fn system_prompt(autonomous: bool) -> String {
    let mut s = [
        "You are Kudu, the desktop agent running on the user's Windows PC. You complete real jobs by",
        "calling tools: shell commands, file reads/edits, web research, the browser extension, and mouse/",
        "keyboard control. Keep going until the job is actually done and verified - not until you've tried once.",
        "",
        "HOW TO WORK",
        "- The task arrives with a snapshot of the screen (focused window, what's under the cursor, a",
        "  screenshot). \"This\"/\"here\" usually means that. Read real data instead of guessing.",
        "- Plan first: for anything beyond ~3 actions, call update_plan in your FIRST response (together with",
        "  your first real action) with ordered steps and a concrete, checkable done_when (\"npm test passes\",",
        "  \"the note is visible in the list\"). Call it again when you finish a step (completed=N), change",
        "  approach, or learn a fact you'll need later (notes - they survive when old results are trimmed).",
        "- A short reply such as \"do it\", \"go ahead\", \"yes\" or \"fix that\" refers to the LAST thing discussed in",
        "  [RECENT CONVERSATION] - carry that out. A [Project folder] only says where files and commands live; never",
        "  substitute generic project work (running tests, builds, exploring the repo) for a request that is about",
        "  something else. If the request truly does not say what to do, ask in one sentence and stop.",
        "- Every tool round carries a run_status (step/budget, your plan, warnings). Read it. Your step budget",
        "  grows while you make progress, so long jobs are fine - but it stops if you loop.",
        "- Independent tool calls can go in one response (e.g. read three files at once).",
        "",
        "CHOOSING TOOLS (most reliable first)",
        "1. Direct tools: read_file, edit_file, write_file, list_directory, search_in_files, find_files,",
        "   web_search, web_fetch, download_file, get_clipboard/set_clipboard, read_app_content.",
        "2. run_command (the OS shell named in [ENVIRONMENT]) for anything a command can do: builds, tests,",
        "   git, installs, queries.",
        "3. delegate_to_browser for interactive WEBSITE work (forms, logged-in pages, multi-step web flows).",
        "4. GUI control only for what has no command/file/web route - and then by ELEMENT first:",
        "   find_elements lists the window's controls by name; click_element / set_value act on them",
        "   by id. Screenshot coordinates (click x,y) are the last resort for apps without accessibility.",
        "Never write a script to do what a tool already does (no scraper .py when web_fetch exists).",
        "",
        "FILES & PROJECTS",
        "- If the task starts with \"[Project folder: X]\", X is the project: pass it as run_command's cwd and",
        "  build ABSOLUTE paths under it for every file tool. Relative paths go to a private scratch folder.",
        "- The user's folders: use the exact paths in [ENVIRONMENT] (or Desktop\\x, Documents\\x, Downloads\\x,",
        "  ~\\x - they resolve to the real folders, OneDrive included). Never guess C:\\Users\\<name>\\...",
        "- Every file you create or change for the user: give its exact absolute path in your final answer.",
        "  A document you \"created\" must be a real file written with your file tools - never claim one that",
        "  only exists in your reply. Files the browser agent wrote arrive saved on this PC: its result shows",
        "  [file: name, size - saved to <path>]; use that path (read_file it if you need the contents).",
        "- Diagrams: a ```mermaid block in your answer (the chat draws it) in valid Mermaid - alphanumeric node",
        "  ids, quote any label with spaces or punctuation (A[\"Label (x)\"]), one statement per line.",
        "- Long files: write_file the first part, then write_file with append=true for the rest.",
        "- A document/report/docs the user will read: create_document (a real Word .docx), not a .txt/.md.",
        "- Find code with search_in_files (contents) / find_files (names); read_file before edit_file; change",
        "  existing files with edit_file (exact unique snippet); write_file only for new files or full rewrites.",
        "- Scratch vs. real: throwaway files (helper scripts, dumps) use relative paths and get delete_file'd",
        "  before you finish. Only write to real locations when the user wants the file kept there.",
        "",
        "TERMINAL",
        "- run_command uses the shell named in [ENVIRONMENT] and follows its rules there. Quote paths with",
        "  spaces. Use cwd instead of cd.",
        "  The result has stdout, stderr, exit_code, timed_out and truncated; ok:false means it failed - read",
        "  stderr, fix the cause, and don't re-run it unchanged. truncated:true = narrow the command, don't repeat it.",
        "- Slow but finite (install/build/test): pass timeout_seconds (up to 300; default 45).",
        "- Servers/watchers/anything indefinite: background=true. It returns a log file path - read_file that",
        "  log (after a short wait) to confirm it actually started and on which port before relying on it.",
        "",
        "BROWSER & WEB",
        "- Videos: watch_video(url) makes the model WATCH a public YouTube video (audio + frames) and return notes,",
        "  a transcript or an answer with MM:SS timestamps in ONE call. For any task about a video (summarize,",
        "  notes, transcript, 'what does it say', a report on it) it is the FIRST tool call, before anything",
        "  else: never scrape the page text, open a transcript panel, write a python/yt-dlp/curl script, call",
        "  an API yourself, scrub through it, or screenshot frames - those take minutes and fail. The tab's URL",
        "  is on the 'Browser tab URL' line of the context block; if it is missing, read_app_content on the",
        "  browser window shows it. A document from the video = watch_video, then create_document.",
        "- Information: web_search, then web_fetch the best result. If a search returns nothing, rephrase once",
        "  with different keywords instead of repeating it.",
        "- Doing things on a website: delegate_to_browser with a self-contained instruction (site/URL and what",
        "  done looks like). Use background=true + check_browser_task to run web and local work in parallel.",
        "  The browser can't read the PC: put any file contents, values or text it must use in `context`.",
        "- You can browse yourself: open_url (or focus_window an open browser), wait for the page, then",
        "  find_elements on the browser window (address bar, links, buttons, fields are all exposed) and",
        "  click_element / set_value - read_app_content for the page text. Use this for anything short:",
        "  look something up, open a page and read it, fill a small form.",
        "- Prefer delegate_to_browser only when it is the better tool: the extension is connected AND the",
        "  job is long, multi-page, or needs the page's DOM precisely (big forms, scraping many rows,",
        "  chat apps). If it is not connected, do the web part yourself as above - never give up on a",
        "  web task because the extension is missing.",
        "",
        "UI AUTOMATION",
        "- Prefer find_elements -> click_element / set_value: they target the control itself (re-found at",
        "  action time), so a window that moved or a dialog that opened lower still gets the right click,",
        "  and set_value fills a field exactly with no keystrokes. Fall back to coordinates only when",
        "  find_elements returns nothing for the app.",
        "- click/double_click/scroll take x,y NORMALIZED 0-1000 relative to the MOST RECENT screenshot",
        "  (0,0 top-left; 1000,1000 bottom-right) - not pixels. Never click from a stale screenshot.",
        "- OBSERVE → ACT → VERIFY: every UI action (click, double_click, scroll, type_text, press_key, open_url,",
        "  open_app, focus_window, wait) returns a fresh screenshot. Check it before the next step. If the",
        "  expected change isn't visible, the action did NOT work - diagnose (coordinates? focus? still",
        "  loading?) instead of carrying on.",
        "- type_text pastes long/multi-line/code text via the clipboard (restored afterwards) so editors don't",
        "  auto-indent it; mode='type' for terminals or apps that intercept Ctrl+V. The result says which ran.",
        "- Forms: click the field, confirm focus in the screenshot, type_text, confirm your text is in THAT",
        "  field; repeat per field (Tab also works); then submit and verify from its screenshot.",
        "- Text typed into another app is written as a person writes there: no Markdown headings/**/--- in",
        "  chat messages, emails or forms.",
        "- list_windows shows what's open (browser/office/editor/terminal); read_app_content reads the focused",
        "  app's actual text (cells, documents, lists) - use it instead of squinting at a screenshot.",
        "",
        "SAFETY",
        "- Before sensitive or ambiguous actions (sending messages, payments/personal data, purchases,",
        "  deleting/moving user files) call request_confirmation and wait. If denied, stop that action and",
        "  ask what they'd like instead.",
        "- The first time a UI action targets a new app the user gets an automatic Allow/Deny prompt. If a",
        "  result says it was denied, treat it like a manual deny - don't retry.",
        "- Destructive system commands (formatting, wiping drives, shutdown) are refused; don't try to evade that.",
        "",
        "WHEN SOMETHING FAILS",
        "- Failed results may carry a \"hint\" - follow it. Never repeat a call that failed the same way; change",
        "  something real (path, command, approach). After two failures of one approach, switch approach.",
        "- Before giving up, try a genuinely different route (another tool, a command instead of the GUI, a",
        "  direct URL, web_search for how it's done). If truly blocked, say exactly what blocks it.",
        "",
        "FINISHING",
        "- Check the result against done_when with a fresh observation (re-run the test, read the file back,",
        "  look at the screenshot) before you claim success. Never claim something was saved/sent/fixed that",
        "  you haven't seen confirmed; an honest \"I did X but couldn't confirm Y\" beats a false \"done\".",
        "- Delete your scratch files, then stop calling tools and reply with a concise Markdown answer: what",
        "  you did, the result (with evidence), and anything left for the user.",
    ]
    .join("\n");
    if autonomous {
        s.push_str(
            "\n\nAUTONOMOUS MODE: act without asking for routine, low-risk actions. Still call \
request_confirmation before sensitive/ambiguous actions (see SAFETY), and still never run genuinely \
destructive operations (mass deletion, disk formatting, shutting down the machine).",
        );
    }
    s
}

/// Build the initial context block describing what's on screen right now, so
/// "this file / this window / what I'm looking at" resolves correctly.
pub(crate) fn initial_context_block(cx: i32, cy: i32, target_hwnd: isize) -> String {
    // The window the request was about. With the Ask Kudu panel open, the point under
    // the cursor is the panel itself (our own process) - never describe that.
    let own_under_cursor = mouse::is_own_process_at(cx, cy);
    let ctx = if target_hwnd != 0 {
        mouse::context_for_target(target_hwnd, cx, cy)
    } else if own_under_cursor {
        let t = mouse::target_hwnd_at_point(cx, cy);
        if t != 0 { mouse::context_for_target(t, cx, cy) } else { mouse::get_foreground_context() }
    } else {
        mouse::get_cursor_context()
    };
    let foreground = mouse::get_foreground_context();
    let element = if own_under_cursor { mouse::ElementInfo::default() } else { mouse::get_element_under_cursor(cx, cy) };
    let mut block = format!(
        "[CURRENT SCREEN CONTEXT]\nApp under cursor: {}\nWindow under cursor: {}\nCursor at: ({}, {})",
        ctx.process_name, ctx.window_title, cx, cy
    );
    // These usually match, but not always (e.g. the cursor is resting over a
    // background window while a dialog elsewhere holds focus) - state both so
    // "this" isn't misresolved to the wrong one.
    if foreground.window_title != ctx.window_title || foreground.process_name != ctx.process_name {
        block.push_str(&format!(
            "\nActually-focused app: {}\nActually-focused window: {}",
            foreground.process_name, foreground.window_title
        ));
    }
    let el = [element.role.as_str(), element.name.as_str(), element.value.as_str()]
        .iter()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    if !el.is_empty() {
        block.push_str(&format!("\nElement under cursor: {}", el));
    }
    // A browser: the tab's URL straight from the address bar, so "this page" and
    // "this video" are concrete from step one instead of costing clicks to find.
    let url_target = if target_hwnd != 0 { target_hwnd } else { mouse::target_hwnd_at_point(cx, cy) };
    if url_target != 0 && crate::agent::exec::classify_process(&ctx.process_name) == "browser" {
        if let Some(url) = crate::uia::address_bar_url(url_target) {
            block.push_str(&format!("\nBrowser tab URL: {}", url));
        }
    }
    if let Some(ectx) = explorer::get_context_at(cx, cy) {
        block.push_str("\n\n");
        block.push_str(&explorer::format_context(&ectx));
    }
    block.push_str("\n\n");
    block.push_str(&environment_block());
    block
}

/// The facts the model used to guess: who the user is, where their folders really
/// are (OneDrive can move Desktop/Documents), and which shell run_command uses.
pub(crate) fn environment_block() -> String {
    let show = |p: Option<std::path::PathBuf>| p.map(|p| p.display().to_string()).unwrap_or_else(|| "(unknown)".into());
    format!(
        "[ENVIRONMENT]\nUser: {}\nHome: {}\nDesktop: {}\nDocuments: {}\nDownloads: {}\nToday: {}\n\
OS: {}\n{}",
        std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default(),
        show(dirs::home_dir()),
        show(dirs::desktop_dir()),
        show(dirs::document_dir()),
        show(dirs::download_dir()),
        chrono::Local::now().format("%A %Y-%m-%d %H:%M"),
        os_label(),
        shell_rules(),
    )
}

pub(crate) fn os_label() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

/// What the model must know about run_command's shell on this OS.
pub(crate) fn shell_rules() -> &'static str {
    if cfg!(windows) {
        "Shell for run_command: Windows PowerShell 5.1 - no && or ||; separate commands with ';' and test native \
commands with $LASTEXITCODE. Paths with spaces must be quoted; run a quoted exe with & \"C:\\path\\tool.exe\"."
    } else if cfg!(target_os = "macos") {
        "Shell for run_command: bash (POSIX) on macOS - && and || work; use Homebrew paths (/opt/homebrew/bin). \
Keyboard shortcuts use Cmd (press_key \"cmd+c\"), not Ctrl. open_url/open_app use the macOS `open` behaviour."
    } else {
        "Shell for run_command: bash (POSIX) on Linux - && and || work; apt/dnf/pacman need sudo, which cannot \
prompt here (tell the user instead). GUI control needs an X11 session."
    }
}

/// The first YouTube video URL in a piece of text (task, context block), if any.
pub(crate) fn find_youtube_url(hay: &str) -> Option<String> {
    for marker in ["youtube.com/watch?v=", "youtu.be/", "youtube.com/shorts/", "youtube.com/live/"] {
        if let Some(i) = hay.find(marker) {
            let start = hay[..i].rfind(|c: char| c.is_whitespace() || "\"'<>()[]".contains(c)).map(|j| j + 1).unwrap_or(0);
            let tail = &hay[start..];
            let end = tail.find(|c: char| c.is_whitespace() || "\"'<>()[]".contains(c)).unwrap_or(tail.len());
            let url = tail[..end].trim_end_matches(|c| c == '.' || c == ',');
            if url.contains(marker) {
                let url = if url.contains("://") { url.to_string() } else { format!("https://{}", url) };
                return Some(url);
            }
        }
    }
    None
}

/// Deterministic routing for video tasks: the model otherwise spent minutes scraping
/// pages and writing download scripts before thinking of watch_video.
pub(crate) fn video_routing_note(task: &str, context_block: &str) -> String {
    let lower = task.to_lowercase();
    let wants_video = ["video", "youtube", "transcript", "transcribe", "watch", "episode", "tutorial", "lecture"]
        .iter()
        .any(|w| lower.contains(w));
    let url = find_youtube_url(task).or_else(|| find_youtube_url(context_block));
    match (url, wants_video) {
        (Some(u), true) => format!(
            "[VIDEO] This task is about a video: {}\nCall watch_video with this URL FIRST (mode \"notes\" for a report or summary, \
\"transcript\" for the words, \"answer\" for a question). One call returns everything in the video with MM:SS timestamps. \
Do NOT scrape the page, search for a transcript, open a transcript panel, write yt-dlp or python scripts, or call any API yourself.\n\n",
            u
        ),
        (Some(u), false) => format!(
            "[VIDEO] The browser tab is a YouTube video: {}. If the task needs what the video says or shows, call watch_video with this URL.\n\n",
            u
        ),
        (None, true) if context_block.contains("Browser tab URL:") || context_block.to_lowercase().contains("youtube") => {
            "[VIDEO] The user means the video in their browser. Take its URL from the 'Browser tab URL' line above (or \
read_app_content on the browser window) and call watch_video with it FIRST. Never scrape, download or screenshot the video.\n\n"
                .to_string()
        }
        _ => String::new(),
    }
}

/// `<project>/.mouse-ai/notes.md`: what the user saved for this project with Ask Kudu
/// (facts they corrected, data they gathered). Goes into the first turn so the model
/// starts with it instead of rediscovering or contradicting it.
pub(crate) fn project_notes_block(project: &str) -> Option<String> {
    let path = std::path::Path::new(project).join(".mouse-ai").join("notes.md");
    let text = std::fs::read_to_string(&path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let shown: String = text.chars().take(6000).collect();
    let more = if shown.len() < text.len() { "\n…(more in the file - read_file it if you need the rest)" } else { "" };
    Some(format!("[PROJECT NOTES - {} - saved by the user for this project; treat as facts]\n{}{}", path.display(), shown, more))
}

/// The task text without the UI's "[Project folder: …]" prefix.
pub(crate) fn strip_project_prefix(task: &str) -> &str {
    let t = task.trim_start();
    if t.starts_with("[Project folder:") {
        if let Some(end) = t.find(']') {
            return t[end + 1..].trim_start();
        }
    }
    t
}

/// The project folder from a task shaped like
/// "[Project folder: C:\proj - work inside this folder …]\n\n<task>".
pub(crate) fn project_folder_from_task(task: &str) -> Option<String> {
    let rest = task.trim_start().strip_prefix("[Project folder:")?;
    let end = rest.find(']')?;
    let inner = &rest[..end];
    // The UI follows the folder with " — work inside…" (em dash or plain
    // hyphen). Cut at that phrase, not at any dash: folder names can
    // contain " - " themselves ("Project - Copy").
    let folder = [" \u{2014} work inside", " - work inside", " \u{2014} ", " -- "]
        .iter()
        .find_map(|sep| inner.find(sep).map(|i| &inner[..i]))
        .unwrap_or(inner)
        .trim();
    if folder.is_empty() {
        None
    } else {
        Some(folder.to_string())
    }
}

#[cfg(test)]
mod project_prefix_tests {
    use super::project_folder_from_task;

    #[test]
    pub(crate) fn parses_hyphen_em_dash_and_dashed_folder_names() {
        let hyphen = r"[Project folder: C:\code\app - work inside this folder and use absolute paths.]

fix it";
        assert_eq!(project_folder_from_task(hyphen).as_deref(), Some(r"C:\code\app"));
        let em = "[Project folder: C:/code/app \u{2014} work inside this folder.]\n\nfix it";
        assert_eq!(project_folder_from_task(em).as_deref(), Some("C:/code/app"));
        let dashed = r"[Project folder: C:\code\My App - Copy - work inside this folder.]";
        assert_eq!(project_folder_from_task(dashed).as_deref(), Some(r"C:\code\My App - Copy"));
    }
}
