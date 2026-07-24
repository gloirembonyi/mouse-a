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

const PILL_AUTOHIDE_MS = 4500;
const TOAST_AUTOHIDE_MS = 2200;

let hover = null; // latest hover-dwell payload from Rust
let expanded = false;
let busy = false;
let autoHideTimer = null;
let agentMode = false; // when on, the ask runs as a full agent task on the PC

// ── Events from Rust ───────────────────────────────────────
window.addEventListener("DOMContentLoaded", async () => {
  await listen("hover-dwell", (event) => onHoverDwell(event.payload));
  await listen("trigger-ask", (event) => onTriggerAsk(event.payload));
  await listen("ai-toast", (event) => onAiToast(event.payload));
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

// ── UI wiring ──────────────────────────────────────────────
function setupUI() {
  pill.addEventListener("click", expand);
  btnClose.addEventListener("click", collapse);
  btnOpenApp.addEventListener("click", () => invoke("show_main_window").catch(() => {}));
  btnAsk.addEventListener("click", () => ask(questionInput.value.trim() || null));
  btnAnalyze.addEventListener("click", () => ask(null));
  btnAgent.addEventListener("click", toggleAgentMode);

  questionInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      ask(questionInput.value.trim() || null);
    }
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") collapse();
  });

  // Clicking away on the desktop dismisses the modal (unless an answer is
  // still being fetched — don't lose the user's question).
  window.addEventListener("blur", () => {
    if (expanded && !busy) collapse();
  });
}

async function expand() {
  showPanel({ resize: true });
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
    try {
      await invoke("expand_overlay", { width: 440, height: 380 });
    } catch { /* ignore */ }
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

async function ask(question) {
  if (!hover || busy) return;
  if (agentMode) return runAgentTask(question);

  setBusy(true);
  thinkingEl.hidden = false;
  answerEl.hidden = true;

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
    answerEl.innerHTML = renderMarkdown(result.text || "(no answer)");
    // Grow the modal so the answer is comfortably readable.
    try {
      await invoke("expand_overlay", { width: 460, height: 460 });
    } catch { /* ignore */ }
  } catch (err) {
    answerEl.innerHTML = `<div class="error-box">⚠️ ${escapeHtml(friendlyError(err))}</div>`;
  } finally {
    thinkingEl.hidden = true;
    answerEl.hidden = false;
    setBusy(false);
    questionInput.value = "";
    questionInput.focus();
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
  setBusy(true);
  thinkingEl.hidden = true;
  answerEl.hidden = true;
  agentLogEl.hidden = false;
  agentLogEl.innerHTML = `<div class="agent-line agent-thinking">💭 Starting…</div>`;
  try {
    await invoke("expand_overlay", { width: 460, height: 480 });
  } catch { /* ignore */ }

  try {
    const result = await invoke("agent_run", {
      task,
      x: hover.x,
      y: hover.y,
      source: "overlay",
      autonomous: true,
    });
    agentLogEl.hidden = true;
    answerEl.hidden = false;
    answerEl.innerHTML = renderMarkdown(result.text || "(done)");
  } catch (err) {
    // Hide the activity log (it may still hold a raw "⚠️ ..." line from the
    // agent-progress "error" event that fired just before this rejection) so
    // the user sees one clean, friendly error box instead of both.
    agentLogEl.hidden = true;
    answerEl.hidden = false;
    answerEl.innerHTML = `<div class="error-box">⚠️ ${escapeHtml(friendlyError(err))}</div>`;
  } finally {
    setBusy(false);
    questionInput.value = "";
    questionInput.focus();
  }
}

function renderAgentProgress(p) {
  if (agentLogEl.hidden) return;
  if (p.kind === "done") return;

  if (p.kind === "confirm") {
    agentLogEl.appendChild(buildConfirmPrompt(p));
    agentLogEl.scrollTop = agentLogEl.scrollHeight;
    return;
  }

  const line = document.createElement("div");
  line.className = `agent-line agent-${p.kind}`;
  const icon = { thinking: "💭", tool: "⚙️", "tool-result": "↳", error: "⚠️" }[p.kind] || "•";
  line.textContent = `${icon} ${p.message}`;
  agentLogEl.appendChild(line);
  agentLogEl.scrollTop = agentLogEl.scrollHeight;
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
  if (/429|rate limit/i.test(msg)) {
    return "Too many requests right now — wait a few seconds and try again.";
  }
  return msg;
}

function setBusy(state) {
  busy = state;
  btnAsk.disabled = state;
  btnAnalyze.disabled = state;
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
