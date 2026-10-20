// Kudu Desktop agent - the built-in tool declarations (Gemini function schemas).
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

/// Gemini functionDeclarations for every local tool the agent can call.
/// Names of the built-in declarations below (skills and MCP tools are not in this list).
pub(crate) const BUILTIN_TOOL_NAMES: &[&str] = &[
    "run_command", "read_file", "write_file", "edit_file", "delete_file", "list_directory", "find_files", "search_in_files",
    "download_file", "web_search", "web_fetch", "watch_video", "capture_screen", "get_screen_context", "list_windows", "read_app_content",
    "focus_window", "find_elements", "click_element", "set_value",
    "click", "double_click", "scroll", "type_text", "press_key", "open_url", "open_app", "wait",
    "get_clipboard", "set_clipboard", "notify", "request_confirmation", "delegate_to_browser", "check_browser_task", "update_plan",
];

pub(crate) fn tool_declarations() -> Value {
    json!([
        {
            "name": "run_command",
            "description": "Run a Windows PowerShell command. Returns {ok, stdout, stderr, exit_code, timed_out, truncated, duration_ms}: ok=false means a non-zero exit or a timeout - read stderr first. Use background=true for long-running/servers so it doesn't block. For anything that takes more than a few seconds (installs, builds), pass timeout_seconds explicitly rather than letting it hit the default and fail - up to 300s foreground, or use background=true for anything longer/indefinite. IMPORTANT: only use this (or write_file) when no built-in tool already does the job - e.g. never write and run a script to search the web or fetch a page, web_search/web_fetch already do that directly.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The PowerShell command to run." },
                    "background": { "type": "boolean", "description": "Run detached and return immediately." },
                    "cwd": { "type": "string", "description": "Working directory to run the command in. Omit it ONLY for your own scratch/throwaway work - it then defaults to a private temp scratch folder, never a real project. Always pass the real absolute folder path when the task is about actual files (a project, the user's Desktop, etc.)." },
                    "timeout_seconds": { "type": "integer", "description": "How long to wait for foreground output before giving up, 1-300 (default 45). Ignored when background=true." }
                },
                "required": ["command"]
            }
        },
        {
            "name": "read_file",
            "description": "Read a file and return its text: plain text/code (UTF-8 or UTF-16), and Word/PowerPoint/Excel documents. Long files: page through them with offset/limit (line numbers). Paths: absolute, or Desktop\\…, Documents\\…, Downloads\\…, ~\\… for the user's real folders; any other relative path is your private scratch folder.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" }, "offset": { "type": "integer", "description": "First line to return (0-based)" }, "limit": { "type": "integer", "description": "How many lines to return" } }, "required": ["path"] }
        },
        {
            "name": "write_file",
            "description": "Create or overwrite a text file on disk. Only use this when the task genuinely needs a persisted file - never as a substitute for a tool that already exists (e.g. don't write a script to do what web_search/web_fetch/run_command can do directly). A relative path resolves inside your private scratch folder (fine for your own working files - delete_file it once you're done, unless the user actually wants it kept); pass an absolute path - the user's project, Desktop, wherever they mean - for anything they should end up seeing.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path, or Desktop\\…, Documents\\…, Downloads\\…, ~\\… for the user's real folders" },
                    "content": { "type": "string" },
                    "append": { "type": "boolean", "description": "Add to the end instead of replacing - write a long file in several parts" }
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "delete_file",
            "description": "Delete a file. Use this to clean up your own scratch files once you've used them and no longer need them - never on a file you haven't verified is your own throwaway work.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        },
        {
            "name": "list_directory",
            "description": "List the files and folders in a directory. A relative path resolves inside your private scratch folder.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        },
        {
            "name": "find_files",
            "description": "Search the PC for files/folders whose name contains a substring (case-insensitive), under a root directory. Use this to locate a file when you don't know its full path.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name_contains": { "type": "string", "description": "Substring to match in file/folder names." },
                    "root": { "type": "string", "description": "Directory to search under. Defaults to the user's home folder." }
                },
                "required": ["name_contains"]
            }
        },
        {
            "name": "web_search",
            "description": "Search the web and return titles, snippets, and URLs.",
            "parameters": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
        },
        {
            "name": "web_fetch",
            "description": "Fetch a URL and return its readable text content.",
            "parameters": { "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }
        },
        {
            "name": "watch_video",
            "description": "Have the model WATCH a public YouTube video (audio and frames) and return complete timestamped notes, a transcript, or the answer to a question - with MM:SS timestamps. This is the only way to know what a video says; never download a video, scrub through it or screenshot frames. Takes 20-90 seconds. The URL of the active browser tab is in its address bar (read_app_content / find_elements on the browser window).",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The YouTube URL (watch?v=, youtu.be, shorts)." },
                    "mode": { "type": "string", "enum": ["notes", "transcript", "answer"], "description": "notes = complete timestamped notes (default); transcript = the spoken words; answer = answer the question." },
                    "question": { "type": "string", "description": "What to look for or answer (optional for notes/transcript)." },
                    "start_sec": { "type": "number", "description": "Optional clip start, in seconds." },
                    "end_sec": { "type": "number", "description": "Optional clip end, in seconds." }
                },
                "required": ["url"]
            }
        },
        {
            "name": "capture_screen",
            "description": "Take a screenshot of the current screen so you can see the present state before acting.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "get_screen_context",
            "description": "Get the cursor position, the window under the cursor, the window that ACTUALLY has keyboard focus (these can differ - type_text/press_key always go to the focused one, not necessarily whatever's under the cursor), and the folder/element under the cursor.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "list_windows",
            "description": "List currently open top-level windows (title + process name + a coarse category - browser/office/editor/terminal/other - with the currently-focused one flagged). Use this to check whether a browser or app is already open (and what kind) before deciding to launch a new one.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "read_app_content",
            "description": "Read visible text/values out of whichever app currently has keyboard focus, via its accessibility tree - generalizes beyond File Explorer (which has its own real directory listing) to spreadsheets, documents, editors, and anything else that exposes accessible content, e.g. Excel cell values, a text editor's document, a list of emails. Use this instead of guessing from a screenshot alone when you need to know what's actually IN an open app.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "focus_window",
            "description": "Bring an already-open window to the foreground by a case-insensitive substring match on its title or process name (e.g. 'chrome', 'comet', 'notepad'). Prefer this over open_url/open_app when list_windows shows the window already exists - reusing it is more reliable than opening a duplicate. The result includes an after-screenshot - confirm the right window is now in front.",
            "parameters": {
                "type": "object",
                "properties": { "title_contains": { "type": "string", "description": "Substring to match against window titles/process names." } },
                "required": ["title_contains"]
            }
        },
        {
            "name": "click",
            "description": "Move the mouse to a point and click. x/y are normalized 0-1000 relative to the most recent screenshot's width/height (0,0=top-left, 1000,1000=bottom-right) - always base these on the LATEST screenshot. The result automatically includes a fresh after-screenshot: inspect it to verify the click did what you intended.",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "Horizontal position, 0-1000, normalized to screenshot width." },
                    "y": { "type": "integer", "description": "Vertical position, 0-1000, normalized to screenshot height." },
                    "button": { "type": "string", "enum": ["left", "right"] }
                },
                "required": ["x", "y"]
            }
        },
        {
            "name": "find_elements",
            "description": "List the controls of the foreground window through Windows UI Automation - buttons, text fields, checkboxes, menu items, list/tree items, tabs, links - each with an id (u1, u2…), role, name, current value, screen rectangle and what it supports (invoke/value/toggle/select/expand). query = words from the control's label/name (case-insensitive); role = button | edit | checkbox | combobox | menuitem | listitem | tab | link | text | document…; include_text=true also lists static text. Far more reliable than guessing coordinates from a screenshot: find the control, then click_element / set_value by id. Ids are valid until the window changes. Returns nothing for apps without accessibility (games, canvas apps) - use capture_screen + click there.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Text to match in the element's name/label/value (optional)." },
                    "role": { "type": "string", "description": "Control type filter (optional)." },
                    "limit": { "type": "integer", "description": "Max elements to return (default 40)." },
                    "include_text": { "type": "boolean", "description": "Also list static text and panes (for reading)." }
                }
            }
        },
        {
            "name": "click_element",
            "description": "Click a control by id (from find_elements) or by name (+ optional role) in the foreground window. action=click (default): a real mouse click at the element's own rectangle, re-found at click time so it lands even if the window moved. action=invoke activates through accessibility (works for off-screen items); toggle = checkbox/switch; select = list/tab item; expand = menu/combo box; focus = keyboard focus only. The result includes a fresh after-screenshot - verify the effect from it.",
            "parameters": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Element id from find_elements, e.g. u3." },
                    "name": { "type": "string", "description": "The control's name/label (used when no id, or if the id is stale)." },
                    "role": { "type": "string", "description": "Control type to disambiguate (optional)." },
                    "action": { "type": "string", "enum": ["click", "invoke", "toggle", "select", "expand", "focus"] }
                }
            }
        },
        {
            "name": "set_value",
            "description": "Set a text field's content by id (from find_elements) or name - through the accessibility Value pattern (instant, exact, no keystrokes); falls back to click + select-all + paste when the field has no Value pattern. Use it for forms, search boxes, address bars and dialog fields instead of click + type_text. The result includes a fresh after-screenshot.",
            "parameters": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "role": { "type": "string" },
                    "text": { "type": "string", "description": "The full new value." },
                    "format": { "type": "string", "enum": ["auto", "raw"], "description": "auto (default): Markdown becomes plain text (no #, **, ---); raw: exactly as given." }
                },
                "required": ["text"]
            }
        },
        {
            "name": "type_text",
            "description": "Type the given text at the current focus (e.g. into a focused form field). mode=auto (default) pastes multi-line, long or code text through the clipboard (restored afterwards) so editors do not auto-indent or auto-close it, and types short text with keystrokes; mode=type forces keystrokes (terminals, apps that intercept Ctrl+V; a newline presses Enter); mode=paste forces the clipboard. The result says which mode ran, includes verified=true/false where the app exposes its text, and a fresh after-screenshot: confirm YOUR text appeared in the RIGHT field before moving on.",
            "parameters": { "type": "object", "properties": { "text": { "type": "string" }, "mode": { "type": "string", "enum": ["auto", "type", "paste"], "description": "auto | type | paste" }, "format": { "type": "string", "enum": ["auto", "raw"], "description": "auto (default): Markdown is typed as plain text (no #, **, ---) except in code editors, terminals and Markdown sites; raw: type the text exactly as given." } }, "required": ["text"] }
        },
        {
            "name": "press_key",
            "description": "Press a key or combo, e.g. 'enter', 'tab', 'ctrl+c', 'ctrl+shift+t', 'alt+tab', 'f5', 'space', 'pagedown' - or a comma-separated sequence like 'ctrl+a, delete'. The result automatically includes a fresh after-screenshot - verify the effect from it.",
            "parameters": { "type": "object", "properties": { "keys": { "type": "string" } }, "required": ["keys"] }
        },
        {
            "name": "double_click",
            "description": "Double-click the left mouse button at a point (e.g. to open an item). x/y are normalized 0-1000 relative to the most recent screenshot, same as click.",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "0-1000, normalized to screenshot width." },
                    "y": { "type": "integer", "description": "0-1000, normalized to screenshot height." }
                },
                "required": ["x", "y"]
            }
        },
        {
            "name": "scroll",
            "description": "Scroll the mouse wheel at a point. x/y are normalized 0-1000 relative to the most recent screenshot, same as click. Positive amount scrolls up, negative scrolls down (a few notches per call). The result includes an after-screenshot showing what's now visible.",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "0-1000, normalized to screenshot width." },
                    "y": { "type": "integer", "description": "0-1000, normalized to screenshot height." },
                    "amount": { "type": "integer", "description": "Notches; negative = down." }
                },
                "required": ["x", "y", "amount"]
            }
        },
        {
            "name": "open_url",
            "description": "Open a URL in the user's default browser (use this to start a browser task before clicking/typing). The result includes an after-screenshot taken ~2s later - check whether the page has actually loaded in it before acting.",
            "parameters": { "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }
        },
        {
            "name": "open_app",
            "description": "Launch an application or open a file/folder by path or name (e.g. 'notepad', 'C:/Users/me/Desktop'). The result includes an after-screenshot taken ~2s later - check the app actually opened in it.",
            "parameters": { "type": "object", "properties": { "target": { "type": "string" } }, "required": ["target"] }
        },
        {
            "name": "wait",
            "description": "Pause briefly to let an app or web page finish loading. The result includes a fresh screenshot taken after the wait, so you can see the new state without a separate capture_screen call.",
            "parameters": { "type": "object", "properties": { "seconds": { "type": "number" } }, "required": ["seconds"] }
        },
        {
            "name": "edit_file",
            "description": "Change part of an existing text file by replacing an exact snippet - much faster and safer than rewriting the whole file with write_file. old_text must match the file exactly (including indentation) and be unique unless replace_all is true. Read the file first.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string", "description": "Exact text currently in the file." },
                    "new_text": { "type": "string", "description": "Replacement text." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence instead of requiring a unique match." }
                },
                "required": ["path", "old_text", "new_text"]
            }
        },
        {
            "name": "search_in_files",
            "description": "Search the CONTENTS of text files under a folder (like grep): returns matching lines as path:line: text. Skips node_modules, .git, target, dist and large/binary files. Use it to find where something is defined or used in a project.",
            "parameters": {
                "type": "object",
                "properties": {
                    "folder": { "type": "string", "description": "Absolute folder to search." },
                    "query": { "type": "string", "description": "Text to find (case-insensitive)." },
                    "extension": { "type": "string", "description": "Optional file extension filter, e.g. 'ts' or 'py'." }
                },
                "required": ["folder", "query"]
            }
        },
        {
            "name": "get_clipboard",
            "description": "Read the text currently on the user's clipboard.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "set_clipboard",
            "description": "Put text on the user's clipboard so they can paste it anywhere.",
            "parameters": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
        },
        {
            "name": "download_file",
            "description": "Download a URL straight to a file on disk (images, PDFs, datasets…). Faster and more reliable than doing it through a browser.",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "path": { "type": "string", "description": "Absolute destination file path." }
                },
                "required": ["url", "path"]
            }
        },
        {
            "name": "notify",
            "description": "Show the user a Windows notification - use it when a long task finishes or needs their attention while they may be in another app.",
            "parameters": {
                "type": "object",
                "properties": { "title": { "type": "string" }, "message": { "type": "string" } },
                "required": ["title", "message"]
            }
        },
        {
            "name": "request_confirmation",
            "description": "Show the user a real Allow/Deny button and WAIT for their decision before proceeding. Call this before sensitive or ambiguous actions: submitting a form with personal/payment info, sending a message, making a purchase, deleting/moving files, or anything you're not confident the user wants.",
            "parameters": {
                "type": "object",
                "properties": { "message": { "type": "string", "description": "A short, specific description of exactly what you're about to do." } },
                "required": ["message"]
            }
        },
        {
            "name": "delegate_to_browser",
            "description": "Delegate a WEB task to the connected Kudu Connect browser extension, which drives the page through the DOM (far more reliable than clicking a browser by screen coordinates). Use this for anything on a website: filling and submitting a form, reading/extracting content from a page, searching a site, clicking through a web flow. By default this waits for the browser to finish and returns its result. Set background=true to fire it off and keep working locally, then call check_browser_task later to collect the result.",
            "parameters": {
                "type": "object",
                "properties": {
                    "instruction": { "type": "string", "description": "A clear, self-contained description of what to do on the web, including the target site/URL and what 'done' looks like." },
                    "url": { "type": "string", "description": "Optional starting URL for the browser to open first." },
                    "context": { "type": "string", "description": "Optional data the browser agent should work from: file contents, values, names, text to type or post. The browser can't read the PC, so pass anything it needs here." },
                    "background": { "type": "boolean", "description": "If true, return a task_id immediately instead of waiting; collect the result later with check_browser_task." }
                },
                "required": ["instruction"]
            }
        },
        {
            "name": "check_browser_task",
            "description": "Wait for and collect the result of a browser task previously started with delegate_to_browser(background=true). Call this when you've finished the local work you could do in parallel and now need the browser's result.",
            "parameters": {
                "type": "object",
                "properties": { "task_id": { "type": "string", "description": "The task_id returned by delegate_to_browser." } },
                "required": ["task_id"]
            }
        },
        {
            // Handled by the loop itself (see `Ledger::apply_plan_update`), never
            // by execute_tool: it touches nothing. It's how the plan, definition
            // of done and key facts survive context compaction, and what the
            // completion check measures a "done" claim against.
            "name": "update_plan",
            "description": "Record your plan and progress (touches nothing on the PC). Call it in your FIRST response for any task needing more than ~3 actions - ordered steps plus a concrete, checkable done_when - then again whenever you finish a step (completed=N), change approach, or learn something you must not forget (notes survive even when old tool results are trimmed). Call it alongside your next real action, not on its own.",
            "parameters": {
                "type": "object",
                "properties": {
                    "plan": { "type": "array", "items": { "type": "string" }, "description": "Ordered steps (send the full list whenever it changes)." },
                    "done_when": { "type": "string", "description": "Concrete, checkable definition of done, e.g. 'npm test exits 0' or 'the new note is visible in the list'." },
                    "completed": { "type": "integer", "description": "How many plan steps are fully done so far." },
                    "notes": { "type": "string", "description": "Key facts/data gathered that you'll need later (paths, ports, values, URLs)." }
                }
            }
        }
    ])
}
