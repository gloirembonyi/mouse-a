// Mouse AI desktop agent - the run loop (observe -> decide -> act -> verify) and progress tracking.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// ── Step budget ──────────────────────────────────────────────────
// A fixed 40-step cap ended long jobs that WERE working mid-way, while a run
// that was stuck burned all 40 steps before stopping. Progress is the right
// signal, not step count: the budget starts at BASE_BUDGET and grows by
// BUDGET_EXTENSION whenever it runs out while recent steps still made progress,
// up to HARD_MAX_STEPS / MAX_RUN_TIME. Stalls and loops stop the run early.
pub(crate) const BASE_BUDGET: usize = 40;

pub(crate) const BUDGET_EXTENSION: usize = 15;

pub(crate) const HARD_MAX_STEPS: usize = 120;

pub(crate) const MAX_RUN_TIME: Duration = Duration::from_secs(30 * 60);

/// Model steps with no progress signal before a "re-think" warning…
pub(crate) const STALL_WARN_STEPS: usize = 6;

/// …and before the run is stopped as stuck.
pub(crate) const STALL_STOP_STEPS: usize = 12;

/// The same call on the same state this many times = a loop (warned)…
pub(crate) const LOOP_REPEAT_WARN: usize = 3;

/// …and this many times with no progress = stopped.
pub(crate) const LOOP_REPEAT_STOP: usize = 6;

/// How many times a "done" answer can be sent back for another look.
pub(crate) const MAX_COMPLETION_REVIEWS: usize = 2;

// A clean HTTP 200 with a genuinely empty candidate (no text, no functionCall) is rare
// but does happen under load - retrying with a short nudge recovers most of these
// instead of ending the whole task on one blank turn (see drive_loop).
pub(crate) const MAX_EMPTY_RESPONSE_RETRIES: usize = 3;

/// Thinking tokens for planning / recovery / verification steps (0 elsewhere).
pub(crate) const PLANNING_THINKING_BUDGET: u32 = 1024;

#[derive(Default)]
pub(crate) struct ProgressTracker {
    /// Bumped whenever the world visibly changes; part of the loop key, so the
    /// same call after a real change isn't counted as a repeat.
    pub(crate) state_version: u64,
    pub(crate) call_counts: HashMap<String, usize>,
    pub(crate) seen_results: HashSet<u64>,
    pub(crate) last_screen: Option<Vec<u8>>,
    pub(crate) stall_steps: usize,
    pub(crate) last_progress_step: usize,
}

impl ProgressTracker {
    /// Counts this call against the current state; returns how many times the
    /// identical call has now been made on this state (0 for exempt tools).
    pub(crate) fn note_call(&mut self, name: &str, args: &Value) -> usize {
        if is_loop_exempt(name) {
            return 0;
        }
        let key = format!("{}:{}@{}", name, args, self.state_version);
        let n = self.call_counts.entry(key).or_insert(0);
        *n += 1;
        *n
    }

    /// Progress from a successful result: new data, or a state change that
    /// hasn't happened before.
    pub(crate) fn observe_result(&mut self, name: &str, args: &Value, response: &Value, is_mcp: bool) -> bool {
        let mut clean = response.clone();
        if let Some(obj) = clean.as_object_mut() {
            obj.remove("run_status");
            obj.remove("verification");
        }
        let progressed = if is_data_tool(name) || is_mcp {
            self.seen_results.insert(hash_str(&format!("{}:{}", name, clean)))
        } else if matches!(name, "write_file" | "edit_file" | "delete_file" | "download_file" | "set_clipboard") {
            self.seen_results.insert(hash_str(&format!("{}:{}", name, args)))
        } else {
            false
        };
        if progressed && (is_effect_tool(name) || is_mcp) {
            self.state_version += 1;
        }
        progressed
    }

    /// Returns true if this screenshot differs from the previous one.
    pub(crate) fn observe_screen(&mut self, thumb: Vec<u8>) -> bool {
        let changed = match &self.last_screen {
            Some(prev) => screens_differ(prev, &thumb),
            None => true,
        };
        if changed {
            self.state_version += 1;
        }
        self.last_screen = Some(thumb);
        changed
    }

    pub(crate) fn end_round(&mut self, progressed: bool, step: usize) {
        if progressed {
            self.stall_steps = 0;
            self.last_progress_step = step;
        } else {
            self.stall_steps += 1;
        }
    }

    pub(crate) fn worst_repeat(&self) -> usize {
        self.call_counts.values().copied().max().unwrap_or(0)
    }
}

// ── History management ───────────────────────────────────────────
// Gemini function calling needs each user turn of functionResponses to follow
// the model turn that issued those functionCalls. Everything here removes or
// trims whole rounds so that pairing stays valid.

/// Stop the run with a clear account of what happened: the model gets one
/// no-tools turn to write it (it knows the nuance); if that fails, the ledger
/// writes it.
#[allow(clippy::too_many_arguments)]
/// Everything one run sends with every step: the local fallback prompt, the tool
/// catalog, and the facts the backend's "desktop" prompt profile renders.
#[derive(Clone)]
pub(crate) struct RunContext {
    pub(crate) system: String,
    pub(crate) tools: Value,
    pub(crate) tool_names: Vec<String>,
    pub(crate) autonomous: bool,
    pub(crate) environment: String,
}

impl RunContext {
    pub(crate) fn prompt_context(&self) -> Value {
        json!({
            "autonomous": self.autonomous,
            "toolNames": self.tool_names,
            // Behaviours this build implements, so the server prompt never describes a
            // result shape the client does not produce.
            "capabilities": ["structured_run_command", "type_text_modes", "checkpoint_resume", "element_tools"],
            "environment": self.environment,
            "clientVersion": env!("CARGO_PKG_VERSION"),
        })
    }

    /// The same run with no tools offered - for the wrap-up turn, where a stray
    /// function call would be a bug rather than an action.
    pub(crate) fn without_tools(&self) -> RunContext {
        RunContext { tools: json!([]), tool_names: Vec::new(), ..self.clone() }
    }
}

pub(crate) async fn stop_with_report(
    app: &AppHandle,
    backend: &BackendClient,
    ctx: &RunContext,
    contents: &mut Vec<Value>,
    source: &str,
    task: &str,
    ledger: &Ledger,
    phase: &mut PhaseCounts,
    reason: &str,
) -> String {
    flush_phase(app, source, phase);
    checkpoint::save(source, task, "incomplete", ledger);
    emit(app, source, "thinking", format!("Wrapping up: {}", reason));
    push_user_text(
        contents,
        &format!(
            "[SYSTEM] The run is stopping now: {}. Do NOT call any tools. Reply in plain text (Markdown) \
with: (1) what you completed, with the evidence you saw; (2) what is still left to do; (3) any data or \
answers the user asked for. Be specific and brief.",
            reason
        ),
    );
    prune_old_images(contents, 1);
    let quiet = ctx.without_tools();
    let text = match agent_step_with_retry(app, backend, &quiet, contents, source, ledger, None).await {
        Ok(step) => step
            .get("parts")
            .and_then(|p| p.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .map(|t| t.trim())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    emit(app, source, "done", "Stopped before finishing");
    if text.is_empty() {
        ledger_report(ledger, reason)
    } else {
        format!("{}{}", text, CONTINUE_HINT)
    }
}

pub(crate) fn stopped_message(mid_burst: bool) -> String {
    if mid_burst {
        "⏹ Stopped at your request. I've halted the task - later steps were not run.".to_string()
    } else {
        "⏹ Stopped at your request. I've halted the task - nothing further was done.".to_string()
    }
}

/// The core agent turn loop: ask the model, execute any tool calls (built-in or
/// MCP), feed results back, repeat until it answers - with a progress-driven
/// step budget, a completion check, loop/stall detection and history
/// compaction (see the sections above).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_loop(
    app: &AppHandle,
    backend: &BackendClient,
    ctx: &RunContext,
    mut contents: Vec<Value>,
    source: &str,
    task: &str,
    mcp: &mut mcp::McpManager,
    screen: &mut Option<ScreenMapping>,
    initial_ledger: Ledger,
) -> Result<String, String> {
    let started = std::time::Instant::now();
    let project = project_folder_from_task(task);
    let mut ledger = initial_ledger;
    // Per-task tool selection: the core set plus what the task's words call for. The
    // full catalog stays available in `ctx`; `active` is what the model is offered, and
    // it widens to everything once if the run stalls, errors repeatedly, the model asks
    // for an un-offered tool, or a completion review is needed.
    let full = ctx;
    let browser_connected = crate::bridge::bridge().is_connected() && crate::bridge::bridge().is_paired();
    let mut active = full.clone();
    {
        let all = full.tools.as_array().cloned().unwrap_or_default();
        let picked = select_tools(strip_project_prefix(task), &all, browser_connected);
        active.tool_names = picked.iter().filter_map(|d| d.get("name").and_then(|n| n.as_str()).map(String::from)).collect();
        active.tools = json!(picked);
    }
    let mut widened = active.tool_names.len() >= full.tool_names.len();
    let widen = |active: &mut RunContext, widened: &mut bool, _why: &str| {
        if *widened {
            return;
        }
        *widened = true;
        active.tools = full.tools.clone();
        active.tool_names = full.tool_names.clone();
        #[cfg(debug_assertions)]
        eprintln!("[agent] tool filter lifted: {}", _why);
    };

    let mut progress = ProgressTracker::default();
    let mut budget = BASE_BUDGET;
    let mut empty_response_count: usize = 0;
    let mut completion_reviews: usize = 0;
    let mut think_next = false;
    let mut last_round_failed = false;
    let mut consecutive_errors: usize = 0;
    let mut last_failed_call: Option<String> = None;
    let mut repeated_failures: usize = 0;
    // Tallies tool-call activity between narration messages - flushed as a
    // "phase-summary" event right before the next "thinking" line (or at the
    // end of the run), so the UI can show "Explored 12 files, 3 searches"
    // instead of 12 separate raw tool-call rows.
    let mut phase = PhaseCounts::default();
    // Processes the user has already approved Mouse AI controlling THIS run -
    // see `guard_target_app`. Empty at the start of every task, never persisted.
    let mut trusted_apps: HashSet<String> = HashSet::new();
    let mut iteration: usize = 0;

    loop {
        if cancel_requested(source) {
            flush_phase(app, source, &mut phase);
            emit(app, source, "done", "Stopped");
            checkpoint::save(source, task, "cancelled", &ledger);
            return Ok(stopped_message(false));
        }

        // ── Budget: extend while progressing, stop when stuck or out of time.
        let time_left = started.elapsed() < MAX_RUN_TIME;
        if iteration >= budget || !time_left {
            let recent_progress = ledger.steps.saturating_sub(progress.last_progress_step) < BUDGET_EXTENSION;
            if time_left && recent_progress && budget < HARD_MAX_STEPS {
                budget = (budget + BUDGET_EXTENSION).min(HARD_MAX_STEPS);
                emit(app, source, "thinking", format!("Still making progress - continuing (step budget now {})", budget));
            } else {
                let why = if !time_left {
                    format!("it has been running for {} minutes", MAX_RUN_TIME.as_secs() / 60)
                } else if budget >= HARD_MAX_STEPS {
                    format!("it reached the maximum of {} steps", HARD_MAX_STEPS)
                } else {
                    format!(
                        "the last {} steps made no visible progress",
                        ledger.steps.saturating_sub(progress.last_progress_step)
                    )
                };
                let report =
                    stop_with_report(app, backend, &active, &mut contents, source, task, &ledger, &mut phase, &why).await;
                return Ok(report);
            }
        }
        iteration += 1;
        ledger.steps = iteration;

        prune_old_images(&mut contents, KEEP_IMAGES);
        // Think on the steps that decide the run: the first plan, recovering from
        // repeated failures or a stall, and the check before finishing.
        let think = iteration == 1
            || think_next
            || consecutive_errors >= 2
            || progress.stall_steps >= STALL_WARN_STEPS;
        think_next = false;
        let step = match agent_step_with_retry(app, backend, &active, &mut contents, source, &ledger, think.then_some(PLANNING_THINKING_BUDGET)).await {
            Ok(step) => {
                // Which prompt/model actually ran - the first thing to check when a run
                // "got dumb mid-task". Debug builds only; the backend logs it per step.
                #[cfg(debug_assertions)]
                if iteration == 1 {
                    eprintln!(
                        "[agent] prompt={} model={}",
                        step.get("promptVersion").and_then(|v| v.as_str()).unwrap_or("local"),
                        step.get("modelUsed").and_then(|v| v.as_str()).unwrap_or("?")
                    );
                }
                step
            }
            Err(e) => {
                if cancel_requested(source) {
                    flush_phase(app, source, &mut phase);
                    emit(app, source, "done", "Stopped");
                    return Ok(stopped_message(false));
                }
                // Nothing done yet: surface the error as before. Otherwise keep
                // the work - report it so "continue" can pick it up.
                if ledger.actions == 0 {
                    return Err(e);
                }
                flush_phase(app, source, &mut phase);
                emit(app, source, "error", truncate(&e, 200));
                emit(app, source, "done", "Stopped before finishing");
                return Ok(ledger_report(&ledger, &format!("the AI service kept failing ({})", clip(&e, 160))));
            }
        };
        let parts = step.get("parts").and_then(|p| p.as_array()).cloned().unwrap_or_default();

        // Split the model's reply into free text and tool calls, and echo the
        // model's OWN parts back verbatim into history (not a rebuilt {name,args}
        // object) - thinking-enabled Gemini models attach a sibling
        // `thoughtSignature` field to functionCall parts that MUST be replayed
        // back unchanged on the next turn, or the API 400s with "Function call is
        // missing a thought_signature". Reconstructing a stripped-down part
        // silently drops that field.
        let mut assistant_text = String::new();
        let mut calls: Vec<(String, Value)> = Vec::new();
        let mut model_parts: Vec<Value> = Vec::new();

        for part in &parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                if !t.trim().is_empty() {
                    assistant_text.push_str(t);
                    model_parts.push(part.clone());
                }
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                model_parts.push(part.clone());
                calls.push((name, args));
            }
        }

        if model_parts.is_empty() {
            // A clean response with no text and no functionCall - Gemini occasionally
            // returns this under load even after the backend's own retry/key-rotation.
            // Nudge it to continue instead of ending the whole task on one blank turn.
            empty_response_count += 1;
            if empty_response_count > MAX_EMPTY_RESPONSE_RETRIES {
                if ledger.actions == 0 {
                    return Err(
                        "The model kept returning empty responses. This can happen under heavy load - \
wait a moment and try again, or try a smaller/more specific task."
                            .to_string(),
                    );
                }
                flush_phase(app, source, &mut phase);
                emit(app, source, "done", "Stopped before finishing");
                return Ok(ledger_report(&ledger, "the AI kept returning empty responses"));
            }
            emit(
                app,
                source,
                "thinking",
                format!(
                    "The model returned an empty response - retrying ({}/{})…",
                    empty_response_count, MAX_EMPTY_RESPONSE_RETRIES
                ),
            );
            push_user_text(
                &mut contents,
                "[SYSTEM NOTE] Your last turn had no content (no text, no tool call). Please continue: call \
a tool if you need more information, or give a plain-text final answer if the task is already done.",
            );
            continue;
        }
        empty_response_count = 0;
        contents.push(json!({ "role": "model", "parts": model_parts }));

        // ── No tool calls → a final answer, held to evidence first.
        if calls.is_empty() {
            let concerns = completion_concerns(&assistant_text, &ledger, task, last_round_failed);
            if !concerns.is_empty() && completion_reviews < MAX_COMPLETION_REVIEWS && iteration + 1 < budget {
                completion_reviews += 1;
                think_next = true;
                widen(&mut active, &mut widened, "completion review");
                flush_phase(app, source, &mut phase);
                emit(app, source, "thinking", "Checking the result before finishing…");
                let mut note = String::from("[SYSTEM] Completion check before you finish.\n");
                if !ledger.done_when.is_empty() {
                    note.push_str(&format!("Definition of done: {}\n", ledger.done_when));
                }
                note.push_str(&format!("Concerns:\n- {}\n\n", concerns.join("\n- ")));
                note.push_str(
                    "If the goal really is met, confirm it with a fresh observation (re-run the check, \
read_file the result, capture_screen) and then give your final answer. If it isn't met, keep working now - \
you still have steps left. If it truly can't be done, say exactly what blocks it.",
                );
                push_user_text(&mut contents, &note);
                continue;
            }
            flush_phase(app, source, &mut phase);
            emit(app, source, "done", "Done");
            checkpoint::clear(source);
            return Ok(assistant_text);
        }

        if !assistant_text.trim().is_empty() {
            // Flush whatever the PREVIOUS burst of tool calls did before
            // showing the model's new narration - so the rollup line reads
            // as "here's what that last phase did" ahead of "here's the next
            // phase," matching the order the work actually happened in.
            flush_phase(app, source, &mut phase);
            emit(app, source, "thinking", truncate(&assistant_text, 400));
        }

        // ── Execute every requested tool (built-in, MCP, or loop-internal) and
        // collect the responses (+ any images) into one user turn.
        strip_run_status(&mut contents);
        let mut response_parts: Vec<Value> = Vec::new();
        let mut progressed = false;
        let mut round_failed = false;
        let mut loop_warning: Option<String> = None;
        for (name, args) in calls {
            // Stop promptly mid-burst too - don't run the remaining tool calls
            // of this turn after the user has already clicked Stop.
            if cancel_requested(source) {
                flush_phase(app, source, &mut phase);
                emit(app, source, "done", "Stopped");
                return Ok(stopped_message(true));
            }

            if name == "update_plan" {
                if ledger.apply_plan_update(&args) {
                    progressed = true;
                }
                if !ledger.plan.is_empty() {
                    emit(app, source, "plan", ledger.plan_summary_line());
                }
                ledger.record("updated plan", true, None);
                let mut resp = json!({ "ok": true, "plan": ledger.plan_lines() });
                if !ledger.done_when.is_empty() {
                    resp["done_when"] = json!(ledger.done_when);
                }
                response_parts.push(json!({ "functionResponse": { "name": name, "response": resp } }));
                continue;
            }

            ledger.actions += 1;
            // The model named a tool it was not offered: the relevance filter guessed
            // wrong. Execute it anyway and lift the filter for the rest of the run.
            if !active.tool_names.iter().any(|n| n == &name) && full.tool_names.iter().any(|n| n == &name) {
                widen(&mut active, &mut widened, "model asked for an un-offered tool");
            }
            let label = tool_label(&name, &args);
            emit(app, source, "tool", label.clone());
            let repeats = progress.note_call(&name, &args);
            let is_mcp = mcp.owns(&name);
            let mut outcome = if is_mcp {
                match mcp.call(&name, args.clone()).await {
                    Ok(text) => ToolOutcome {
                        response: json!({ "result": truncate(&text, MAX_TOOL_OUTPUT) }),
                        image_data_url: None,
                        log: truncate(&text, 300),
                    },
                    Err(e) => ToolOutcome::err(e),
                }
            } else {
                let mut outcome =
                    execute_tool(app, backend, source, &name, &args, screen, &mut trusted_apps).await;
                // Observe-act-verify: UI actions get a fresh screenshot attached
                // automatically so the model SEES the result instead of assuming it.
                attach_verification_shot(&name, &mut outcome, screen).await;
                outcome
            };
            if !outcome.response.is_object() {
                outcome.response = json!({ "result": outcome.response });
            }

            // Did the screen visibly change? (Progress for UI work.)
            if let Some(url) = outcome.image_data_url.clone() {
                if let Ok(Some(thumb)) = tokio::task::spawn_blocking(move || screen_thumbnail(&url)).await {
                    if progress.observe_screen(thumb) {
                        progressed = true;
                    }
                }
            }

            if tool_failed(&name, &outcome.response) {
                round_failed = true;
                ledger.errors += 1;
                consecutive_errors += 1;
                let failure = response_text(&outcome.response);
                // Identical call, identical failure, again: the model isn't
                // adapting, it's retrying. Say so where it can't be missed.
                let fingerprint = format!("{}:{}", name, args);
                if last_failed_call.as_deref() == Some(fingerprint.as_str()) {
                    repeated_failures += 1;
                    outcome.response["repeat_warning"] = json!(format!(
                        "You've now called {} with exactly these arguments {} times and it failed the same way. \
Repeating it won't work - change something real or take a different route.",
                        name,
                        repeated_failures + 1
                    ));
                } else {
                    repeated_failures = 0;
                }
                last_failed_call = Some(fingerprint);
                if let Some(hint) = recovery_hint(&name, &failure) {
                    outcome.response["hint"] = json!(hint);
                }
                let first_line = failure.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                ledger.record(&label, false, Some(clip(&first_line, 120)));
            } else {
                consecutive_errors = 0;
                repeated_failures = 0;
                last_failed_call = None;
                // A read-only command (Get-ChildItem, git status, dir…) is an
                // observation, not evidence that anything was done.
                if (is_effect_tool(&name) && !is_read_only_command(&name, &args)) || is_mcp {
                    ledger.effects += 1;
                }
                if progress.observe_result(&name, &args, &outcome.response, is_mcp) {
                    progressed = true;
                }
                // Data worth keeping past compaction.
                let keep_fact = is_mcp
                    || matches!(
                        name.as_str(),
                        "web_search" | "web_fetch" | "delegate_to_browser" | "check_browser_task" | "read_app_content"
                    );
                if keep_fact {
                    ledger.record_fact(format!("[step {}] {}: {}", ledger.steps, clip(&label, 60), response_text(&outcome.response)));
                }
                ledger.record(&label, true, None);
            }

            if repeats >= LOOP_REPEAT_WARN {
                let warning = format!(
                    "You've done {} with the same arguments {} times without anything changing in between. \
It isn't working - stop repeating it and take a different route.",
                    name, repeats
                );
                outcome.response["loop_warning"] = json!(warning.clone());
                loop_warning = Some(warning);
            }

            let (clean_log, diff) = extract_diff_marker(&outcome.log);
            phase.record(&name);
            if let Some((added, removed)) = diff {
                phase.record_diff(added, removed);
            }
            emit(app, source, "tool-result", truncate(&clean_log, 300));
            response_parts.push(json!({
                "functionResponse": { "name": name, "response": outcome.response }
            }));
            if let Some(img) = outcome.image_data_url.as_deref().and_then(image_part) {
                response_parts.push(json!({ "text": "Screenshot after the action:" }));
                response_parts.push(img);
            }
        }
        last_round_failed = round_failed;
        progress.end_round(progressed, ledger.steps);

        // run_status rides on the last result of the round.
        let can_extend = budget < HARD_MAX_STEPS && started.elapsed() < MAX_RUN_TIME;
        let warning = if progress.stall_steps >= STALL_WARN_STEPS {
            Some(format!(
                "No progress in the last {} steps (no new data, no visible change). Re-think: re-check the \
state, re-plan, and try a genuinely different route.",
                progress.stall_steps
            ))
        } else {
            loop_warning
        };
        let status = run_status(&ledger, budget, can_extend, project.as_deref(), warning);
        if let Some(last) = response_parts.iter_mut().rev().find(|p| p.get("functionResponse").is_some()) {
            if let Some(obj) = last.pointer_mut("/functionResponse/response").and_then(|r| r.as_object_mut()) {
                obj.insert("run_status".into(), status);
            }
        }
        contents.push(json!({ "role": "user", "parts": response_parts }));
        checkpoint::save(source, task, "running", &ledger);
        if consecutive_errors >= 2 || progress.stall_steps >= STALL_WARN_STEPS {
            widen(&mut active, &mut widened, "errors or stall");
        }

        // After several failures in a row, force a step back.
        if consecutive_errors >= 4 {
            consecutive_errors = 0;
            emit(app, source, "thinking", "Several steps failed in a row - rethinking the approach…");
            push_user_text(
                &mut contents,
                "[SYSTEM NOTE] Several tool calls in a row failed. Stop retrying this approach: re-check the \
current state, reconsider the plan, and try a genuinely different route. If the task truly can't be done, \
finish now with a plain-text explanation of what's blocking it.",
            );
        }

        // A hard loop (the identical call on an unchanged state, warned about
        // and done again anyway) won't resolve itself; neither will a long stall.
        if !progressed && progress.worst_repeat() >= LOOP_REPEAT_STOP {
            let report = stop_with_report(
                app,
                backend,
                &active,
                &mut contents,
                source,
                task,
                &ledger,
                &mut phase,
                "it kept repeating the same action without anything changing",
            )
            .await;
            return Ok(report);
        }
        if progress.stall_steps >= STALL_STOP_STEPS {
            let why = format!("the last {} steps made no visible progress (it looks stuck)", progress.stall_steps);
            let report =
                stop_with_report(app, backend, &active, &mut contents, source, task, &ledger, &mut phase, &why).await;
            return Ok(report);
        }

        // Graceful finish: when the budget is nearly spent and won't be
        // extended, tell the model to verify and report instead of being cut off.
        let will_extend =
            ledger.steps.saturating_sub(progress.last_progress_step) < BUDGET_EXTENSION && can_extend;
        if budget.saturating_sub(ledger.steps) == 2 && !will_extend {
            push_user_text(
                &mut contents,
                "[SYSTEM NOTE] You're almost out of steps. Do at most one more essential action, then verify \
the outcome and give your final plain-text answer - state clearly what you completed and what is left.",
            );
        }

        slim_old_tool_results(&mut contents, KEEP_FULL_TOOL_ROUNDS);
        if compact_history(&mut contents, &ledger.digest(30), false) {
            emit(app, source, "thinking", "Compacting older steps to keep the context small…");
        }
    }
}

#[cfg(test)]
mod agent_loop_tests {
    use super::*;

    pub(crate) fn model_call(name: &str, n: usize) -> Value {
        json!({ "role": "model", "parts": [{ "functionCall": { "name": name, "args": { "i": n } }, "thoughtSignature": "sig" }] })
    }

    pub(crate) fn tool_response(name: &str, body: &str) -> Value {
        json!({ "role": "user", "parts": [{ "functionResponse": { "name": name, "response": { "result": body } } }] })
    }

    pub(crate) fn history(rounds: usize, body: &str) -> Vec<Value> {
        let mut contents = vec![json!({ "role": "user", "parts": [{ "text": "[TASK]\nfix the build" }] })];
        for i in 0..rounds {
            contents.push(model_call("read_file", i));
            contents.push(tool_response("read_file", body));
        }
        contents
    }

    /// Every functionResponse turn must directly follow the model turn that
    /// issued the same number of functionCalls, and roles must alternate.
    pub(crate) fn assert_valid_transcript(contents: &[Value]) {
        assert_eq!(contents[0]["role"], "user");
        for i in 1..contents.len() {
            assert_ne!(contents[i]["role"], contents[i - 1]["role"], "roles must alternate at {}", i);
            if is_tool_round(&contents[i]) {
                let prev = &contents[i - 1];
                assert_eq!(prev["role"], "model");
                let calls = prev["parts"].as_array().unwrap().iter().filter(|p| p.get("functionCall").is_some()).count();
                let resps =
                    contents[i]["parts"].as_array().unwrap().iter().filter(|p| p.get("functionResponse").is_some()).count();
                assert_eq!(calls, resps, "call/response count mismatch at {}", i);
            }
        }
    }

    #[test]
    pub(crate) fn compaction_keeps_pairs_valid_and_recent_rounds() {
        let mut contents = history(20, "ok");
        assert!(compact_history(&mut contents, "Plan:\n✓ 1. read", false));
        assert_valid_transcript(&contents);
        assert_eq!(tool_round_indices(&contents).len(), COMPACT_KEEP_ROUNDS);
        // The newest round survived verbatim, thought signature included.
        let last_model = &contents[contents.len() - 2];
        assert_eq!(last_model["parts"][0]["functionCall"]["args"]["i"], 19);
        assert_eq!(last_model["parts"][0]["thoughtSignature"], "sig");
        let summary = contents[0]["parts"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string();
        assert!(summary.starts_with(COMPACTION_MARKER) && summary.contains("✓ 1. read"));
        // The task itself is still the first part.
        assert_eq!(contents[0]["parts"][0]["text"], "[TASK]\nfix the build");

        // A second compaction replaces the summary instead of stacking another.
        for i in 20..32 {
            contents.push(model_call("read_file", i));
            contents.push(tool_response("read_file", "ok"));
        }
        assert!(compact_history(&mut contents, "second digest", false));
        assert_valid_transcript(&contents);
        let markers = contents[0]["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["text"].as_str().is_some_and(|t| t.starts_with(COMPACTION_MARKER)))
            .count();
        assert_eq!(markers, 1);
    }

    #[test]
    pub(crate) fn compaction_skips_small_histories_unless_forced() {
        let mut small = history(10, "ok");
        assert!(!compact_history(&mut small, "d", false));
        assert_eq!(small.len(), 21);
        assert!(compact_history(&mut small, "d", true));
        assert_valid_transcript(&small);
        assert_eq!(tool_round_indices(&small).len(), (COMPACT_KEEP_ROUNDS / 2).max(2));
    }

    #[test]
    pub(crate) fn compaction_triggers_on_size_and_keeps_text_turns_valid() {
        let big = "x".repeat(COMPACT_AT_CHARS / 9);
        let mut contents = history(10, &big);
        // A completion-check exchange (model text, then user text) between rounds.
        contents.insert(9, json!({ "role": "model", "parts": [{ "text": "All done!" }] }));
        contents.insert(10, json!({ "role": "user", "parts": [{ "text": "[SYSTEM] Completion check" }] }));
        assert_valid_transcript(&contents);
        assert!(compact_history(&mut contents, "d", false));
        assert_valid_transcript(&contents);
        assert_eq!(tool_round_indices(&contents).len(), COMPACT_KEEP_ROUNDS);
    }

    #[test]
    pub(crate) fn slimming_trims_old_rounds_only_and_is_idempotent() {
        let big = "y".repeat(5000);
        let mut contents = history(6, &big);
        slim_old_tool_results(&mut contents, 2);
        let rounds = tool_round_indices(&contents);
        let first = &contents[rounds[0]]["parts"][0]["functionResponse"]["response"];
        assert!(first.get("trimmed").is_some());
        assert!(first["result_preview"].as_str().unwrap().chars().count() <= SLIM_TOOL_RESULT_CHARS);
        let last = &contents[*rounds.last().unwrap()]["parts"][0]["functionResponse"]["response"];
        assert_eq!(last["result"].as_str().unwrap().len(), 5000);
        let snapshot = contents.clone();
        slim_old_tool_results(&mut contents, 2);
        assert_eq!(snapshot, contents);
    }

    #[test]
    pub(crate) fn run_status_is_stripped_and_text_merges_into_user_turn() {
        let mut contents = history(2, "ok");
        contents[2]["parts"][0]["functionResponse"]["response"]["run_status"] = json!({ "step": "1 of 40" });
        strip_run_status(&mut contents);
        assert!(contents[2]["parts"][0]["functionResponse"]["response"].get("run_status").is_none());

        push_user_text(&mut contents, "note");
        assert_eq!(contents.len(), 5);
        assert_eq!(contents[4]["parts"].as_array().unwrap().len(), 2);
        contents.push(json!({ "role": "model", "parts": [{ "text": "hi" }] }));
        push_user_text(&mut contents, "note 2");
        assert_eq!(contents.len(), 7);
        assert_valid_transcript(&contents);
    }

    #[test]
    pub(crate) fn loop_detection_counts_repeats_on_unchanged_state() {
        let mut p = ProgressTracker::default();
        let args = json!({ "path": "C:/a.txt" });
        assert_eq!(p.note_call("read_file", &args), 1);
        assert!(p.observe_result("read_file", &args, &json!({ "result": "same" }), false));
        assert_eq!(p.note_call("read_file", &args), 2);
        assert!(!p.observe_result("read_file", &args, &json!({ "result": "same" }), false));
        assert_eq!(p.note_call("read_file", &args), 3);
        // A real change (an edit) resets the count for the next identical read.
        let edit = json!({ "path": "C:/a.txt", "old_text": "a", "new_text": "b" });
        p.note_call("edit_file", &edit);
        assert!(p.observe_result("edit_file", &edit, &json!({ "result": "Edited" }), false));
        assert_eq!(p.note_call("read_file", &args), 1);
        assert_eq!(p.worst_repeat(), 3);
        // Waiting is legitimately repetitive.
        assert_eq!(p.note_call("wait", &json!({ "seconds": 2 })), 0);
        assert_eq!(p.note_call("wait", &json!({ "seconds": 2 })), 0);
    }

    #[test]
    pub(crate) fn stall_counter_and_screen_changes() {
        let mut p = ProgressTracker::default();
        p.end_round(false, 1);
        p.end_round(false, 2);
        assert_eq!(p.stall_steps, 2);
        p.end_round(true, 3);
        assert_eq!((p.stall_steps, p.last_progress_step), (0, 3));

        let a = vec![100u8; (THUMB_W * THUMB_H) as usize];
        let mut b = a.clone();
        b[0] = 200; // a clock tick
        assert!(!screens_differ(&a, &b));
        b[10] = 200;
        b[20] = 0;
        assert!(screens_differ(&a, &b));
        assert!(p.observe_screen(a.clone()));
        assert!(!p.observe_screen(a.clone()));
    }

    #[test]
    pub(crate) fn screen_thumbnail_decodes_a_data_url() {
        use base64::Engine;
        let img = image::RgbImage::from_pixel(64, 40, image::Rgb([10, 200, 30]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png));
        let thumb = screen_thumbnail(&url).expect("thumbnail");
        assert_eq!(thumb.len(), (THUMB_W * THUMB_H) as usize);
        assert!(screen_thumbnail("data:image/png;base64,!!!").is_none());
    }

    pub(crate) fn ledger_with(effects: usize, errors: usize, steps: usize) -> Ledger {
        Ledger { effects, errors, steps, ..Ledger::default() }
    }

    #[test]
    pub(crate) fn completion_check_flags_unbacked_success_claims() {
        let task = "[Project folder: C:\\proj — work inside this folder and use absolute paths under it for files and shell commands.]\n\nFix the failing test";
        let c = completion_concerns("Done! I fixed the test.", &ledger_with(0, 0, 5), task, false);
        assert_eq!(c.len(), 1, "{:?}", c);
        assert!(completion_concerns("Done! I fixed the test.", &ledger_with(2, 0, 5), task, false).is_empty());
        // Last call failed → concern even with effects.
        assert_eq!(completion_concerns("Fixed it.", &ledger_with(2, 1, 5), task, true).len(), 1);
    }

    #[test]
    pub(crate) fn completion_check_accepts_plain_answers() {
        let task = "[Project folder: C:\\proj — work inside this folder.]\n\nwhat does main.rs do?";
        assert!(completion_concerns("main.rs is done in three stages: …", &ledger_with(0, 0, 3), task, false).is_empty());
        assert!(completion_concerns("It's a weather app.", &ledger_with(0, 0, 1), "what is on my screen?", false).is_empty());
    }

    #[test]
    pub(crate) fn completion_check_flags_unfinished_plan_and_early_give_up() {
        let mut l = ledger_with(3, 0, 8);
        l.apply_plan_update(&json!({ "plan": ["edit", "test", "commit"], "completed": 1 }));
        let c = completion_concerns("All set — the change is saved.", &l, "update the readme", false);
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("2 unfinished"), "{}", c[0]);

        let c = completion_concerns("I couldn't install it.", &ledger_with(0, 2, 4), "install ffmpeg", false);
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("giving up"));
        // Honest failure after a long attempt is accepted.
        assert!(completion_concerns("I couldn't install it.", &ledger_with(0, 6, 30), "install ffmpeg", false).is_empty());
    }

    #[test]
    pub(crate) fn plan_updates_track_progress_and_notes() {
        let mut l = Ledger::default();
        assert!(!l.apply_plan_update(&json!({ "plan": ["a", "b", "c"], "done_when": "tests pass" })));
        assert!(l.apply_plan_update(&json!({ "completed": 1, "notes": "port 5173" })));
        assert!(!l.apply_plan_update(&json!({ "completed": 1, "notes": "port 5173" })));
        assert!(l.apply_plan_update(&json!({ "completed": 99 })));
        assert_eq!(l.plan_done, 3);
        assert_eq!(l.notes, "port 5173");
        l.plan_done = 1;
        assert_eq!(l.plan_summary_line(), "Plan: ✓ a → b · c");
        assert_eq!(l.plan_lines()[1], "→ 2. b");
        let digest = l.digest(10);
        assert!(digest.contains("Done when: tests pass") && digest.contains("port 5173"));
    }

    #[test]
    pub(crate) fn project_folder_is_parsed_from_the_task_prefix() {
        let t = "[Project folder: C:\\Users\\me\\app — work inside this folder and use absolute paths.]\n\nrun tests";
        assert_eq!(project_folder_from_task(t).as_deref(), Some("C:\\Users\\me\\app"));
        assert_eq!(strip_project_prefix(t), "run tests");
        assert_eq!(project_folder_from_task("[Project folder: D:/x]\nhi").as_deref(), Some("D:/x"));
        assert_eq!(project_folder_from_task("run tests"), None);
    }

    #[test]
    pub(crate) fn tool_failure_detection() {
        assert!(tool_failed("read_file", &json!({ "error": "nope" })));
        assert!(tool_failed("run_command", &json!({ "output": "oops\n[exit code 3]" })));
        assert!(!tool_failed("run_command", &json!({ "output": "all good" })));
        assert!(!tool_failed("read_file", &json!({ "result": "[exit code 3]" })));
    }

    #[test]
    pub(crate) fn recovery_hints_match_common_failures() {
        let h = recovery_hint("run_command", "'pnpm' is not recognized as the name of a cmdlet, function…\n[exit code 1]").unwrap();
        assert!(h.contains("PATH"));
        assert!(recovery_hint("run_command", "command timed out after 45s").unwrap().contains("background=true"));
        assert!(recovery_hint("edit_file", "old_text was not found in C:/a.rs").unwrap().contains("read_file"));
        assert!(recovery_hint("read_file", "could not read C:/x: The system cannot find the file specified. (os error 2)")
            .unwrap()
            .contains("list_directory"));
        assert!(recovery_hint("list_windows", "something odd").is_none());
    }

    #[test]
    pub(crate) fn model_errors_are_classified() {
        assert_eq!(step_error_status("Agent step failed (503): busy"), Some(503));
        assert!(is_retryable_step_error("Agent step failed (503): busy"));
        assert!(is_retryable_step_error("Agent step failed (429) [retry-after=5]: slow down"));
        assert_eq!(retry_after_hint_secs("Agent step failed (429) [retry-after=5]: x"), Some(5));
        assert!(is_retryable_step_error("Agent step failed (408): timeout"));
        assert!(!is_retryable_step_error("Agent step failed (400): bad request"));
        assert!(!is_retryable_step_error("Agent step failed (429): You exceeded your current quota"));
        assert!(is_retryable_step_error("Can't reach the Thothai backend. Check your internet…"));
        assert!(is_retryable_step_error("Network error: error sending request for url"));
        assert!(!is_retryable_step_error("Session expired — please try again."));
        assert!(is_payload_too_large("Agent step failed (413): Payload Too Large"));
        assert!(!is_payload_too_large("Agent step failed (500): oops"));
    }
}
