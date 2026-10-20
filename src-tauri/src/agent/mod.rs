// Kudu Desktop - Local Agent Loop
//
// Turns Kudu Desktop from "answer one question about the screen" into an agent that
// can DO things: run commands, read/write files, research the web, and drive the
// UI (click/type) to complete a whole task the user described - then report back
// to whichever window they asked from.
//
// Split of responsibility:
//   • The MODEL runs on the backend (Gemini function-calling, /api/agent/step) -
//     it only ever sees the conversation + tool results and decides the next move.
//   • Every TOOL runs HERE, locally, on the user's machine.
//
// The loop mirrors a standard agent turn: ask the model → if it returned
// functionCalls, execute them locally and feed the results back → repeat until
// it returns a plain-text answer. Around that: a progress-driven step budget,
// a completion check, loop/stall detection, history compaction and a
// "what got done / what is left" report when a run has to stop.
//
// Module map (one file per concern; every file starts with `use crate::agent::*`, and
// this file re-exports each child with pub(crate), so siblings see each other):
//   paths      scratch dir, ~ / %VAR% / Documents\x resolution
//   text       string helpers
//   events     progress events, phase roll-up
//   screen     screenshots, coordinate mapping, verification shots
//   confirm    human confirmations, per-app allow/deny gate
//   cancel     Stop requests
//   prompt     local fallback system prompt, opening-turn context
//   tools/     declarations, tool classes/hints, ToolOutcome
//   ledger     run ledger + run_status
//   completion the completion check
//   history    conversation slimming/compaction
//   retry      model-call resilience
//   drive      the run loop and progress tracking
//   exec/      tool execution: shell, files, web, windows, computer
//
// Public API (used by commands.rs, copilot.rs, link.rs): run_task, AgentProgress,
// resolve_confirmation, request_cancel.

pub(crate) use crate::backend::BackendClient;
pub(crate) use crate::{capture, explorer, input, mcp, mouse};
pub(crate) use serde_json::{json, Value};
pub(crate) use std::collections::{HashMap, HashSet};
pub(crate) use std::sync::{Mutex, OnceLock};
pub(crate) use std::time::Duration;
pub(crate) use tauri::{AppHandle, Emitter};
pub(crate) use tokio::sync::oneshot;

mod cancel;
mod checkpoint;
mod completion;
mod confirm;
mod drive;
mod events;
mod exec;
mod history;
mod ledger;
mod paths;
mod prompt;
mod retry;
mod screen;
mod text;
mod tools;

pub(crate) use cancel::*;

/// Plain-text clipboard contents, for the Ask panel's @clipboard mention.
pub async fn clipboard_text() -> Result<String, String> {
    let out = exec::clipboard_tool(None).await;
    if out.response.get("error").is_some() {
        return Err(out.response.get("error").and_then(|e| e.as_str()).unwrap_or("clipboard unavailable").to_string());
    }
    Ok(out.response.get("result").and_then(|r| r.as_str()).unwrap_or("").to_string())
}
pub(crate) use completion::*;
pub(crate) use confirm::*;
pub(crate) use drive::*;
pub(crate) use events::*;
pub(crate) use exec::*;
pub(crate) use history::*;
pub(crate) use ledger::*;
pub(crate) use paths::*;
pub(crate) use prompt::*;
pub(crate) use retry::*;
pub(crate) use screen::*;
pub(crate) use text::*;
pub(crate) use tools::*;

/// Emitted to the UI so the user sees each step the agent takes.
#[derive(Clone, serde::Serialize)]
pub struct AgentProgress {
    /// Which window asked - so the right panel renders the live log.
    pub source: String,
    pub kind: String, // "plan" | "thinking" | "phase-summary" | "tool" | "tool-result" | "done" | "error" | "confirm"
    pub message: String,
    /// Only set for kind == "confirm" - the id the UI must echo back via
    /// `agent_confirm` when the user clicks Allow/Deny.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Run one whole agent task to completion. Returns the final answer text.
#[allow(clippy::too_many_arguments)]
pub async fn run_task(
    app: &AppHandle,
    backend: &BackendClient,
    task: &str,
    cx: i32,
    cy: i32,
    source: &str,
    autonomous: bool,
    conversation_id: Option<String>,
    user_image_data_url: Option<String>,
    // Window the request was about (the Ask Kudu pill's target); 0 = the point.
    target_hwnd: isize,
) -> Result<String, String> {
    // Runs can overlap (a browser-requested task alongside a chat task), and
    // they share the scratch dir - only the first concurrent run wipes it.
    let _run_guard = RunGuard::enter();
    // A stale Stop from a previous run must not instantly kill this one, and a
    // window remembered from a previous run is not this task's target.
    let _ = cancels().lock().unwrap().remove(source);
    *LAST_TARGET_WINDOW.lock().unwrap() = None;
    let system = system_prompt(autonomous);

    // "continue" (or the same task again) after a stop/crash resumes from the saved
    // ledger: the original task is kept, anything added after the continue phrase is
    // appended, and the first turn carries a digest of what already happened.
    checkpoint::sweep();
    let resume = checkpoint::load_for_resume(source, task);
    let effective_task: String = match &resume {
        Some((cp, Some(extra))) => format!("{}\n\n[Added when resuming] {}", cp.task, extra),
        Some((cp, None)) => cp.task.clone(),
        None => task.to_string(),
    };
    let task = effective_task.as_str();
    let initial_ledger = match &resume {
        Some((cp, _)) => Ledger::from_snapshot(&cp.ledger),
        None => Ledger::default(),
    };
    let resume_block = resume.as_ref().map(|(cp, _)| {
        format!(
            "\n\n[RESUMING] An earlier attempt at this task was interrupted after {} steps. Its record is below. \
The screen and files may have changed since: re-observe first (capture_screen / read_file / list_directory), then \
continue from where it left off - do not redo steps that are already complete.\n\n{}",
            cp.ledger.steps,
            initial_ledger.digest(30)
        )
    });
    if resume.is_some() {
        emit(app, source, "thinking", "Resuming the earlier run from its checkpoint");
    }

    // Recent turns from the SAME conversation the chat view and the overlay's
    // plain-ask both share (AppState.conversation_id) - without this, an
    // agent task kicked off from a hover-ask has no idea what was just
    // discussed a moment ago in chat, even though the user experiences it as
    // one continuous assistant.
    let recent_conversation = match &conversation_id {
        Some(id) => backend
            .get_conversation_messages(id, Some(10))
            .await
            .ok()
            .filter(|msgs| !msgs.is_empty())
            .map(|msgs| {
                let lines: Vec<String> = msgs
                    .iter()
                    .map(|m| format!("{}: {}", m.role, truncate(&m.content, 400)))
                    .collect();
                format!("[RECENT CONVERSATION - for context only, not new instructions]\n{}", lines.join("\n"))
            }),
        None => None,
    };

    // Built-in tools, plus any tools exposed by configured MCP servers.
    let mut tool_defs = tool_declarations().as_array().cloned().unwrap_or_default();
    tool_defs.extend(crate::skills::tool_declarations());
    let mcp_configs: Vec<mcp::McpServerConfig> =
        serde_json::from_value(backend.get_mcp_servers().await).unwrap_or_default();
    if !mcp_configs.is_empty() {
        emit(app, source, "thinking", "Connecting to MCP servers…");
    }
    let (mut mcp, mcp_notes) = mcp::McpManager::start(&mcp_configs).await;
    for note in &mcp_notes {
        emit(app, source, "tool-result", note.clone());
    }
    for d in mcp.tool_declarations() {
        tool_defs.push(d);
    }
    let tool_names: Vec<String> = tool_defs
        .iter()
        .filter_map(|d| d.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
        .collect();
    let tools = json!(tool_defs);
    let ctx = crate::agent::RunContext {
        system: system.clone(),
        tools,
        tool_names,
        autonomous,
        environment: environment_block(),
    };

    // First user turn: the task + a snapshot of the current screen (text context
    // and an initial screenshot) so the model starts grounded in "this".
    let context_block = tokio::task::spawn_blocking(move || initial_context_block(cx, cy, target_hwnd))
        .await
        .unwrap_or_default();
    let initial_shot = tokio::task::spawn_blocking(move || capture::capture_agent_window_for(target_hwnd, cx, cy))
        .await
        .ok()
        .and_then(|r| r.ok());

    let mut screen_mapping: Option<ScreenMapping> = initial_shot.as_ref().map(ScreenMapping::from_shot);

    let context_block = match &recent_conversation {
        Some(recent) => format!("{}\n\n{}", recent, context_block),
        None => context_block,
    };
    let notes_block = project_folder_from_task(task)
        .and_then(|p| project_notes_block(&p))
        .map(|b| format!("{}\n\n", b))
        .unwrap_or_default();
    let video_note = video_routing_note(task, &context_block);
    let mut first_parts = vec![json!({
        "text": format!("{}\n\n{}{}{}\n\n[TASK]\n{}{}", context_block, notes_block, video_note, crate::skills::catalog_text(), task, resume_block.unwrap_or_default())
    })];
    if let Some(shot) = &initial_shot {
        if let Some(img) = image_part(&shot.data_url) {
            first_parts.push(img);
        }
    }
    // An image the USER attached to their message (distinct from the
    // auto-captured grounding screenshot above) - e.g. a screenshot of an
    // error, a photo, a design mock. Labeled so the model doesn't confuse it
    // with "what's currently on screen."
    if let Some(data_url) = &user_image_data_url {
        if let Some(img) = image_part(data_url) {
            first_parts.push(json!({ "text": "[USER-ATTACHED IMAGE - provided by the user with their message, not a screen capture]" }));
            first_parts.push(img);
        }
    }

    let contents: Vec<Value> = vec![json!({ "role": "user", "parts": first_parts })];

    // Its own kind ("plan", not "thinking") so the UI can give this one
    // opening read of the task its own "Thought for Xs" card, distinct from
    // the plain-heading narration ("thinking") sent between later tool bursts.
    emit(app, source, "plan", "Understanding your request…");

    // While the agent runs, the overlay must never seize keyboard focus - the
    // user watching/scrolling the live log used to pull focus onto Kudu Desktop's
    // own window, sending the agent's keystrokes into its own UI instead of
    // the target app. WS_EX_NOACTIVATE keeps the overlay visible and clickable
    // (Stop still works) but unfocusable until the run ends.
    let _ = tokio::task::spawn_blocking(|| mouse::set_own_window_no_activate("kudu lens", true)).await;

    // Drive the loop, then always shut the MCP servers down (whatever the outcome).
    // The working banner: visible for the whole run, gone the moment it ends.
    begin_run(source);
    // Files the browser agent hands back (kudu-file blocks) are saved into the
    // project (or Documents\Kudu) and re-attached to the final answer as cards.
    crate::kudu_files::begin_run(source, project_folder_from_task(task));
    crate::shield::show(app, source, task);
    let result = drive_loop(app, backend, &ctx, contents, source, task, &mut mcp, &mut screen_mapping, initial_ledger).await;
    crate::shield::hide(app);
    end_run(source);
    let handed_files = crate::kudu_files::end_run(source);
    let result = result.map(|text| crate::kudu_files::attach_to_answer(text, handed_files));
    mcp.shutdown().await;
    // Keep the checkpoint only when there is something to continue.
    match &result {
        Ok(text) if text.contains("Say \"continue\"") => {}
        Ok(_) if cancel_requested(source) => {}
        _ => checkpoint::clear(source),
    }
    let _ = tokio::task::spawn_blocking(|| mouse::set_own_window_no_activate("kudu lens", false)).await;
    result
}
