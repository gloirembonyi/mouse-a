// Mouse AI desktop agent - web search/fetch and browser delegation.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

pub(crate) async fn web_search_tool(backend: &BackendClient, args: &Value) -> ToolOutcome {
    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
    if query.is_empty() {
        return ToolOutcome::err("no query provided");
    }
    match backend.agent_search(query).await {
        Ok(v) => {
            let results = v.get("results").cloned().unwrap_or_else(|| json!([]));
            let n = results.as_array().map(|a| a.len()).unwrap_or(0);

            // Snippets from keyless search are thin (often just the title), so
            // fetch the top result's real text to ground the answer. Best-effort
            // and quota-free (the fetch endpoint doesn't call Gemini).
            let mut response = json!({ "results": results });
            if let Some(top_url) = results
                .as_array()
                .and_then(|a| a.first())
                .and_then(|r| r.get("url"))
                .and_then(|u| u.as_str())
            {
                if let Ok(text) = backend.agent_fetch(top_url).await {
                    if !text.trim().is_empty() {
                        response["top_result_url"] = json!(top_url);
                        response["top_result_excerpt"] = json!(truncate(&text, 2500));
                    }
                }
            }

            ToolOutcome {
                response,
                image_data_url: None,
                log: format!("{} results for \"{}\"", n, query),
            }
        }
        Err(e) => ToolOutcome::err(e),
    }
}

pub(crate) async fn web_fetch_tool(backend: &BackendClient, args: &Value) -> ToolOutcome {
    let url = args.get("url").and_then(|u| u.as_str()).unwrap_or("");
    if url.is_empty() {
        return ToolOutcome::err("no url provided");
    }
    match backend.agent_fetch(url).await {
        Ok(text) => ToolOutcome {
            response: json!({ "text": truncate(&text, MAX_TOOL_OUTPUT) }),
            image_data_url: None,
            log: format!("fetched {} ({} chars)", url, text.len()),
        },
        Err(e) => ToolOutcome::err(e),
    }
}

/// Hand a web task to the connected browser extension over the local bridge.
/// Inline by default (waits for the result); background=true returns a task_id
/// so the desktop agent can keep working and collect it later via
/// check_browser_task - that's how desktop + browser run in parallel.
pub(crate) async fn delegate_to_browser_tool(source: &str, args: &Value) -> ToolOutcome {
    let instruction = args.get("instruction").and_then(|v| v.as_str()).unwrap_or("").trim();
    if instruction.is_empty() {
        return ToolOutcome::err("delegate_to_browser needs an 'instruction' describing the web task.");
    }
    // A task the browser handed us must not bounce back to the browser -
    // that's how two cooperating agents end up in an endless loop.
    if source.starts_with("browser:") {
        return ToolOutcome::err("This task came from the browser agent - do the desktop part and return the result instead of delegating back to the browser.");
    }
    let background = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut options = json!({});
    if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
        if !url.trim().is_empty() {
            options["url"] = json!(url.trim());
        }
    }
    // Shared data for the browser side (file contents, values) - it can't read the PC.
    if let Some(ctx) = args.get("context").and_then(|v| v.as_str()) {
        let ctx: String = ctx.trim().chars().take(20000).collect();
        if !ctx.is_empty() {
            options["context"] = json!(ctx);
        }
    }

    let b = crate::bridge::bridge();
    if background {
        match b.start_task(source, instruction, options) {
            Ok(id) => ToolOutcome::text(format!(
                "Started the browser task in the background (task_id: {}). Continue with other \
local work, then call check_browser_task with this id to get the result.",
                id
            )),
            Err(e) => ToolOutcome::err(e),
        }
    } else {
        match b.run_task(source, instruction, options).await {
            Ok(result) => ToolOutcome::text(format!("Browser task result:\n{}", result)),
            Err(e) => ToolOutcome::err(e),
        }
    }
}

/// Collect the result of a background browser task started earlier.
pub(crate) async fn check_browser_task_tool(args: &Value) -> ToolOutcome {
    let id = args.get("task_id").and_then(|v| v.as_str()).unwrap_or("").trim();
    if id.is_empty() {
        return ToolOutcome::err("check_browser_task needs the 'task_id' from delegate_to_browser.");
    }
    match crate::bridge::bridge().await_task(id, 600).await {
        Ok(result) => ToolOutcome::text(format!("Browser task result:\n{}", result)),
        Err(e) => ToolOutcome::err(e),
    }
}

pub(crate) async fn download_file_tool(args: &Value) -> ToolOutcome {
    const MAX_BYTES: usize = 300 * 1024 * 1024;
    let url = args.get("url").and_then(|p| p.as_str()).unwrap_or("").trim();
    let raw = args.get("path").and_then(|p| p.as_str()).unwrap_or("").trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) || raw.is_empty() {
        return ToolOutcome::err("download_file needs an http(s) 'url' and a destination 'path'");
    }
    let path = resolve_agent_path(raw);
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(300)).build() {
        Ok(c) => c,
        Err(e) => return ToolOutcome::err(e.to_string()),
    };
    let resp = match client.get(url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => return ToolOutcome::err(format!("download failed: HTTP {}", r.status())),
        Err(e) => return ToolOutcome::err(format!("download failed: {}", e)),
    };
    if resp.content_length().map(|n| n as usize > MAX_BYTES).unwrap_or(false) {
        return ToolOutcome::err("file is larger than 300 MB - use run_command (e.g. curl) if you really need it");
    }
    let bytes = match resp.bytes().await {
        Ok(b) if b.len() <= MAX_BYTES => b,
        Ok(_) => return ToolOutcome::err("file is larger than 300 MB"),
        Err(e) => return ToolOutcome::err(format!("download failed: {}", e)),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, &bytes) {
        Ok(()) => ToolOutcome::text(format!("Saved {} ({} KB).", path.display(), bytes.len() / 1024)),
        Err(e) => ToolOutcome::err(format!("could not write {}: {}", path.display(), e)),
    }
}
