// Mouse AI desktop agent - which tools to put in front of the model for THIS task.
//
// Port of the browser agent's selectTools: the full catalog (31 built-ins + skills +
// MCP) is ~10 KB of JSON re-sent on every step, and offering download_file or
// delegate_to_browser on "rename these files" invites a wrong turn. A model can only
// call a declared function, so the core is deliberately broad and the hints generous;
// drive_loop widens to the full set once if the run stalls, errors repeatedly, or the
// model names a tool it was not offered (that call is executed anyway).

#[allow(unused_imports)]
use crate::agent::*;

pub(crate) const CORE_TOOLS: &[&str] = &[
    "update_plan",
    "run_command", "read_file", "write_file", "edit_file", "delete_file", "list_directory", "find_files", "search_in_files",
    "web_search", "web_fetch",
    "capture_screen", "get_screen_context", "list_windows", "focus_window", "read_app_content",
    "find_elements", "click_element", "set_value",
    "click", "double_click", "type_text", "press_key", "scroll", "wait", "open_url", "open_app",
    "request_confirmation",
];

/// Specialist tools and the task words that make them worth offering.
pub(crate) const TOOL_HINTS: &[(&[&str], &[&str])] = &[
    (&["download_file"], &["download", "save from", "installer", "zip", "pdf", "image", "fetch the file"]),
    (&["get_clipboard", "set_clipboard"], &["clipboard", "copy", "paste", "copied"]),
    (&["notify"], &["notify", "notification", "let me know", "alert", "when done", "when it finishes"]),
    (&["create_document"], &["docx", "word", "document", "report", "letter", "essay", "cv", "resume", "proposal", "memo", "write up", "write-up"]),
    (&["delegate_to_browser", "check_browser_task"], &[
        "browser", "website", "site", "log in", "login", "sign in", "form", "online", "web app", "http", "www", ".com",
        "order", "book", "buy", "post", "email", "gmail", "linkedin", "youtube", "search for", "look up",
    ]),
];

/// `available` is the full catalog (built-ins + skills + MCP). MCP tools and use_skill
/// are always kept: the user configured them on purpose.
pub(crate) fn select_tools(task: &str, available: &[Value], browser_connected: bool) -> Vec<Value> {
    let text = normalize_words(task);
    let mut wanted: std::collections::HashSet<&str> = CORE_TOOLS.iter().copied().collect();
    for (names, words) in TOOL_HINTS {
        if names.contains(&"delegate_to_browser") && !browser_connected {
            continue;
        }
        if has_any_phrase(&text, words) {
            for n in *names {
                wanted.insert(n);
            }
        }
    }
    let builtin: std::collections::HashSet<&str> = crate::agent::BUILTIN_TOOL_NAMES.iter().copied().collect();
    let picked: Vec<Value> = available
        .iter()
        .filter(|d| {
            let name = d.get("name").and_then(|n| n.as_str()).unwrap_or("");
            wanted.contains(name) || name == "use_skill" || !builtin.contains(name)
        })
        .cloned()
        .collect();
    // Never hand back a set so small the agent cannot work.
    if picked.len() >= CORE_TOOLS.len().min(available.len()) {
        picked
    } else {
        available.to_vec()
    }
}
