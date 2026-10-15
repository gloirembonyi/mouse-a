// Mouse AI Desktop - Hover Overlay Logic
// Runs inside the transparent always-on-top overlay window. Rust shows this
// window next to the cursor on hover-dwell and sends context via events.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const pill = document.getElementById("pill");
const pillContext = document.getElementById("pill-context");
const panel = document.getElementById("panel");
const panelApp = document.getElementById("panel-app");
const panelElement = document.getElementById("panel-element");
const answerEl = document.getElementById("answer");
const thinkingEl = document.getElementById("thinking");

// The overlay never streams - the user watches this indicator for the whole model
// latency - so it is worth being the same considered orb the extension uses rather
// than three bouncing dots. If the orb scripts are absent the original dots remain.
const thinkingLabelEl = thinkingEl?.querySelector(".thinking-label");
let thinkingOrb = null;
if (thinkingEl && typeof window.GhostOrb !== "undefined" && window.GhostOrb.isAvailable()) {
  thinkingEl.querySelectorAll(".dot").forEach((d) => d.remove());
  thinkingOrb = window.GhostOrb.create(thinkingEl, { state: "working", size: 22, preset: 64, theme: "dark" });
  // Keep the existing label markup (it is styled by overlay.css) and put the orb first.
  if (thinkingLabelEl) thinkingEl.appendChild(thinkingLabelEl);
}

/** Show the indicator in the state that matches what is actually happening. */
function setThinking(on, state, label) {
  if (!thinkingEl) return;
  thinkingEl.hidden = !on;
  if (!on) { thinkingOrb?.pause(); return; }
  thinkingOrb?.resume();
  if (state) thinkingOrb?.setState(state);
  if (label && thinkingLabelEl) thinkingLabelEl.textContent = label;
}
const questionInput = document.getElementById("question");
const btnAsk = document.getElementById("btn-ask");
const btnAnalyze = document.getElementById("btn-analyze");
const btnClose = document.getElementById("btn-close");
const btnOpenApp = document.getElementById("btn-open-app");
const btnAgent = document.getElementById("btn-agent");
const btnHistory = document.getElementById("btn-history");
const agentLogEl = document.getElementById("agent-log");
const workingIndicator = document.getElementById("working-indicator");
const historyPanel = document.getElementById("history-panel");
const historyList = document.getElementById("history-list");
const historyNew = document.getElementById("history-new");
const confirmHost = document.getElementById("confirm-host");
const toast = document.getElementById("toast");
const resizeGrip = document.getElementById("resize-grip");

// Inline SVGs for the send/stop button so its state swap stays crisp (not an
// emoji glyph that renders differently per-font).
const SEND_ICON =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 2 11 13"/><path d="M22 2 15 22l-4-9-9-4 20-7Z"/></svg>';
const STOP_ICON =
  '<svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2.5"/></svg>';

const PILL_AUTOHIDE_MS = 4500;
const TOAST_AUTOHIDE_MS = 2200;

let hover = null; // latest hover-dwell payload from Rust
let expanded = false;
let busy = false;
let autoHideTimer = null;
// Default ON so the cursor pill acts like the main window: a submission runs a
// full agent task (which can now also delegate web work to the browser
// extension), not just a one-shot question. The toggle still switches back to
// quick "what's under my mouse" answers.
let agentMode = true; // when on, a request that reads as a task runs as an agent on the PC
let agentSummary = null; // live "Thought for Xs" summary widget for the in-flight run
let agentRunning = false; // an agent_run is in flight - the send button doubles as Stop
let runSource = null; // "overlay:<n>" of the in-flight run; agent-progress is filtered on it
let runSeq = 0;
let threadTurns = 0; // turns rendered in #agent-log (the conversation thread)
let lastConversationId = null; // the backend conversation the thread is in ("Continue in Mouse AI")
let lastCollapseAt = 0;
const THREAD_KEEP_MS = 5 * 60 * 1000; // reopen within this and the thread continues

// ── Events from Rust ───────────────────────────────────────
window.addEventListener("DOMContentLoaded", async () => {
  await listen("hover-dwell", (event) => onHoverDwell(event.payload));
  await listen("trigger-ask", (event) => onTriggerAsk(event.payload));
  await listen("ai-toast", (event) => onAiToast(event.payload));
  await listen("surface-theme", (event) => applySurfaceTheme(!!event.payload));
  await listen("agent-progress", (event) => {
    const p = event.payload || {};
    if (!runSource || p.source !== runSource) return;
    renderAgentProgress(p);
  });
  setupUI();
});

// Hotkey / tray "Ask at Cursor": Rust already positioned, sized, and focused
// the window - jump straight to the input panel.
function onTriggerAsk(payload) {
  hover = payload;
  showPanel({ resize: false });
}

function onHoverDwell(payload) {
  if (expanded || busy) return;
  hover = payload;

  const label = elementSummary(payload) || appSummary(payload);
  pillContext.textContent = truncate(label, 34);

  toast.hidden = true;
  panel.hidden = true;
  pill.hidden = false;

  scheduleAutoHide(PILL_AUTOHIDE_MS);
}

// Toggles the same light/dark "surface" classes the browser extension uses
// (see stealth-ai-extension/shared/adaptive-theme.js) on the root so the pill/
// panel/toast all stay readable over whatever's currently behind them.
function applySurfaceTheme(isLight) {
  document.body.classList.toggle("surface-light", isLight);
  document.body.classList.toggle("surface-dark", !isLight);
}

function onAiToast(active) {
  if (expanded) return;
  pill.hidden = true;
  panel.hidden = true;
  toast.textContent = active
    ? "🖱️✨ Mouse AI active - pause your mouse on anything to ask"
    : "Mouse AI off";
  toast.hidden = false;
  scheduleAutoHide(TOAST_AUTOHIDE_MS);
}

function scheduleAutoHide(ms) {
  if (autoHideTimer) clearTimeout(autoHideTimer);
  autoHideTimer = setTimeout(async () => {
    if (expanded || busy) return;
    pill.hidden = true;
    toast.hidden = true;
    try {
      await invoke("hide_overlay");
    } catch { /* ignore */ }
  }, ms);
}

// Makes an element draggable (moves the whole overlay window) while still
// firing a normal click when the user just presses and releases without
// moving. Deliberately NOT using data-tauri-drag-region here: Tauri's native
// drag-region invokes the OS window-move on mousedown for ANY click, which on
// Windows reliably swallows the paired click on a real <button> (the OS's
// caption-drag loop takes over the mouse) - the exact "I can't click it
// anymore" symptom. Instead: only actually start the native drag once the
// pointer has moved past a small threshold, so a real click never triggers it.
function makeDraggableClickable(el, onClick) {
  const DRAG_THRESHOLD = 4;
  let pressed = false;
  let dragging = false;
  let startX = 0;
  let startY = 0;

  el.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    pressed = true;
    dragging = false;
    startX = e.clientX;
    startY = e.clientY;
  });

  window.addEventListener("mousemove", (e) => {
    if (!pressed || dragging) return;
    if (Math.abs(e.clientX - startX) > DRAG_THRESHOLD || Math.abs(e.clientY - startY) > DRAG_THRESHOLD) {
      dragging = true;
      window.__TAURI__.window.getCurrentWindow().startDragging().catch(() => {});
    }
  });

  window.addEventListener("mouseup", () => {
    pressed = false;
  });

  el.addEventListener("click", (e) => {
    if (dragging) {
      // Tail end of a drag gesture, not a real click - swallow it.
      e.preventDefault();
      e.stopPropagation();
      dragging = false;
      return;
    }
    onClick(e);
  });
}

// ── UI wiring ──────────────────────────────────────────────
function setupUI() {
  makeDraggableClickable(pill, expand);
  btnClose.addEventListener("click", closePanel);
  btnOpenApp.addEventListener("click", () => openInMain());
  btnAsk.addEventListener("click", () => {
    if (agentRunning) {
      // The send button is a Stop button while an agent task runs.
      btnAsk.disabled = true;
      questionInput.placeholder = "Stopping…";
      invoke("agent_cancel", { source: runSource || "overlay" }).catch(() => {});
      return;
    }
    submit();
  });
  // The eye is always "explain what's here" - a plain look, never an agent run.
  btnAnalyze.addEventListener("click", () => askQuestion(null));
  btnAgent.addEventListener("click", toggleAgentMode);
  btnHistory.addEventListener("click", toggleHistory);
  historyNew.addEventListener("click", startNewChatFromOverlay);
  btnMention?.addEventListener("click", () => (mentionMenu.hidden ? openMentionMenu("") : closeMentionMenu()));
  // Reflect the default agent-mode state in the button + placeholder at startup.
  btnAgent.classList.toggle("active", agentMode);
  questionInput.placeholder = placeholderFor();

  questionInput.addEventListener("keydown", (e) => {
    if (!mentionMenu.hidden && handleMentionKey(e)) return;
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    } else if (e.key === "Backspace" && !questionInput.value && chips.length) {
      removeChip(chips.length - 1);
    }
  });
  questionInput.addEventListener("input", () => {
    const m = /(^|\s)@([\w.-]*)$/.exec(questionInput.value.slice(0, questionInput.selectionStart ?? questionInput.value.length));
    if (m) openMentionMenu(m[2]);
    else if (!mentionMenu.hidden) closeMentionMenu();
  });

  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (!mentionMenu.hidden) { closeMentionMenu(); return; }
    if (!confirmHost.hidden) return; // the pending confirm owns Esc (Deny)
    if (agentRunning) {
      // Esc = Stop while a task runs (the same key stops it from anywhere).
      btnAsk.disabled = true;
      questionInput.placeholder = "Stopping…";
      invoke("agent_cancel", { source: runSource || "overlay" }).catch(() => {});
      return;
    }
    if (!busy) collapse(); // a plain ask in flight just finishes
  });

  // Manual resize handle - the window is also natively resizable from its
  // edges (tauri.conf.json's overlay window has resizable:true), but that's
  // barely discoverable on a borderless window, so this corner grip gives an
  // explicit affordance for making the panel bigger.
  resizeGrip?.addEventListener("mousedown", async (e) => {
    e.preventDefault();
    try {
      const { getCurrentWindow } = window.__TAURI__.window;
      await getCurrentWindow().startResizeDragging("SouthEast");
    } catch { /* ignore */ }
  });

  // Clicking elsewhere on the desktop used to auto-collapse the panel - lost
  // in-progress research/answers the moment the user glanced at another
  // window. The panel now stays open regardless of focus; ✕ / Esc / the
  // pill's own collapse() are the only ways to dismiss it. Alt+Tab-ing back
  // to it just works since the window was never hidden.
}

async function expand() {
  showPanel({ resize: true });
}

// The window used to be resized to hardcoded pixel constants (380/460/480)
// regardless of how much text actually came back - a one-line answer left a
// large blank gap below it up to that fixed height. Instead, measure how
// tall the panel's content actually wants to be (by briefly letting it hug
// its content instead of stretching to fill the window) and resize the
// native window to match, capped so a very long answer scrolls instead of
// growing the window off-screen.
//
// A modest floor so a bare "Ask" state (header + input) still looks like a
// deliberate compact panel rather than a sliver - but low enough that the
// window truly hugs short answers instead of leaving a large empty gap above
// the input (the content areas are sized to content + capped, not flex:1, so
// the measured height is the real content height).
const MIN_OVERLAY_HEIGHT = 132;
// #root's 6px padding top+bottom plus the panel border: the window used to be
// sized to the panel's content height alone, so the bottom ~14px - the lower
// half of the input row - was clipped whenever nothing scrollable was visible.
const ROOT_CHROME_PX = 14;
let sizeRaf = 0;

/**
 * Size the window to the panel's content, capped. `growOnly` (progress events,
 * follow-up turns) never makes the window smaller than it is now - so a window
 * the user dragged bigger stays bigger, and the input never jumps off-screen.
 * Coalesced to one native resize per frame.
 */
function syncOverlaySize(maxHeight = 560, { growOnly = false } = {}) {
  if (!panel || panel.hidden) return Promise.resolve();
  return new Promise((resolve) => {
    cancelAnimationFrame(sizeRaf);
    sizeRaf = requestAnimationFrame(async () => {
      const prevHeight = panel.style.height;
      panel.style.height = "auto";
      const natural = panel.scrollHeight + ROOT_CHROME_PX;
      panel.style.height = prevHeight;
      const current = window.innerHeight;
      const cap = Math.max(maxHeight, growOnly ? current : 0, MIN_OVERLAY_HEIGHT);
      const height = Math.min(cap, Math.max(MIN_OVERLAY_HEIGHT, natural));
      if (growOnly && height <= current + 2) { resolve(); return; }
      const width = Math.max(480, growOnly ? window.innerWidth : 0);
      try {
        await invoke("expand_overlay", { width, height });
      } catch { /* ignore */ }
      resolve();
    });
  });
}

async function showPanel({ resize }) {
  if (!hover) return;
  expanded = true;
  if (autoHideTimer) clearTimeout(autoHideTimer);

  pill.hidden = true;
  toast.hidden = true;
  panel.hidden = false;

  panelApp.textContent = appSummary(hover) || "Mouse AI";
  panelElement.textContent = truncate(elementSummary(hover), 60);
  answerEl.hidden = true;
  answerEl.innerHTML = "";
  setThinking(false);
  questionInput.value = "";
  closeMentionMenu();
  // The thread survives a quick close/reopen (a follow-up a minute later is
  // the normal case); after a longer gap this is a new question about
  // something else, so start a fresh thread AND a fresh server conversation.
  const stale = threadTurns > 0 && Date.now() - lastCollapseAt > THREAD_KEEP_MS;
  if (stale) resetThread({ newConversation: true });
  agentLogEl.hidden = threadTurns === 0;
  renderQuickRow();

  if (resize) {
    await syncOverlaySize(threadTurns ? answerHeight() : 380);
  }
  // The input is the whole point of opening the panel: visible, focused, cursor ready.
  requestAnimationFrame(() => {
    questionInput.focus();
    questionInput.scrollIntoView?.({ block: "nearest" });
  });
}

function resetThread({ newConversation } = {}) {
  agentLogEl.innerHTML = "";
  agentLogEl.hidden = true;
  threadTurns = 0;
  clearChips();
  renderQuickRow();
  if (newConversation) {
    lastConversationId = null;
    invoke("new_conversation").catch(() => {});
  }
}

/** Open the main window on this thread's conversation (falls back to the current one). */
function openInMain() {
  invoke("open_in_main", { conversationId: lastConversationId }).catch(() => invoke("show_main_window").catch(() => {}));
}

/** Copy through the OS clipboard (Rust) first; the browser API is the fallback. */
async function copyText(text) {
  try {
    await invoke("copy_text", { text: String(text || "") });
    return true;
  } catch {
    try {
      await navigator.clipboard.writeText(String(text || ""));
      return true;
    } catch {
      return false;
    }
  }
}

/** ✕: stop a running task first, then put the panel away. */
async function closePanel() {
  if (agentRunning) invoke("agent_cancel", { source: runSource || "overlay" }).catch(() => {});
  await collapse();
}

async function collapse() {
  expanded = false;
  busy = false;
  lastCollapseAt = Date.now();
  panel.hidden = true;
  pill.hidden = true;
  toast.hidden = true;
  answerEl.innerHTML = "";
  // The thread itself is kept (see showPanel) - only the transient surfaces reset.
  questionInput.value = "";
  closeMentionMenu();
  // Reset the transient overlay surfaces so a re-expand starts clean.
  hideHistory();
  clearPinnedConfirm();
  setWorkingIndicator(false);
  btnHistory.classList.remove("active");
  setBusy(false);
  try {
    await invoke("collapse_overlay");
  } catch { /* ignore */ }
}

function toggleAgentMode() {
  agentMode = !agentMode;
  btnAgent.classList.toggle("active", agentMode);
  btnAgent.title = agentMode
    ? "Auto: questions are answered, tasks are done on your PC (click for ask-only)"
    : "Ask only: every message is answered, nothing is done on your PC (click for auto)";
  questionInput.placeholder = placeholderFor();
}

function placeholderFor() {
  return agentMode ? "Ask anything or tell Mouse AI what to do…  @ to attach" : "Ask about what's under your mouse…  @ to attach";
}

// The panel only ever showed the ANSWER, never what was actually asked - easy
// to lose track of, especially once the panel's grown to fit a long reply.
// Echoed above the answer/log for both the plain-ask and agent-task paths.
function questionHeaderHtml(question) {
  if (!question) return "";
  return `<div class="asked-question">${escapeHtml(question)}</div>`;
}

function formatDuration(ms) {
  const totalSeconds = Math.max(1, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes > 0 ? `${minutes}m ${seconds}s` : `${seconds}s`;
}

// ── Routing: question or task? ─────────────────────────────
// With Auto on (the sparkle), a question gets a fast answer - with web search
// when it needs the web - and an instruction runs as an agent on the PC. Ask-only
// mode answers everything. There is no model call in the decision: it has to be
// instant, and a wrong guess is one toggle away.
const QUESTION_START = /^(what|what's|whats|why|how|how's|who|who's|whose|when|where|which|is|are|was|were|can|could|does|do|did|should|would|will|explain|summarize|summarise|translate|define|describe|tell me|compare|meaning|difference|search|look up|lookup|research|find out|google)\b/i;
const TASK_START = /^(open|close|click|create|make|write|type|send|run|install|go to|goto|navigate|download|save|delete|remove|move|copy|paste|rename|set|turn|start|stop|play|pause|fix|build|deploy|update|upgrade|add|edit|change|fill|submit|book|order|buy|schedule|reply|email|message|organize|organise|sort|clean|format|convert|resize|launch|switch|minimize|maximize|scroll|select|drag|press|put|insert|do|help me|please)\b/i;
const WEB_HINT = /\b(latest|current|today|tonight|yesterday|this week|news|price|prices|cost|weather|forecast|stock|score|scores|release|released|version|who is|who was|when did|when is|search|look up|lookup|google|research|find out|20[2-9]\d|how much|near me|review|reviews|best .+ (for|in|of)|vs\.?|versus|compare)\b/i;
const HOVER_HINT = /\b(this|here|that|screen|window|page|button|error|under (my|the) (mouse|cursor)|what('s| is) (this|that|here)|selected|highlighted)\b/i;

function routeFor(text) {
  if (!agentMode) return "ask";
  const t = String(text || "").trim();
  if (!t) return "ask";
  if (chips.some((c) => c.kind === "web")) return "ask";
  if (TASK_START.test(t) && !/^(how|what|which|can you tell|do you know)\b/i.test(t)) return "agent";
  if (/\?\s*$/.test(t) || QUESTION_START.test(t) || WEB_HINT.test(t)) return "ask";
  return "agent";
}

function needsWeb(text) {
  if (chips.some((c) => c.kind === "web")) return true;
  const t = String(text || "");
  // Something on screen is the subject -> the screenshot answers it, not the web.
  if (HOVER_HINT.test(t) && !/\b(search|look up|latest|news|price)\b/i.test(t)) return false;
  return WEB_HINT.test(t);
}

// A pasted URL means "read this page": it becomes a link chip, resolved at send time.
function attachLinksIn(text) {
  const urls = (text.match(/https?:\/\/[^\s<>"')\]]+/g) || []).slice(0, 3);
  for (const url of urls) {
    if (chips.some((c) => c.kind === "link" && c.path === url)) continue;
    let host = url;
    try { host = new URL(url).host; } catch { /* keep the url */ }
    chips.push({ kind: "link", label: host, path: url });
  }
  if (urls.length) renderChips();
}

function submit() {
  const text = questionInput.value.trim();
  if (!text && !chips.length) return;
  attachLinksIn(text);
  if (routeFor(text) === "agent") runAgentTask(text);
  else askQuestion(text || null, { webSearch: needsWeb(text) });
}

// ── Thread turns ───────────────────────────────────────────
// Every question and every task is one turn in the same scrollable thread: the
// question chip, then the answer (or the run summary and its final answer). A
// follow-up appends below instead of wiping what came before.
function newTurn(question, attachments) {
  answerEl.hidden = true;
  answerEl.innerHTML = "";
  hideHistory();
  agentLogEl.hidden = false;
  const turn = document.createElement("div");
  turn.className = "turn";
  const chipsHtml = attachments.length
    ? `<div class="turn-attach">${attachments.map((a) => `<span class="turn-chip">${escapeHtml(chipIcon(a.kind))} ${escapeHtml(a.label)}</span>`).join("")}</div>`
    : "";
  turn.innerHTML = questionHeaderHtml(question || "What is this?") + chipsHtml;
  agentLogEl.appendChild(turn);
  threadTurns += 1;
  renderQuickRow();
  requestAnimationFrame(() => turn.scrollIntoView({ block: "start", behavior: "smooth" }));
  return turn;
}

/** Resolve the @-mention chips into text + an image, right now. */
async function resolveAttachments() {
  const out = { text: [], image: null, labels: [] };
  for (const chip of chips) {
    if (chip.kind === "web") { out.labels.push({ kind: "web", label: "Web" }); continue; }
    try {
      const r = chip.resolved || (await invoke("resolve_mention", { kind: chip.kind, x: hover?.x, y: hover?.y, path: chip.path || null, targetHwnd: hover?.hwnd || 0 }));
      if (r.text) out.text.push(r.text);
      if (r.image_data_url && !out.image) out.image = r.image_data_url;
      out.labels.push({ kind: chip.kind, label: r.label || chip.label });
    } catch (err) {
      out.text.push(`[${chip.kind} could not be attached: ${String(err?.message || err)}]`);
      out.labels.push({ kind: chip.kind, label: chip.label });
    }
  }
  return out;
}

async function askQuestion(question, { webSearch = false } = {}) {
  if (!hover || busy) return;
  setBusy(true);
  questionInput.value = "";
  closeMentionMenu();
  const attach = await resolveAttachments();
  clearChips();
  const turn = newTurn(question, attach.labels);
  const body = document.createElement("div");
  body.className = "turn-body";
  turn.appendChild(body);
  setThinking(true, "shaping", webSearch ? "Searching the web…" : attach.image ? "Reading what you attached…" : "Looking at your screen…");
  syncOverlaySize(420, { growOnly: true });

  try {
    const result = await withTimeout(
      invoke("ask_about_hover", {
        question,
        x: hover.x,
        y: hover.y,
        appName: hover.context?.process_name || null,
        windowTitle: hover.context?.window_title || null,
        elementText: elementContextForAI(hover) || null,
        webSearch,
        extraContext: attach.text.join("\n\n") || null,
        imageDataUrl: attach.image,
        targetHwnd: hover.hwnd || 0,
      }),
      webSearch ? 90000 : 55000
    );
    if (result.conversation_id) lastConversationId = result.conversation_id;
    body.innerHTML = `<div class="answer-body">${renderMarkdown(result.text || "(no answer)")}</div>` + answerActionsHtml();
  } catch (err) {
    body.innerHTML = `<div class="error-box">⚠️ ${escapeHtml(friendlyError(err))}</div>`;
  } finally {
    setThinking(false);
    setBusy(false);
    questionInput.value = "";
    questionInput.focus();
    requestAnimationFrame(() => turn.scrollIntoView({ block: "start", behavior: "smooth" }));
    await syncOverlaySize(answerHeight(), { growOnly: true });
  }
}

// Full agent task, reporting live activity into the overlay and the final
// answer in the same thread the user asked from.
async function runAgentTask(task) {
  if (!hover || busy) return;
  if (!task) {
    questionInput.placeholder = "Type what you'd like me to do first…";
    questionInput.focus();
    return;
  }
  agentRunning = true;
  const mySource = `overlay:${++runSeq}`;
  runSource = mySource;
  setBusy(true);
  setWorkingIndicator(true);
  hideHistory();
  clearPinnedConfirm();
  questionInput.value = "";
  closeMentionMenu();
  setThinking(false);
  const attach = await resolveAttachments();
  clearChips();
  const turn = newTurn(task, attach.labels);
  agentSummary = createAgentSummary();
  turn.appendChild(agentSummary.root);
  await syncOverlaySize(480, { growOnly: true });

  const fullTask = attach.text.length ? `${task}\n\n[ATTACHED BY THE USER]\n${attach.text.join("\n\n")}` : task;
  try {
    const result = await invoke("agent_run", {
      task: fullTask,
      x: hover.x,
      y: hover.y,
      source: mySource,
      autonomous: true,
      imageDataUrl: attach.image,
      targetHwnd: hover.hwnd || 0,
    });
    finishAgentSummary(true);
    if (result.conversation_id) lastConversationId = result.conversation_id;
    appendFinalAnswer(`<div class="answer-body">${renderMarkdown(result.text || "(done)")}</div>` + answerActionsHtml(), turn);
  } catch (err) {
    finishAgentSummary(false);
    appendFinalAnswer(`<div class="error-box">⚠️ ${escapeHtml(friendlyError(err))}</div>`, turn);
  } finally {
    if (runSource === mySource) runSource = null;
    agentRunning = false;
    setWorkingIndicator(false);
    clearPinnedConfirm();
    setBusy(false);
    questionInput.value = "";
    questionInput.focus();
    // Size to the answer (up to ~75% of the screen) so it's readable
    // without dragging the window bigger.
    await syncOverlaySize(answerHeight(), { growOnly: true });
  }
}

// The panel border animates and a "don't touch" banner shows while the agent
// is acting on the machine - a clear, unmissable "AI is driving" signal so the
// user doesn't grab the mouse/keyboard and fight it mid-run.
function setWorkingIndicator(on) {
  panel.classList.toggle("agent-running", on);
  workingIndicator.hidden = !on;
}

// ── Chat history (from the cursor overlay) ─────────────────────────
// The overlay could only ask/act - you couldn't get back to earlier chats
// without opening the main window. This lets you browse recent conversations
// and jump into one (its transcript loads here, and the next message continues
// that thread) right from the pill.
let historyOpen = false;

function hideHistory() {
  historyOpen = false;
  historyPanel.hidden = true;
  btnHistory.classList.remove("active");
}

async function toggleHistory() {
  if (historyOpen) { hideHistory(); await syncOverlaySize(480); return; }
  if (agentRunning) return; // don't switch threads mid-run
  historyOpen = true;
  historyPanel.hidden = false;
  btnHistory.classList.add("active");
  // History replaces the content area - never stacks on top of a previous
  // answer/log (which is how the two used to end up visible at once).
  answerEl.hidden = true;
  agentLogEl.hidden = true;
  setThinking(false);
  clearPinnedConfirm();
  historyList.innerHTML = `<div class="history-empty">Loading…</div>`;
  await syncOverlaySize(520);
  try {
    const convos = await invoke("get_conversations", { limit: 25 });
    renderHistoryList(convos || []);
  } catch (err) {
    historyList.innerHTML = `<div class="history-empty">${escapeHtml(friendlyError(err))}</div>`;
  }
  await syncOverlaySize(520);
}

function renderHistoryList(convos) {
  if (!convos.length) {
    historyList.innerHTML = `<div class="history-empty">No saved chats yet.</div>`;
    return;
  }
  historyList.innerHTML = "";
  for (const c of convos) {
    const item = document.createElement("button");
    item.className = "history-item";
    const title = cleanTitle(c.title);
    const when = relativeTime(c.updated_at || c.created_at);
    item.innerHTML =
      `<span class="history-item-ic" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"/></svg></span>` +
      `<span class="history-item-text">` +
      `<span class="history-item-title">${escapeHtml(title)}</span>` +
      `<span class="history-item-meta">${escapeHtml(when)}</span>` +
      `</span>` +
      `<span class="history-item-chevron" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg></span>`;
    item.addEventListener("click", () => openConversation(c.id, title));
    historyList.appendChild(item);
  }
}

async function openConversation(id, title) {
  hideHistory();
  answerEl.hidden = false;
  agentLogEl.hidden = true;
  answerEl.innerHTML = `<div class="history-loading">Loading “${escapeHtml(title)}”…</div>`;
  await syncOverlaySize(560);
  try {
    await invoke("set_conversation_id", { id });
    lastConversationId = id;
    const messages = await invoke("get_conversation_messages", { id, limit: 40 });
    renderTranscript(title, messages || []);
  } catch (err) {
    answerEl.innerHTML = `<div class="error-box">⚠️ ${escapeHtml(friendlyError(err))}</div>`;
  }
  // A transcript deserves room to read - a taller cap so it rarely needs a
  // manual resize to see the whole conversation.
  await syncOverlaySize(600);
}

// Titles and stored user messages carry the agent's internal prompt scaffolding
// (hover context, "[TASK]" markers, "…on their screen and asks:"). None of that
// belongs in a human-facing list/transcript - reduce it to what the user meant.
function cleanUserText(raw) {
  let s = String(raw || "");
  const taskIdx = s.lastIndexOf("[TASK]");
  if (taskIdx !== -1) s = s.slice(taskIdx + "[TASK]".length);
  const asksIdx = s.toLowerCase().lastIndexOf("asks:");
  if (asksIdx !== -1) s = s.slice(asksIdx + "asks:".length);
  s = s
    .split("\n")
    .filter((line) => !/^\s*(\[[A-Z][A-Z0-9 _\-]+\]|Application:|Window:|Cursor position:|URL:|Page:|Selected text)/.test(line))
    .join("\n")
    .trim();
  return s;
}

function cleanTitle(rawTitle) {
  const raw = String(rawTitle || "").trim();
  if (!raw) return "Untitled chat";
  const looksScaffolded = /hovering over this|desktop hover context|\[task\]|\[current page\]|application:\s|cursor position:/i.test(raw);
  const base = looksScaffolded ? cleanUserText(raw) : raw;
  const t = base.replace(/\s+/g, " ").trim() || "Chat";
  return t.length > 46 ? t.slice(0, 46).trimEnd() + "…" : t;
}

function renderTranscript(title, messages) {
  const rows = messages
    .map((m) => {
      const isUser = m.role === "user";
      const who = isUser ? "You" : "Mouse AI";
      const cls = isUser ? "transcript-user" : "transcript-ai";
      const text = isUser ? cleanUserText(m.content) : (m.content || "");
      if (!text.trim()) return "";
      return `<div class="transcript-row ${cls}"><span class="transcript-who">${who}</span><div class="transcript-bubble">${renderMarkdown(text)}</div></div>`;
    })
    .join("");
  answerEl.innerHTML =
    `<div class="transcript-head"><span class="transcript-title">${escapeHtml(title)}</span><span class="transcript-hint">Continuing this chat - type below</span></div>` +
    `<div class="transcript-body">${rows || `<div class="history-empty">This chat has no messages.</div>`}</div>`;
  // Reading a past chat starts at the BEGINNING, not the last reply - scrolling
  // to the bottom is what made an opened chat look like only its final message.
  requestAnimationFrame(() => { answerEl.scrollTop = 0; });
}

async function startNewChatFromOverlay() {
  hideHistory();
  btnHistory.classList.remove("active");
  answerEl.hidden = true;
  answerEl.innerHTML = "";
  resetThread({ newConversation: true });
  questionInput.focus();
  await syncOverlaySize(380);
}

// "3m ago" / "2h ago" / "5d ago" from an ISO timestamp; falls back to the raw
// date string if it can't parse.
function relativeTime(iso) {
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return iso || "";
  const secs = Math.max(0, Math.round((Date.now() - t) / 1000));
  if (secs < 60) return "just now";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  const days = Math.round(hrs / 24);
  return days < 30 ? `${days}d ago` : new Date(t).toLocaleDateString();
}

// Cursor/Codex-style collapsible "Thought for Xs" widget - the raw step-by-
// step tool log defaults to hidden inside it (expanded automatically only
// while running or when a confirm prompt needs the user's attention), so the
// popup reads as a clean one-line status instead of a wall of tool-call text.
function createAgentSummary() {
  const root = document.createElement("div");
  root.className = "agent-summary";

  const header = document.createElement("button");
  header.className = "agent-summary-header";
  header.innerHTML = `
    <span class="agent-summary-icon"><span class="agent-summary-spinner"></span></span>
    <span class="agent-summary-text">
      <span class="agent-summary-label is-live">Understanding your request…</span>
      <span class="agent-summary-meta"><span class="agent-summary-duration"></span><span class="agent-summary-count"></span></span>
    </span>
    <span class="agent-summary-chevron">▾</span>`;

  const body = document.createElement("div");
  body.className = "agent-summary-body";
  // Visible from the first frame: the model's first decision can take
  // several seconds, and an empty box read as "stuck".
  const placeholder = document.createElement("div");
  placeholder.className = "agent-step agent-step-placeholder";
  placeholder.innerHTML = `<div class="agent-step-rail"><span class="agent-step-icon">•</span></div><div class="agent-step-body is-live">Planning the steps…</div>`;
  body.appendChild(placeholder);

  root.appendChild(header);
  root.appendChild(body);

  const state = {
    root,
    header,
    body,
    icon: header.querySelector(".agent-summary-icon"),
    label: header.querySelector(".agent-summary-label"),
    duration: header.querySelector(".agent-summary-duration"),
    chevron: header.querySelector(".agent-summary-chevron"),
    count: header.querySelector(".agent-summary-count"),
    placeholder,
    steps: 0,
    startedAt: Date.now(),
    tickInterval: null,
    userCollapsed: null, // null = auto (follow running/confirm state); true/false once the user toggles it
    hasPendingConfirm: false,
  };

  const applyExpanded = () => {
    const expand = state.userCollapsed === null ? state.running !== false || state.hasPendingConfirm : !state.userCollapsed;
    body.hidden = !expand;
    state.chevron.style.transform = expand ? "rotate(180deg)" : "rotate(0deg)";
  };
  state.applyExpanded = applyExpanded;
  state.running = true;
  applyExpanded();

  header.addEventListener("click", () => {
    const currentlyExpanded = !body.hidden;
    state.userCollapsed = currentlyExpanded;
    applyExpanded();
    syncOverlaySize(480, { growOnly: true });
  });

  if (typeof window.GhostOrb !== "undefined" && window.GhostOrb.isAvailable()) {
    state.icon.innerHTML = "";
    state.orb = window.GhostOrb.create(state.icon, { state: "solving", size: 20, preset: 20 });
  }

  state.tickInterval = setInterval(() => {
    state.duration.textContent = `for ${formatDuration(Date.now() - state.startedAt)}`;
  }, 1000);

  return state;
}

function finishAgentSummary(success) {
  if (!agentSummary) return;
  clearInterval(agentSummary.tickInterval);
  agentSummary.running = false;
  try { agentSummary.orb?.destroy?.(); } catch { /* orb already gone */ }
  agentSummary.placeholder?.remove();
  agentSummary.label.classList.remove("is-live");
  agentSummary.label.textContent = success ? "Done" : "Stopped";
  if (agentSummary.steps) agentSummary.count.textContent = ` · ${agentSummary.steps} step${agentSummary.steps === 1 ? "" : "s"}`;
  agentSummary.duration.textContent = `for ${formatDuration(Date.now() - agentSummary.startedAt)}`;
  agentSummary.icon.innerHTML = success ? "✓" : "✕";
  agentSummary.icon.classList.toggle("failed", !success);
  if (agentSummary.userCollapsed === null) agentSummary.applyExpanded();
  agentSummary = null;
}

// Appends the final answer (or error box) below the now-collapsed "Thought
// for Xs" summary, inside the same scrollable log - then scrolls down so the
// answer is what's actually in view, not left scrolled up at the old step
// log's position.
function appendFinalAnswer(html, turn) {
  const wrap = document.createElement("div");
  wrap.className = "agent-final-answer";
  wrap.innerHTML = html;
  (turn || agentLogEl).appendChild(wrap);
  requestAnimationFrame(() => {
    agentLogEl.scrollTop = agentLogEl.scrollHeight;
  });
}

function renderAgentProgress(p) {
  if (agentLogEl.hidden || !agentSummary) return;
  if (p.kind === "done") return;

  if (p.kind === "confirm") {
    showPinnedConfirm(p);
    return;
  }

  agentSummary.placeholder?.remove();
  agentSummary.body.querySelector(".agent-step.is-current")?.classList.remove("is-current");
  const stepEl = p.kind === "phase-summary" ? buildPhaseSummary(p) : buildAgentStep(p);
  agentSummary.body.appendChild(stepEl);
  // The header always says what's happening RIGHT NOW, so the run is
  // readable at a glance even if the step list is scrolled or collapsed.
  if (p.kind === "tool" || p.kind === "thinking" || p.kind === "plan") {
    stepEl.classList.add("is-current");
    const line = String(p.message || "").replace(/[`*_]/g, "").split("\n")[0].trim();
    if (line) agentSummary.label.textContent = line.length > 70 ? line.slice(0, 69) + "…" : line;
  }
  if (p.kind === "tool") {
    agentSummary.steps += 1;
    agentSummary.count.textContent = ` · ${agentSummary.steps} step${agentSummary.steps === 1 ? "" : "s"}`;
  }
  agentSummary.body.scrollTop = agentSummary.body.scrollHeight;
  agentLogEl.scrollTop = agentLogEl.scrollHeight;
  syncOverlaySize(480, { growOnly: true });
}

// A rollup ("Explored 12 files, 3 searches +367 -14") isn't a timeline entry
// the way a tool call is - no icon/rail, just a small muted recap line under
// whatever heading it summarizes (same as Cursor's grey rollup lines).
function buildPhaseSummary(p) {
  const div = document.createElement("div");
  div.className = "agent-step kind-phase-summary";
  div.textContent = p.message;
  return div;
}

// Icon badge + connecting rail beside the message - same timeline shape as
// the main window's buildAgentStep (src/main.js), so both surfaces read as
// one product instead of two different-looking activity logs.
function buildAgentStep(p) {
  const step = document.createElement("div");
  step.className = `agent-step kind-${p.kind}`;

  const rail = document.createElement("div");
  rail.className = "agent-step-rail";
  const icon = document.createElement("span");
  icon.className = "agent-step-icon";
  icon.textContent = { plan: "💭", tool: "⚙", "tool-result": "↳", error: "!" }[p.kind] || "•";
  const line = document.createElement("span");
  line.className = "agent-step-line";
  rail.appendChild(icon);
  rail.appendChild(line);

  const body = document.createElement("div");
  body.className = "agent-step-body";
  // Same friendly formatting the final error box uses - the log is normally
  // hidden before an error-kind row would be seen, but this covers the race
  // between the agent-progress event arriving and the invoke() rejecting.
  // Rendered as inline markdown (not raw textContent) so `code` and **bold**
  // in the model's own narration show up formatted instead of as literal
  // backticks/asterisks.
  body.innerHTML = renderInlineMarkdown(p.kind === "error" ? friendlyError(p.message) : p.message);

  step.appendChild(rail);
  step.appendChild(body);
  return step;
}

// Approval requests used to be appended into the scrollable step log, where a
// long run pushed them below the fold - the user had to scroll/expand to even
// see the Allow/Deny buttons. Now they render into a pinned host that sits
// above the input and is ALWAYS visible, slides in with a pulse, and auto-
// focuses Allow (Enter = Allow, Esc = Deny) so a decision is one keypress away.
let confirmKeyHandler = null;

function showPinnedConfirm(p) {
  clearPinnedConfirm();
  // While waiting on the human, the "don't touch" banner is wrong - the user
  // MUST interact to answer. Hide it for the duration of the prompt.
  workingIndicator.hidden = true;

  const prompt = buildConfirmPrompt(p);
  confirmHost.innerHTML = "";
  confirmHost.appendChild(prompt);
  confirmHost.hidden = false;
  confirmHost.classList.remove("confirm-enter");
  // Restart the entrance animation each time a new prompt appears.
  void confirmHost.offsetWidth;
  confirmHost.classList.add("confirm-enter");

  const allowBtn = prompt.querySelector(".agent-confirm-btn.allow");
  const denyBtn = prompt.querySelector(".agent-confirm-btn.deny");
  requestAnimationFrame(() => allowBtn?.focus());

  confirmKeyHandler = (e) => {
    if (e.key === "Enter") { e.preventDefault(); allowBtn?.click(); }
    else if (e.key === "Escape") { e.preventDefault(); denyBtn?.click(); }
  };
  document.addEventListener("keydown", confirmKeyHandler, true);

  syncOverlaySize(480, { growOnly: true });
}

function clearPinnedConfirm() {
  if (confirmKeyHandler) {
    document.removeEventListener("keydown", confirmKeyHandler, true);
    confirmKeyHandler = null;
  }
  confirmHost.hidden = true;
  confirmHost.innerHTML = "";
  confirmHost.classList.remove("confirm-enter");
}

// Real Allow/Deny prompt the agent is waiting on - the click is reported
// straight back to the paused tool call in Rust via agent_confirm.
function buildConfirmPrompt(p) {
  const wrap = document.createElement("div");
  wrap.className = "agent-confirm";

  const msg = document.createElement("div");
  msg.className = "agent-confirm-msg";
  msg.textContent = `🔒 ${p.message}`;
  wrap.appendChild(msg);

  const row = document.createElement("div");
  row.className = "agent-confirm-buttons";

  const allowBtn = document.createElement("button");
  allowBtn.className = "agent-confirm-btn allow";
  allowBtn.textContent = "Allow";

  const denyBtn = document.createElement("button");
  denyBtn.className = "agent-confirm-btn deny";
  denyBtn.textContent = "Deny";

  const decide = async (approved) => {
    allowBtn.disabled = true;
    denyBtn.disabled = true;
    if (confirmKeyHandler) {
      document.removeEventListener("keydown", confirmKeyHandler, true);
      confirmKeyHandler = null;
    }
    try {
      await invoke("agent_confirm", { requestId: p.request_id, approved });
    } catch { /* ignore */ }
    // Briefly show the outcome, then dismiss the pinned host and - if the run
    // is still going - restore the "AI working" banner.
    msg.textContent = approved ? `✅ Allowed - continuing…` : `🚫 Denied - stopping that action.`;
    row.remove();
    setTimeout(() => {
      clearPinnedConfirm();
      if (agentRunning) workingIndicator.hidden = false;
      syncOverlaySize(480, { growOnly: true });
    }, 900);
  };

  allowBtn.addEventListener("click", () => decide(true));
  denyBtn.addEventListener("click", () => decide(false));
  row.appendChild(allowBtn);
  row.appendChild(denyBtn);
  wrap.appendChild(row);
  return wrap;
}

function withTimeout(promise, ms) {
  return Promise.race([
    promise,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error("Request timed out - the backend didn't respond.")), ms)
    ),
  ]);
}

function friendlyError(err) {
  const msg = String(err?.message || err);
  if (/not signed in/i.test(msg)) {
    return "You're not signed in. Click ⤢ to open the Mouse AI window and sign in first.";
  }
  if (/can't reach|network|timed out|connect/i.test(msg)) {
    return "Can't reach the Thothai backend. Check your internet or the Backend URL in Settings (⤢ → ⚙).";
  }
  if (/empty response/i.test(msg)) {
    return "The model didn't respond after several tries - it may be under heavy load. Try again in a moment.";
  }
  if (/step limit/i.test(msg)) {
    return "That task took more steps than I could finish in one go. Try breaking it into smaller steps.";
  }
  if (/daily message limit|daily limit/i.test(msg)) {
    return "You've hit today's free message limit. It resets daily - try again tomorrow.";
  }
  if (/referer|permission_denied|rejected every configured api key|api key not valid/i.test(msg)) {
    return "The backend's Gemini API key is misconfigured (not something you can fix here) - let the app owner know it needs a working key.";
  }
  // Distinct from a transient rate limit: a quota/billing error means the
  // account's Gemini quota is genuinely exhausted - retrying can't fix it.
  if (/exceeded your current quota|check your plan and billing|quota_exceeded|resource_exhausted/i.test(msg)) {
    return "The Gemini API quota/billing limit has been reached - this won't resolve by retrying. Check the plan/billing for the API key in use, or switch to a different key in Settings.";
  }
  if (/429|rate limit/i.test(msg)) {
    return "Too many requests right now - wait a few seconds and try again.";
  }
  return msg;
}

function setBusy(state) {
  busy = state;
  // While an agent task runs, the send button stays clickable as a Stop
  // button - a minutes-long run with no way out reads as a hang.
  btnAsk.disabled = state && !agentRunning;
  const isStop = state && agentRunning;
  btnAsk.innerHTML = isStop ? STOP_ICON : SEND_ICON;
  btnAsk.classList.toggle("is-stop", isStop);
  btnAsk.title = isStop ? "Stop the agent" : "Ask / do it (Enter)";
  btnAnalyze.disabled = state;
  // The question is already echoed above (questionHeaderHtml) once a task
  // starts - leaving the same text sitting in the input for the whole
  // "Working…" duration just duplicates it on screen. Lock the field instead
  // so it visibly reads as busy rather than as an editable, stale question.
  questionInput.readOnly = state;
  questionInput.placeholder = state ? (agentRunning ? "Working… (⏹ to stop)" : "Working…") : placeholderFor();
}

// ── Helpers ────────────────────────────────────────────────
// Window titles from UI Automation are sometimes internal class names
// ("Chrome Legacy Window", "Intermediate D3D Window") rather than anything a
// person would recognize - those are dropped instead of shown in the pill.
const JUNK_TITLE = /^(chrome legacy window|chrome_widgetwin_\d+|chrome_renderwidgethosthwnd|intermediate d3d window|default ime|msctfime ui|desktop|program manager)$/i;

function prettyApp(name) {
  const app = (name || "").replace(/\.exe$/i, "").trim();
  return app ? app.charAt(0).toUpperCase() + app.slice(1) : "";
}

function appSummary(p) {
  const app = prettyApp(p?.context?.process_name);
  let title = (p?.context?.window_title || "").trim();
  if (JUNK_TITLE.test(title)) title = "";
  // Browsers/editors append " - Google Chrome" etc.; the app name already says that.
  if (app && title.toLowerCase().endsWith(app.toLowerCase())) title = title.slice(0, -app.length).replace(/[\s|–—-]+$/, "");
  if (app && title) return `${app} · ${title}`;
  return app || title;
}

function elementSummary(p) {
  const el = p?.element;
  if (!el) return "";
  const parts = [];
  // Generic container roles ("pane", "document", "group") say nothing on
  // their own - only keep the role when it adds meaning ("button", "link").
  if (el.role && !/^(pane|document|group|window|custom|client)$/i.test(el.role)) parts.push(el.role);
  if (el.name && !JUNK_TITLE.test(el.name)) parts.push(el.name);
  if (el.value && el.value !== el.name) parts.push(el.value);
  return parts.join(" · ");
}

// Fuller version sent to the AI (not shown in the compact pill/panel label):
// includes the approximate container so the model knows this is, say, a row
// inside a file list rather than an isolated bit of text.
function elementContextForAI(p) {
  const summary = elementSummary(p);
  const container = p?.element?.container;
  if (container && !summary.includes(container)) {
    return summary ? `${summary} (in: ${container})` : `(in: ${container})`;
  }
  return summary;
}

function truncate(s, n) {
  if (!s) return "";
  return s.length > n ? s.slice(0, n - 1) + "…" : s;
}

function escapeHtml(text) {
  const div = document.createElement("div");
  div.textContent = text;
  return div.innerHTML;
}

// Line-based markdown renderer (the overlay is plain JS, not React, so it
// can't share the main window's react-markdown). The old regex chain left
// "####" headings, "---" rules, numbered lists and tables as raw text.
// Everything is escaped first; only the markup below is generated.
function renderInlineRich(text) {
  const codes = [];
  let html = escapeHtml(text).replace(/`([^`]+)`/g, (_, c) => {
    codes.push(c);
    return `\u0000${codes.length - 1}\u0000`;
  });
  html = html
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/__(.+?)__/g, "<strong>$1</strong>")
    .replace(/(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])/g, "<em>$1</em>")
    .replace(/~~(.+?)~~/g, "<del>$1</del>")
    .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
    .replace(/(^|[\s(])(https?:\/\/[^\s<)]+)/g, '$1<a href="$2" target="_blank" rel="noopener">$2</a>');
  return html.replace(/\u0000(\d+)\u0000/g, (_, i) => `<code>${codes[Number(i)]}</code>`);
}

function renderMarkdown(text) {
  if (!text) return "";
  const lines = String(text).replace(/\r\n?/g, "\n").split("\n");
  const out = [];
  let para = [];
  const flushPara = () => {
    if (para.length) out.push(`<p>${para.map(renderInlineRich).join("<br>")}</p>`);
    para = [];
  };
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const trimmed = line.trim();

    // Fenced code block
    const fence = /^```\s*([\w+-]*)/.exec(trimmed);
    if (fence) {
      flushPara();
      const lang = fence[1] || "";
      const body = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i].trim())) body.push(lines[i++]);
      i++;
      out.push(
        `<div class="md-code"><div class="md-code-head"><span>${escapeHtml(lang || "code")}</span>` +
          `<button class="md-copy" data-act="copy-code" type="button">Copy</button></div>` +
          `<pre><code>${escapeHtml(body.join("\n"))}</code></pre></div>`,
      );
      continue;
    }
    if (!trimmed) {
      flushPara();
      i++;
      continue;
    }
    // Horizontal rule
    if (/^(-{3,}|\*{3,}|_{3,})$/.test(trimmed)) {
      flushPara();
      out.push("<hr>");
      i++;
      continue;
    }
    // Heading (#..######)
    const h = /^(#{1,6})\s+(.+?)\s*#*$/.exec(trimmed);
    if (h) {
      flushPara();
      const level = Math.min(h[1].length + 1, 5); // # → h2 … keeps sizes calm in a small panel
      out.push(`<h${level}>${renderInlineRich(h[2])}</h${level}>`);
      i++;
      continue;
    }
    // Table: header row, then a |---| separator row
    if (trimmed.startsWith("|") && i + 1 < lines.length && /^\|?\s*:?-{2,}/.test(lines[i + 1].trim())) {
      flushPara();
      const cells = (row) => row.trim().replace(/^\||\|$/g, "").split("|").map((c) => renderInlineRich(c.trim()));
      const head = cells(lines[i]);
      i += 2;
      const rows = [];
      while (i < lines.length && lines[i].trim().startsWith("|")) rows.push(cells(lines[i++]));
      out.push(
        `<div class="md-table"><table><thead><tr>${head.map((c) => `<th>${c}</th>`).join("")}</tr></thead><tbody>` +
          rows.map((r) => `<tr>${r.map((c) => `<td>${c}</td>`).join("")}</tr>`).join("") +
          "</tbody></table></div>",
      );
      continue;
    }
    // Blockquote
    if (trimmed.startsWith(">")) {
      flushPara();
      const quote = [];
      while (i < lines.length && lines[i].trim().startsWith(">")) quote.push(lines[i++].trim().replace(/^>\s?/, ""));
      out.push(`<blockquote>${renderMarkdown(quote.join("\n"))}</blockquote>`);
      continue;
    }
    // Lists: ordered/unordered, one level of nesting by indent. A blank line between
    // items does NOT end the list (models write "1. …\n\n2. …" constantly - that used to
    // produce three separate lists, every one numbered "1."), and an item's own number
    // is kept (<li value>), so "3." stays "3." even after a paragraph in between.
    const LI = /^(\s*)([-*+]|(\d+)[.)])\s+(.*)$/;
    const li = LI.exec(line);
    if (li) {
      flushPara();
      const ordered = li[3] !== undefined;
      const items = [];
      while (i < lines.length) {
        const m = LI.exec(lines[i]);
        if (m) {
          items.push({ indent: m[1].length, ordered: m[3] !== undefined, num: m[3] ? Number(m[3]) : null, text: m[4], extra: [] });
          i++;
        } else if (lines[i].trim() && /^\s{2,}/.test(lines[i]) && items.length) {
          // Indented continuation (a wrapped line or a "Path: …" detail line): part of
          // the item, on its own line.
          items[items.length - 1].extra.push(lines[i].trim());
          i++;
        } else if (!lines[i].trim() && items.length) {
          // Blank line: keep going only if the next non-blank line is still a list item.
          let j = i + 1;
          while (j < lines.length && !lines[j].trim()) j++;
          if (j < lines.length && LI.test(lines[j])) { i = j; continue; }
          break;
        } else break;
      }
      const base = items[0].indent;
      const itemHtml = (it) => renderInlineRich(it.text) + it.extra.map((l) => "<br>" + renderInlineRich(l)).join("");
      const openTag = (o) => (o ? "<ol>" : "<ul>");
      const closeTag = (o) => (o ? "</ol>" : "</ul>");
      let html = openTag(ordered);
      let nested = null; // ordered flag of the open nested list, or null
      let openLi = false;
      for (const it of items) {
        const isNested = it.indent > base + 1;
        if (isNested && nested === null) {
          nested = it.ordered;
          html += openTag(nested);
        } else if (!isNested && nested !== null) {
          html += closeTag(nested) + "</li>";
          nested = null;
          openLi = false;
        }
        if (isNested) {
          html += `<li${it.ordered && it.num ? ` value="${it.num}"` : ""}>${itemHtml(it)}</li>`;
        } else {
          if (openLi) html += "</li>";
          html += `<li${it.ordered && it.num ? ` value="${it.num}"` : ""}>${itemHtml(it)}`;
          openLi = true;
        }
      }
      if (nested !== null) html += closeTag(nested);
      if (openLi) html += "</li>";
      out.push(html + closeTag(ordered));
      continue;
    }
    para.push(trimmed);
    i++;
  }
  flushPara();
  return out.join("");
}

// The main window's active project (same origin, same localStorage - the copilot bar
// reads it the same way). Where "Save to project" goes.
function activeProject() {
  const p = localStorage.getItem("mouse-ai-active-project") || "";
  if (!p) return null;
  return { path: p, name: p.split(/[\\/]/).filter(Boolean).pop() || p };
}

/** Copy / save / open-in-app row under every finished answer. Ask AI is the collector:
 *  what it gathered and you corrected can go to the project's notes (the agent reads
 *  them) or into the meeting copilot's context. */
function answerActionsHtml() {
  const proj = activeProject();
  const save = proj
    ? `<button type="button" data-act="save-project" title="Append to ${escapeHtml(proj.path)}\\.mouse-ai\\notes.md - the agent reads it on every task in this project"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16"/><path d="M4 4h10l6 6v10"/><path d="M14 4v6h6"/></svg><span>Save to ${escapeHtml(proj.name)}</span></button>`
    : "";
  return `<div class="answer-actions">
    <button type="button" data-act="copy-answer"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg><span>Copy</span></button>
    ${save}
    <button type="button" data-act="save-meeting" title="Add to the meeting copilot's context (This session)"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 10l4.5-2.5v9L15 14"/><rect x="3" y="6" width="12" height="12" rx="2"/></svg><span>To meeting</span></button>
    <button type="button" data-act="open-app"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/></svg><span>Continue in Mouse AI</span></button>
  </div>`;
}

/** The answer text and the question it answered, for saving. */
function answerPayload(btn) {
  const turn = btn.closest(".turn");
  const body = btn.closest(".agent-final-answer, .turn-body, #answer")?.querySelector(".answer-body");
  const question = turn?.querySelector(".asked-question")?.textContent?.trim() || "";
  const app = (hover && appSummary(hover)) || "";
  return { question, text: body?.innerText?.trim() || "", app };
}

// One delegated handler for answer actions and code-block copy buttons in
// both result areas.
function onAnswerAction(e) {
  const btn = e.target.closest("[data-act]");
  if (!btn) return;
  const act = btn.dataset.act;
  const flash = (label) => {
    const span = btn.querySelector("span") || btn;
    const old = span.textContent;
    span.textContent = label;
    setTimeout(() => (span.textContent = old), 1300);
  };
  if (act === "copy-code") {
    const code = btn.closest(".md-code")?.querySelector("code")?.textContent || "";
    copyText(code).then((ok) => flash(ok ? "Copied" : "Copy failed"));
  } else if (act === "copy-answer") {
    const body = btn.closest(".agent-final-answer, .turn-body, #answer")?.querySelector(".answer-body");
    copyText(body?.innerText || "").then((ok) => flash(ok ? "Copied" : "Copy failed"));
  } else if (act === "save-project") {
    const proj = activeProject();
    const { question, text, app } = answerPayload(btn);
    if (!proj || !text) { flash("Nothing to save"); return; }
    const title = (question || `Gathered from ${app || "the screen"}`).slice(0, 80);
    invoke("project_notes_append", { project: proj.path, title, text: (app ? `Source: ${app}\n\n` : "") + text })
      .then(() => flash("Saved"))
      .catch((e) => flash(String(e?.message || e).slice(0, 40)));
  } else if (act === "save-meeting") {
    const { question, text, app } = answerPayload(btn);
    if (!text) { flash("Nothing to add"); return; }
    const head = question ? `${question}${app ? ` (from ${app})` : ""}` : app ? `From ${app}` : "";
    invoke("meeting_context_append", { field: "session_context", text: head ? `${head}:\n${text}` : text })
      .then(() => flash("Added"))
      .catch((e) => flash(String(e?.message || e).slice(0, 40)));
  } else if (act === "open-app") {
    openInMain();
  }
}
answerEl.addEventListener("click", onAnswerAction);
agentLogEl.addEventListener("click", onAnswerAction);

/** Tall enough for a real answer: up to ~75% of the screen. */
function answerHeight() {
  return Math.round(Math.min(760, Math.max(520, (window.screen?.availHeight || 900) * 0.75)));
}

// Same escaping as renderMarkdown but no block-level wrapping (no <p>/<br>) -
// used for the compact single-line agent-step narration, where a full
// markdown block layout would look broken in an 11px-tall row.
function renderInlineMarkdown(text) {
  if (!text) return "";
  let html = escapeHtml(text);
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");
  html = html.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, "<em>$1</em>");
  return html;
}

// ── Quick actions ──────────────────────────────────────────
// Ask AI as a collector: one tap gathers what is in front of you (the window under
// the cursor is attached) and shapes it; the answer can then be corrected and saved.
const quickRow = document.getElementById("quick-row");
const QUICK = [
  { label: "Summarize this", prompt: "Summarize what is in this window in a few clear bullet points. Keep every number, name and date exactly as shown." },
  { label: "Key facts", prompt: "Extract the key facts from this window as a clean list: names, numbers, dates, decisions, links. Group related facts. Do not interpret, just organize." },
  { label: "Make a checklist", prompt: "Turn what is in this window into an actionable checklist, one task per line, in a sensible order." },
  { label: "What is this?", prompt: "Explain what this is and what I can do with it, in plain language." },
];

function renderQuickRow() {
  if (!quickRow) return;
  const show = threadTurns === 0 && !busy && expanded;
  quickRow.hidden = !show;
  if (!show) return;
  quickRow.innerHTML = "";
  for (const q of QUICK) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "quick-chip";
    b.textContent = q.label;
    b.addEventListener("click", () => {
      addChip({ kind: "window", label: "Window" });
      questionInput.value = q.prompt;
      submit();
    });
    quickRow.appendChild(b);
  }
}

// ── @-mentions ─────────────────────────────────────────────
// Type "@" (or press the @ button) to attach context the way you would in an
// editor: the screen, the window under the cursor, the clipboard, a file, or
// "search the web". Chips sit above the input; they resolve when you send, so
// "@screen" is the screen as it is at that moment.
const btnMention = document.getElementById("btn-mention");
const chipsEl = document.getElementById("chips");
const mentionMenu = document.getElementById("mention-menu");
const chips = []; // { kind, label, path?, resolved? }
let mentionIndex = 0;
let mentionFilter = "";

const MENTIONS = [
  { kind: "screen", label: "Screen", hint: "Screenshot of the whole screen", icon: "📷" },
  { kind: "window", label: "Window", hint: "The window under your cursor - its text and a screenshot", icon: "🪟" },
  { kind: "clipboard", label: "Clipboard", hint: "What you last copied", icon: "📋" },
  { kind: "file", label: "File…", hint: "A file from your PC (text, code or an image)", icon: "📄" },
  { kind: "web", label: "Web", hint: "Search the web for this", icon: "🌐" },
];

function chipIcon(kind) {
  if (kind === "link") return "🔗";
  return MENTIONS.find((m) => m.kind === kind)?.icon || "•";
}

function renderChips() {
  chipsEl.innerHTML = "";
  chipsEl.hidden = chips.length === 0;
  chips.forEach((c, idx) => {
    const el = document.createElement("span");
    el.className = `chip chip-${c.kind}`;
    el.innerHTML = `<span class="chip-ic">${escapeHtml(chipIcon(c.kind))}</span><span class="chip-label">${escapeHtml(c.label)}</span><button type="button" class="chip-x" title="Remove" aria-label="Remove">×</button>`;
    el.querySelector(".chip-x").addEventListener("click", () => removeChip(idx));
    chipsEl.appendChild(el);
  });
  syncOverlaySize(480, { growOnly: true });
}

function addChip(chip) {
  if (chip.kind !== "file" && chips.some((c) => c.kind === chip.kind)) return;
  chips.push(chip);
  renderChips();
}

function removeChip(idx) {
  chips.splice(idx, 1);
  renderChips();
  questionInput.focus();
}

function clearChips() {
  chips.length = 0;
  renderChips();
}

function visibleMentions() {
  const f = mentionFilter.toLowerCase();
  return MENTIONS.filter((m) => !f || m.kind.startsWith(f) || m.label.toLowerCase().startsWith(f));
}

function openMentionMenu(filter) {
  mentionFilter = filter || "";
  const items = visibleMentions();
  if (!items.length) { closeMentionMenu(); return; }
  mentionIndex = Math.min(mentionIndex, items.length - 1);
  mentionMenu.innerHTML = items
    .map((m, i) => `<button type="button" class="mention-item${i === mentionIndex ? " is-active" : ""}" data-kind="${m.kind}"><span class="mention-ic">${m.icon}</span><span class="mention-text"><span class="mention-label">@${escapeHtml(m.kind)}</span><span class="mention-hint">${escapeHtml(m.hint)}</span></span></button>`)
    .join("");
  mentionMenu.querySelectorAll(".mention-item").forEach((b) => {
    b.addEventListener("mousedown", (e) => e.preventDefault()); // keep the input focused
    b.addEventListener("click", () => pickMention(b.dataset.kind));
  });
  mentionMenu.hidden = false;
  syncOverlaySize(480, { growOnly: true });
}

function closeMentionMenu() {
  if (!mentionMenu) return;
  mentionMenu.hidden = true;
  mentionMenu.innerHTML = "";
  mentionIndex = 0;
  mentionFilter = "";
}

function handleMentionKey(e) {
  const items = visibleMentions();
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    mentionIndex = (mentionIndex + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length;
    openMentionMenu(mentionFilter);
    return true;
  }
  if (e.key === "Enter" || e.key === "Tab") {
    e.preventDefault();
    pickMention(items[mentionIndex]?.kind);
    return true;
  }
  return false;
}

async function pickMention(kind) {
  if (!kind) return;
  // Drop the "@partial" token the user was typing.
  const caret = questionInput.selectionStart ?? questionInput.value.length;
  const before = questionInput.value.slice(0, caret).replace(/(^|\s)@[\w.-]*$/, "$1");
  questionInput.value = before + questionInput.value.slice(caret);
  closeMentionMenu();
  questionInput.focus();
  if (kind === "file") {
    try {
      const open = window.__TAURI__?.dialog?.open;
      if (!open) throw new Error("File picker unavailable");
      const picked = await open({ multiple: false, title: "Attach a file" });
      const path = Array.isArray(picked) ? picked[0] : picked;
      if (!path) return;
      const name = String(path).split(/[\\/]/).pop();
      const chip = { kind: "file", label: name, path: String(path) };
      // Read it now so a bad pick (binary, too large) is reported immediately.
      chip.resolved = await invoke("resolve_mention", { kind: "file", path: chip.path, targetHwnd: 0 });
      addChip(chip);
    } catch (err) {
      questionInput.placeholder = friendlyError(err);
      setTimeout(() => (questionInput.placeholder = placeholderFor()), 2500);
    }
    return;
  }
  const m = MENTIONS.find((x) => x.kind === kind);
  addChip({ kind, label: m?.label || kind });
}
