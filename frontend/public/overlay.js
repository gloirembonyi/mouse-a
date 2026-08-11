// Mouse AI Desktop — Hover Overlay Logic
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
const questionInput = document.getElementById("question");
const btnAsk = document.getElementById("btn-ask");
const btnAnalyze = document.getElementById("btn-analyze");
const btnClose = document.getElementById("btn-close");
const btnOpenApp = document.getElementById("btn-open-app");
const btnAgent = document.getElementById("btn-agent");
const agentLogEl = document.getElementById("agent-log");
const toast = document.getElementById("toast");
const resizeGrip = document.getElementById("resize-grip");

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
let agentMode = true; // when on, the ask runs as a full agent task on the PC
let agentSummary = null; // live "Thought for Xs" summary widget for the in-flight run
let agentRunning = false; // an agent_run is in flight — the send button doubles as Stop

// ── Events from Rust ───────────────────────────────────────
window.addEventListener("DOMContentLoaded", async () => {
  await listen("hover-dwell", (event) => onHoverDwell(event.payload));
  await listen("trigger-ask", (event) => onTriggerAsk(event.payload));
  await listen("ai-toast", (event) => onAiToast(event.payload));
  await listen("surface-theme", (event) => applySurfaceTheme(!!event.payload));
  await listen("agent-progress", (event) => {
    const p = event.payload || {};
    if (p.source !== "overlay") return;
    renderAgentProgress(p);
  });
  setupUI();
});

// Hotkey / tray "Ask at Cursor": Rust already positioned, sized, and focused
// the window — jump straight to the input panel.
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
    ? "🖱️✨ Mouse AI active — pause your mouse on anything to ask"
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
// caption-drag loop takes over the mouse) — the exact "I can't click it
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
      // Tail end of a drag gesture, not a real click — swallow it.
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
  btnClose.addEventListener("click", collapse);
  btnOpenApp.addEventListener("click", () => invoke("show_main_window").catch(() => {}));
  btnAsk.addEventListener("click", () => {
    if (agentRunning) {
      // The send button is a Stop button while an agent task runs.
      btnAsk.disabled = true;
      questionInput.placeholder = "Stopping…";
      invoke("agent_cancel", { source: "overlay" }).catch(() => {});
      return;
    }
    ask(questionInput.value.trim() || null);
  });
  btnAnalyze.addEventListener("click", () => ask(null));
  btnAgent.addEventListener("click", toggleAgentMode);
  // Reflect the default agent-mode state in the button + placeholder at startup.
  btnAgent.classList.toggle("active", agentMode);
  questionInput.placeholder = agentMode
    ? "Tell Mouse AI what to do…"
    : "Ask about what's under your mouse…";

  questionInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      ask(questionInput.value.trim() || null);
    }
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") collapse();
  });

  // Manual resize handle — the window is also natively resizable from its
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

  // Clicking elsewhere on the desktop used to auto-collapse the panel — lost
  // in-progress research/answers the moment the user glanced at another
  // window. The panel now stays open regardless of focus; ✕ / Esc / the
  // pill's own collapse() are the only ways to dismiss it. Alt+Tab-ing back
  // to it just works since the window was never hidden.
}

async function expand() {
  showPanel({ resize: true });
}

// The window used to be resized to hardcoded pixel constants (380/460/480)
// regardless of how much text actually came back — a one-line answer left a
// large blank gap below it up to that fixed height. Instead, measure how
// tall the panel's content actually wants to be (by briefly letting it hug
// its content instead of stretching to fill the window) and resize the
// native window to match, capped so a very long answer scrolls instead of
// growing the window off-screen.
//
// Never shrinks below MIN_OVERLAY_HEIGHT even when content is just the
// header + input row — a popup that shrinks to barely more than its own
// title bar (as it briefly did while "Working…" with nothing logged yet)
// reads as broken, not efficient, so there's always a comfortable default
// amount of room regardless of how little is on screen yet.
const MIN_OVERLAY_HEIGHT = 360;

async function syncOverlaySize(maxHeight = 560) {
  if (!panel || panel.hidden) return;
  const prevHeight = panel.style.height;
  panel.style.height = "auto";
  const natural = panel.scrollHeight;
  panel.style.height = prevHeight;
  const height = Math.min(Math.max(maxHeight, MIN_OVERLAY_HEIGHT), Math.max(MIN_OVERLAY_HEIGHT, natural));
  try {
    await invoke("expand_overlay", { width: 460, height });
  } catch { /* ignore */ }
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
  agentLogEl.hidden = true;
  agentLogEl.innerHTML = "";
  thinkingEl.hidden = true;
  questionInput.value = "";

  if (resize) {
    await syncOverlaySize(380);
  }
  questionInput.focus();
}

async function collapse() {
  expanded = false;
  busy = false;
  panel.hidden = true;
  pill.hidden = true;
  toast.hidden = true;
  answerEl.innerHTML = "";
  agentLogEl.hidden = true;
  agentLogEl.innerHTML = "";
  questionInput.value = "";
  setBusy(false);
  try {
    await invoke("collapse_overlay");
  } catch { /* ignore */ }
}

function toggleAgentMode() {
  agentMode = !agentMode;
  btnAgent.classList.toggle("active", agentMode);
  questionInput.placeholder = agentMode
    ? "Tell Mouse AI what to do…"
    : "Ask about what's under your mouse…";
}

// The panel only ever showed the ANSWER, never what was actually asked — easy
// to lose track of, especially once the panel's grown to fit a long reply.
// Echoed above the answer/log for both the plain-ask and agent-task paths.
function questionHeaderHtml(question) {
  if (!question) return "";
  return `<div class="asked-question">${escapeHtml(question)}</div>`;
}

// "Worked for 4m 28s" — same run-duration line shown under a finished agent
// task in the main window (main.js's finishAgentActivity/formatDuration).
function durationFooterHtml(elapsedMs) {
  return `<div class="agent-activity-footer">Worked for ${formatDuration(elapsedMs)}</div>`;
}

function formatDuration(ms) {
  const totalSeconds = Math.max(1, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes > 0 ? `${minutes}m ${seconds}s` : `${seconds}s`;
}

async function ask(question) {
  if (!hover || busy) return;
  if (agentMode) return runAgentTask(question);

  setBusy(true);
  questionInput.value = "";
  thinkingEl.hidden = false;
  // Fully clear both panes (not just toggle `hidden`) — a stale previous
  // answer's innerHTML sitting under a newly-visible agent-log (or vice
  // versa) is exactly how two unrelated Q&A turns end up visually stacked.
  answerEl.hidden = true;
  answerEl.innerHTML = "";
  agentLogEl.hidden = true;
  agentLogEl.innerHTML = "";

  try {
    // Watchdog: never let the panel spin forever, whatever happens below.
    const result = await withTimeout(
      invoke("ask_about_hover", {
        question,
        x: hover.x,
        y: hover.y,
        appName: hover.context?.process_name || null,
        windowTitle: hover.context?.window_title || null,
        elementText: elementContextForAI(hover) || null,
      }),
      55000
    );
    answerEl.innerHTML = questionHeaderHtml(question) + renderMarkdown(result.text || "(no answer)");
  } catch (err) {
    answerEl.innerHTML = questionHeaderHtml(question) + `<div class="error-box">⚠️ ${escapeHtml(friendlyError(err))}</div>`;
  } finally {
    thinkingEl.hidden = true;
    answerEl.hidden = false;
    setBusy(false);
    questionInput.value = "";
    questionInput.focus();
    // Grow (or shrink) the window to fit whatever came back — answer or error.
    await syncOverlaySize(460);
  }
}

// Full agent task, reporting live activity into the overlay and the final
// answer in the same panel the user asked from.
async function runAgentTask(task) {
  if (!task) {
    answerEl.hidden = false;
    answerEl.innerHTML = `<div class="error-box">Type what you'd like me to do first.</div>`;
    return;
  }
  agentRunning = true;
  setBusy(true);
  questionInput.value = "";
  thinkingEl.hidden = true;
  // Fully clear both panes — see the matching comment in ask() above.
  answerEl.hidden = true;
  answerEl.innerHTML = "";
  agentLogEl.hidden = false;
  agentLogEl.innerHTML = questionHeaderHtml(task);
  agentSummary = createAgentSummary();
  agentLogEl.appendChild(agentSummary.root);
  agentLogEl.scrollTop = 0;
  await syncOverlaySize(480);

  try {
    const result = await invoke("agent_run", {
      task,
      x: hover.x,
      y: hover.y,
      source: "overlay",
      autonomous: true,
    });
    finishAgentSummary(true);
    // The final answer joins the SAME scrollable log the collapsed "Thought
    // for Xs" recap lives in (Cursor-style) — it doesn't replace it. Only the
    // plain (non-agent) ask() path uses the separate #answer element.
    appendFinalAnswer(renderMarkdown(result.text || "(done)"));
  } catch (err) {
    finishAgentSummary(false);
    appendFinalAnswer(`<div class="error-box">⚠️ ${escapeHtml(friendlyError(err))}</div>`);
  } finally {
    agentRunning = false;
    setBusy(false);
    questionInput.value = "";
    questionInput.focus();
    // The "Thought for Xs" summary just collapsed to one line — shrink the
    // window to match instead of leaving the old expanded-log space behind.
    await syncOverlaySize(480);
  }
}

// Cursor/Codex-style collapsible "Thought for Xs" widget — the raw step-by-
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
    <span class="agent-summary-label">Working</span>
    <span class="agent-summary-duration"></span>
    <span class="agent-summary-chevron">▾</span>`;

  const body = document.createElement("div");
  body.className = "agent-summary-body";

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
    syncOverlaySize(480);
  });

  state.tickInterval = setInterval(() => {
    state.duration.textContent = `for ${formatDuration(Date.now() - state.startedAt)}`;
  }, 1000);

  return state;
}

function finishAgentSummary(success) {
  if (!agentSummary) return;
  clearInterval(agentSummary.tickInterval);
  agentSummary.running = false;
  agentSummary.label.textContent = success ? "Thought" : "Failed";
  agentSummary.duration.textContent = `for ${formatDuration(Date.now() - agentSummary.startedAt)}`;
  agentSummary.icon.innerHTML = success ? "✓" : "✕";
  agentSummary.icon.classList.toggle("failed", !success);
  if (agentSummary.userCollapsed === null) agentSummary.applyExpanded();
  agentSummary = null;
}

// Appends the final answer (or error box) below the now-collapsed "Thought
// for Xs" summary, inside the same scrollable log — then scrolls down so the
// answer is what's actually in view, not left scrolled up at the old step
// log's position.
function appendFinalAnswer(html) {
  const wrap = document.createElement("div");
  wrap.className = "agent-final-answer";
  wrap.innerHTML = html;
  agentLogEl.appendChild(wrap);
  requestAnimationFrame(() => {
    agentLogEl.scrollTop = agentLogEl.scrollHeight;
  });
}

function renderAgentProgress(p) {
  if (agentLogEl.hidden || !agentSummary) return;
  if (p.kind === "done") return;

  if (p.kind === "confirm") {
    agentSummary.hasPendingConfirm = true;
    agentSummary.applyExpanded();
    agentSummary.body.appendChild(buildConfirmPrompt(p));
    agentLogEl.scrollTop = agentLogEl.scrollHeight;
    syncOverlaySize(480);
    return;
  }

  agentSummary.body.appendChild(p.kind === "phase-summary" ? buildPhaseSummary(p) : buildAgentStep(p));
  agentLogEl.scrollTop = agentLogEl.scrollHeight;
  syncOverlaySize(480);
}

// A rollup ("Explored 12 files, 3 searches +367 -14") isn't a timeline entry
// the way a tool call is — no icon/rail, just a small muted recap line under
// whatever heading it summarizes (same as Cursor's grey rollup lines).
function buildPhaseSummary(p) {
  const div = document.createElement("div");
  div.className = "agent-step kind-phase-summary";
  div.textContent = p.message;
  return div;
}

// Icon badge + connecting rail beside the message — same timeline shape as
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
  // Same friendly formatting the final error box uses — the log is normally
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

// Real Allow/Deny prompt the agent is waiting on — the click is reported
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
    try {
      await invoke("agent_confirm", { requestId: p.request_id, approved });
    } catch { /* ignore */ }
    msg.textContent = approved ? `✅ Allowed: ${p.message}` : `🚫 Denied: ${p.message}`;
    row.remove();
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
      setTimeout(() => reject(new Error("Request timed out — the backend didn't respond.")), ms)
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
    return "The model didn't respond after several tries — it may be under heavy load. Try again in a moment.";
  }
  if (/step limit/i.test(msg)) {
    return "That task took more steps than I could finish in one go. Try breaking it into smaller steps.";
  }
  if (/daily message limit|daily limit/i.test(msg)) {
    return "You've hit today's free message limit. It resets daily — try again tomorrow.";
  }
  if (/referer|permission_denied|rejected every configured api key|api key not valid/i.test(msg)) {
    return "The backend's Gemini API key is misconfigured (not something you can fix here) — let the app owner know it needs a working key.";
  }
  // Distinct from a transient rate limit: a quota/billing error means the
  // account's Gemini quota is genuinely exhausted — retrying can't fix it.
  if (/exceeded your current quota|check your plan and billing|quota_exceeded|resource_exhausted/i.test(msg)) {
    return "The Gemini API quota/billing limit has been reached — this won't resolve by retrying. Check the plan/billing for the API key in use, or switch to a different key in Settings.";
  }
  if (/429|rate limit/i.test(msg)) {
    return "Too many requests right now — wait a few seconds and try again.";
  }
  return msg;
}

function setBusy(state) {
  busy = state;
  // While an agent task runs, the send button stays clickable as a Stop
  // button (⏹) — a minutes-long run with no way out reads as a hang.
  btnAsk.disabled = state && !agentRunning;
  btnAsk.textContent = state && agentRunning ? "⏹" : "➤";
  btnAsk.title = state && agentRunning ? "Stop the agent" : "Ask / do it (Enter)";
  btnAnalyze.disabled = state;
  // The question is already echoed above (questionHeaderHtml) once a task
  // starts — leaving the same text sitting in the input for the whole
  // "Working…" duration just duplicates it on screen. Lock the field instead
  // so it visibly reads as busy rather than as an editable, stale question.
  questionInput.readOnly = state;
  questionInput.placeholder = state
    ? agentRunning
      ? "Working… (⏹ to stop)"
      : "Working…"
    : agentMode
      ? "Tell Mouse AI what to do…"
      : "Ask about what's under your mouse…";
}

// ── Helpers ────────────────────────────────────────────────
function appSummary(p) {
  const app = (p?.context?.process_name || "").replace(/\.exe$/i, "");
  const title = p?.context?.window_title || "";
  if (app && title) return `${app} — ${title}`;
  return app || title;
}

function elementSummary(p) {
  const el = p?.element;
  if (!el) return "";
  const parts = [];
  if (el.role) parts.push(el.role);
  if (el.name) parts.push(el.name);
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

// Same lightweight markdown renderer as the main window.
function renderMarkdown(text) {
  if (!text) return "";
  let html = escapeHtml(text);

  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
    return `<pre><code class="language-${lang}">${code.trim()}</code></pre>`;
  });
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");
  html = html.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, "<em>$1</em>");
  html = html.replace(/^### (.+)$/gm, "<h3>$1</h3>");
  html = html.replace(/^## (.+)$/gm, "<h2>$1</h2>");
  html = html.replace(/^# (.+)$/gm, "<h1>$1</h1>");
  html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank">$1</a>');
  html = html.replace(/^&gt; (.+)$/gm, "<blockquote>$1</blockquote>");
  html = html.replace(/^[*\-] (.+)$/gm, "<li>$1</li>");
  html = html.replace(/(<li>.*<\/li>\n?)+/g, "<ul>$&</ul>");
  html = html.replace(/^\d+\. (.+)$/gm, "<li>$1</li>");
  html = html.replace(/\n\n/g, "</p><p>");
  html = html.replace(/\n/g, "<br>");
  if (!html.startsWith("<")) html = `<p>${html}</p>`;
  return html;
}

// Same escaping as renderMarkdown but no block-level wrapping (no <p>/<br>) —
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
