// Mouse AI Desktop — Main Application Logic
// Handles all UI interactions, Tauri IPC calls, and event handling.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ── State ──────────────────────────────────────────────
let aiActive = false;
let isLoading = false;
let messages = [];
let hoverTimer = null;
let contextPollingInterval = null;
let agentMode = true; // Agent can run commands / research / act on the PC to finish a task
let activeAgentActivityEl = null; // live activity list for the in-flight agent run

// ── DOM Elements ───────────────────────────────────────
const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => document.querySelectorAll(sel);

const authGate = $("#auth-gate");
const mainView = $("#main-view");
const settingsPanel = $("#settings-panel");
const loadingOverlay = $("#loading-overlay");
const messagesContainer = $("#messages-container");
const messagesEl = $("#messages");
const chatInput = $("#chat-input");
const sendBtn = $("#btn-send");
const indicator = $("#ai-indicator");
const statusBadge = $("#status-badge");
const contextBar = $("#context-bar");
const contextApp = $("#context-app");
const contextWindow = $("#context-window");

// ── Initialize ─────────────────────────────────────────
document.addEventListener("DOMContentLoaded", async () => {
  setupEventListeners();
  await setupTauriListeners();
  await checkAuth();
});

function setupEventListeners() {
  // Auth
  $("#btn-google-signin")?.addEventListener("click", handleGoogleSignIn);
  $("#btn-token-signin")?.addEventListener("click", handleTokenSignIn);
  $("#btn-retry-connect")?.addEventListener("click", checkAuth);

  // Chat
  sendBtn?.addEventListener("click", handleSend);
  chatInput?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  });
  chatInput?.addEventListener("input", autoResizeTextarea);

  // Header buttons
  $("#btn-new-chat")?.addEventListener("click", handleNewChat);
  $("#btn-settings")?.addEventListener("click", showSettings);
  $("#btn-close-settings")?.addEventListener("click", hideSettings);
  $("#btn-signout")?.addEventListener("click", handleSignOut);
  $("#btn-save-backend-url")?.addEventListener("click", handleSaveBackendUrl);
  $("#btn-test-backend")?.addEventListener("click", handleTestBackend);
  $("#btn-save-mcp")?.addEventListener("click", handleSaveMcp);

  // AI indicator click
  indicator?.addEventListener("click", handleToggleAI);

  // Agent mode toggle
  $("#btn-agent-mode")?.addEventListener("click", toggleAgentMode);

  // Context bar
  $("#btn-clear-context")?.addEventListener("click", clearContext);

  // Quick action chips
  $$(".action-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      const action = chip.dataset.action;
      if (action === "capture") handleCaptureAnalyze();
      else if (action === "ask") chatInput?.focus();
      else if (action === "search") {
        chatInput.value = "Search the web for: ";
        chatInput.focus();
      }
    });
  });

  // Input hints
  $("#capture-hint")?.addEventListener("click", handleCaptureAnalyze);
  $("#toggle-hint")?.addEventListener("click", handleToggleAI);
  $("#ask-cursor-hint")?.addEventListener("click", () => invoke("ask_at_cursor").catch(() => {}));

  // Escape to dismiss
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      if (settingsPanel && !settingsPanel.hidden) hideSettings();
    }
  });
}

async function setupTauriListeners() {
  // Rust owns the AI-active state (hotkey, tray, commands all flip it there)
  // and broadcasts changes to every window.
  await listen("ai-state", (event) => applyAIState(!!event.payload));
  await listen("capture-analyze", () => handleCaptureAnalyze());

  // Live agent activity — only the events tagged for the main window.
  await listen("agent-progress", (event) => {
    const p = event.payload || {};
    if (p.source && p.source !== "main") return;
    renderAgentProgress(p);
  });
  await listen("quick-ask", () => {
    chatInput?.focus();
    if (mainView.hidden) showMainView();
  });

  // Sync initial state (e.g. window reopened while AI already active)
  try {
    const active = await invoke("get_ai_active");
    applyAIState(!!active, { silent: true });
  } catch { /* not critical */ }
}

// ── Auth ────────────────────────────────────────────────
// No login wall: the app auto-provisions an anonymous per-install "guest"
// session against the backend so it's usable immediately. Google sign-in
// stays available (under "Advanced") for anyone who wants to link a real
// account later; it's optional, never required.
async function checkAuth() {
  const status = $("#connect-status");
  const errorEl = $("#auth-error");
  const retryBtn = $("#btn-retry-connect");
  const googleSection = $("#google-link-section");

  authGate.hidden = false;
  mainView.hidden = true;
  settingsPanel.hidden = true;
  if (status) { status.hidden = false; status.textContent = "Connecting…"; }
  if (errorEl) errorEl.hidden = true;
  if (retryBtn) retryBtn.hidden = true;

  try {
    const state = await withTimeout(invoke("ensure_session"), 20000);
    if (state.signed_in && state.user) {
      onSignedIn(state.user);
      // Best-effort background revalidation of a real (non-guest) session.
      invoke("verify_session").catch(() => {});
      return;
    }
    throw new Error("Couldn't start a session.");
  } catch (err) {
    if (status) status.hidden = true;
    showError(errorEl, `Can't reach the Thothai backend: ${err?.message || err}`, 30000);
    if (retryBtn) retryBtn.hidden = false;
    if (googleSection) googleSection.hidden = false;
  }
}

function showMainView() {
  authGate.hidden = true;
  mainView.hidden = false;
  settingsPanel.hidden = true;
}

function onSignedIn(user) {
  showMainView();
  updateUserInfo(user);
}

function updateUserInfo(user) {
  if (!user) return;
  const avatar = $("#user-avatar");
  const name = $("#user-name");
  const email = $("#user-email");

  if (avatar && user.picture) {
    avatar.src = user.picture;
    avatar.hidden = false;
  }
  if (name) name.textContent = user.name || user.email || "User";
  if (email) email.textContent = user.email || "";
}

async function handleGoogleSignIn() {
  const errorEl = $("#auth-error");
  const btn = $("#btn-google-signin");
  const statusEl = $("#auth-status");

  try {
    if (btn) btn.disabled = true;
    if (statusEl) {
      statusEl.textContent = "Waiting for you to finish signing in in your browser…";
      statusEl.hidden = false;
    }

    // Rust opens the browser and waits on a localhost listener for the
    // OAuth redirect; the whole flow can take a few minutes.
    const result = await withTimeout(invoke("start_google_signin"), 250000);

    if (result.signed_in && result.user) {
      onSignedIn(result.user);
    } else {
      showError(errorEl, "Sign in failed — please try again.");
    }
  } catch (err) {
    showError(errorEl, `${err?.message || err}`, 12000);
    // Offer the manual token path as a fallback.
    const tokenSection = $("#token-input-section");
    if (tokenSection) tokenSection.style.display = "flex";
  } finally {
    if (btn) btn.disabled = false;
    if (statusEl) statusEl.hidden = true;
  }
}

// Accepts either a raw Google access token or a full pasted redirect URL
// (e.g. https://…#access_token=ya29.…&token_type=…) and extracts the token.
function extractAccessToken(raw) {
  const text = (raw || "").trim();
  if (!text) return "";
  const match = text.match(/access_token=([^&\s]+)/);
  if (match) {
    try { return decodeURIComponent(match[1]); } catch { return match[1]; }
  }
  return text;
}

async function handleTokenSignIn() {
  const input = $("#access-token-input");
  const errorEl = $("#auth-error");
  const btn = $("#btn-token-signin");
  const token = extractAccessToken(input?.value);

  if (!token) {
    showError(errorEl, "Please enter your access token");
    return;
  }

  try {
    if (btn) {
      btn.disabled = true;
      btn.textContent = "Signing in…";
    }
    // Watchdog so the UI can never get stuck on a spinner.
    const result = await withTimeout(
      invoke("sign_in_with_token", { accessToken: token }),
      30000
    );

    if (result.signed_in && result.user) {
      onSignedIn(result.user);
    } else {
      showError(errorEl, "Sign in failed. Please check your token.");
    }
  } catch (err) {
    showError(errorEl, `Sign in error: ${err?.message || err}`);
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = "Sign In";
    }
  }
}

function withTimeout(promise, ms) {
  return Promise.race([
    promise,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error("Request timed out — backend didn't respond.")), ms)
    ),
  ]);
}

async function handleSignOut() {
  try {
    await invoke("sign_out");
    await invoke("set_ai_active", { active: false }).catch(() => {});
    messages = [];
    stopContextPolling();
    // Not a login wall — immediately fall back to an anonymous guest session
    // rather than stranding the user on a sign-in screen.
    await checkAuth();
  } catch (err) {
    console.error("Sign out error:", err);
  }
}

// ── AI Active Toggle ───────────────────────────────────
async function handleToggleAI() {
  try {
    // Rust flips the state and broadcasts "ai-state" — applyAIState updates the UI.
    await invoke("toggle_ai_active");
  } catch (err) {
    console.error("Toggle AI error:", err);
  }
}

function applyAIState(active, opts = {}) {
  if (aiActive === active && !opts.force) return;
  aiActive = active;

  indicator.classList.toggle("active", active);
  indicator.classList.toggle("inactive", !active);
  indicator.title = active ? "AI Active — Click to disable" : "AI Inactive — Click to enable";

  statusBadge.textContent = active ? "ACTIVE" : "OFF";
  statusBadge.classList.toggle("active", active);
  statusBadge.classList.toggle("inactive", !active);

  if (active) {
    startContextPolling();
    if (!opts.silent) {
      addSystemMessage("🖱️ Hover mode ON — pause your mouse on anything, anywhere, and click the ✦ Ask AI button that appears (Ctrl+Shift+Q to turn off)");
    }
  } else {
    stopContextPolling();
    clearContext();
    if (!opts.silent) addSystemMessage("Hover mode OFF - Ctrl+Shift+Space still asks at your cursor anytime");
  }
}

// Kept for compatibility with older call sites.
function setAIActive(active) {
  applyAIState(active, { force: true });
}

// ── Context Polling (when AI is active) ────────────────
function startContextPolling() {
  stopContextPolling();
  let lastContext = "";

  contextPollingInterval = setInterval(async () => {
    if (!aiActive || isLoading) return;

    try {
      const ctx = await invoke("get_cursor_context");
      const key = `${ctx.process_name}|${ctx.window_title}`;

      if (key !== lastContext && ctx.process_name) {
        lastContext = key;
        updateContextBar(ctx);
      }
    } catch {
      // Silently fail polling
    }
  }, 1000);
}

function stopContextPolling() {
  if (contextPollingInterval) {
    clearInterval(contextPollingInterval);
    contextPollingInterval = null;
  }
  if (hoverTimer) {
    clearTimeout(hoverTimer);
    hoverTimer = null;
  }
}

function updateContextBar(ctx) {
  if (contextBar) {
    contextBar.hidden = false;
    contextApp.textContent = ctx.process_name || "Desktop";
    contextWindow.textContent = ctx.window_title || "";
  }
}

function clearContext() {
  if (contextBar) contextBar.hidden = true;
  contextApp.textContent = "";
  contextWindow.textContent = "";
}

// ── Chat ────────────────────────────────────────────────
async function handleSend() {
  const text = chatInput.value.trim();
  if (!text || isLoading) return;

  chatInput.value = "";
  autoResizeTextarea();
  clearWelcome();

  addMessage("user", text);

  if (agentMode) {
    await runAgentTask(text);
    return;
  }

  const shouldSearch = text.toLowerCase().startsWith("search") ||
    text.includes("latest") || text.includes("current") ||
    text.includes("today") || text.includes("news");

  try {
    showThinking();
    isLoading = true;
    sendBtn.disabled = true;

    const result = await withTimeout(
      invoke("send_chat", {
        text,
        imageDataUrl: null,
        webSearch: shouldSearch,
      }),
      55000
    );

    removeThinking();
    addMessage("assistant", result.text);
  } catch (err) {
    removeThinking();
    addMessage("assistant", `❌ Error: ${err}`);
  } finally {
    isLoading = false;
    sendBtn.disabled = false;
  }
}

// ── Agent mode ─────────────────────────────────────────
function toggleAgentMode() {
  agentMode = !agentMode;
  const btn = $("#btn-agent-mode");
  const stateEl = $("#agent-mode-state");
  if (btn) btn.classList.toggle("active", agentMode);
  if (stateEl) stateEl.textContent = agentMode ? "ON" : "OFF";
  chatInput.placeholder = agentMode
    ? "Ask Mouse AI to do anything…"
    : "Ask Mouse AI anything…";
}

async function runAgentTask(text) {
  isLoading = true;
  sendBtn.disabled = true;
  // Fresh activity panel for this run; agent-progress events append into it.
  activeAgentActivityEl = createAgentActivity();

  try {
    // No client timeout — a multi-step task can legitimately take minutes.
    // The Rust loop is bounded by its own iteration + per-command limits.
    const result = await invoke("agent_run", {
      task: text,
      source: "main",
      autonomous: true,
    });
    finishAgentActivity(true);
    addMessage("assistant", result.text || "(done)");
  } catch (err) {
    finishAgentActivity(false);
    addMessage("assistant", `⚠️ ${friendlyAgentError(err)}`);
  } finally {
    isLoading = false;
    sendBtn.disabled = false;
    activeAgentActivityEl = null;
  }
}

function createAgentActivity() {
  const wrap = document.createElement("div");
  wrap.className = "message assistant";
  wrap.innerHTML = `<div class="agent-activity">
    <div class="agent-activity-head"><span class="agent-spinner"></span> Working…</div>
    <div class="agent-activity-log"></div>
  </div>`;
  messagesEl.appendChild(wrap);
  scrollToBottom();
  return wrap;
}

function renderAgentProgress(p) {
  if (!activeAgentActivityEl) return;
  const log = activeAgentActivityEl.querySelector(".agent-activity-log");
  if (!log) return;

  if (p.kind === "done") {
    const head = activeAgentActivityEl.querySelector(".agent-activity-head");
    if (head) head.innerHTML = "✓ Done";
    return;
  }

  if (p.kind === "confirm") {
    log.appendChild(buildConfirmPrompt(p));
    scrollToBottom();
    return;
  }

  const line = document.createElement("div");
  line.className = `agent-line agent-${p.kind}`;
  const icon = { thinking: "💭", tool: "⚙️", "tool-result": "↳", error: "⚠️" }[p.kind] || "•";
  // Route error events through the same friendly formatter as the final
  // failure bubble, so this inline log line doesn't show a raw JSON/HTTP
  // error string just before the friendlier one appears below it.
  const message = p.kind === "error" ? friendlyAgentError(p.message) : p.message;
  line.textContent = `${icon} ${message}`;
  log.appendChild(line);
  scrollToBottom();
}

// Renders a real Allow/Deny prompt the agent is waiting on, and reports the
// user's click straight back to the paused tool call in Rust.
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

function finishAgentActivity(success) {
  if (!activeAgentActivityEl) return;
  const head = activeAgentActivityEl.querySelector(".agent-activity-head");
  if (!head) return;
  head.innerHTML = success ? "✓ Done" : "✗ Failed";
  head.classList.toggle("failed", !success);
}

// Turns a raw Rust/backend error string into something a non-technical user
// can act on, mirroring overlay.js's friendlyError (kept in sync manually —
// the two windows don't share a module system here).
function friendlyAgentError(err) {
  const msg = String(err?.message || err || "Unknown error");
  if (/not signed in/i.test(msg)) {
    return "You're not signed in. Open Settings to sign in first.";
  }
  if (/can't reach|network|timed out|connect/i.test(msg)) {
    return "Can't reach the Thothai backend. Check your internet or the Backend URL in Settings.";
  }
  if (/empty response/i.test(msg)) {
    return "The model didn't respond after several tries — it may be under heavy load. Try again in a moment.";
  }
  if (/step limit/i.test(msg)) {
    return "That task took more steps than I could finish in one go — try breaking it into smaller steps.";
  }
  if (/daily message limit|daily limit/i.test(msg)) {
    return "You've hit today's free message limit. It resets daily — try again tomorrow.";
  }
  if (/referer|permission_denied|rejected every configured api key|api key not valid/i.test(msg)) {
    return "The backend's Gemini API key is misconfigured (not something you can fix here) — let the app owner know it needs a working key.";
  }
  if (/429|rate limit/i.test(msg)) {
    return "Too many requests right now — wait a few seconds and try again.";
  }
  return msg;
}

async function handleCaptureAnalyze() {
  if (isLoading) return;

  clearWelcome();
  addSystemMessage("📸 Capturing screen around cursor...");

  try {
    showThinking();
    isLoading = true;
    sendBtn.disabled = true;

    const result = await withTimeout(
      invoke("analyze_at_cursor", {
        userQuestion: null,
        radius: 300,
      }),
      55000
    );

    removeThinking();
    addMessage("assistant", result.text);
  } catch (err) {
    removeThinking();
    addMessage("assistant", `❌ Capture error: ${err}`);
  } finally {
    isLoading = false;
    sendBtn.disabled = false;
  }
}

async function handleNewChat() {
  try {
    await invoke("new_conversation");
    messages = [];
    messagesEl.innerHTML = "";
    showWelcome();
  } catch (err) {
    console.error("New chat error:", err);
  }
}

// ── Message Rendering ──────────────────────────────────
function addMessage(role, content) {
  messages.push({ role, content });

  const div = document.createElement("div");
  div.className = `message ${role}`;

  const bubble = document.createElement("div");
  bubble.className = "msg-content";

  if (role === "assistant") {
    bubble.innerHTML = renderMarkdown(content);
  } else {
    bubble.textContent = content;
  }

  div.appendChild(bubble);
  messagesEl.appendChild(div);
  scrollToBottom();
}

function addSystemMessage(text) {
  const div = document.createElement("div");
  div.className = "message system";
  div.innerHTML = `<div class="msg-content" style="
    background: var(--bg-tertiary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    max-width: 100%;
    text-align: center;
    color: var(--text-muted);
    font-size: 11px;
    padding: 6px 12px;
  ">${escapeHtml(text)}</div>`;
  messagesEl.appendChild(div);
  scrollToBottom();
}

function showThinking() {
  const div = document.createElement("div");
  div.className = "message assistant";
  div.id = "thinking-indicator";
  div.innerHTML = `<div class="msg-content">
    <div class="thinking">
      <div class="thinking-dot"></div>
      <div class="thinking-dot"></div>
      <div class="thinking-dot"></div>
    </div>
  </div>`;
  messagesEl.appendChild(div);
  scrollToBottom();
}

function removeThinking() {
  const el = document.getElementById("thinking-indicator");
  if (el) el.remove();
}

function clearWelcome() {
  const welcome = messagesEl.querySelector(".welcome-message");
  if (welcome) welcome.remove();
}

function showWelcome() {
  messagesEl.innerHTML = `
    <div class="welcome-message">
      <div class="welcome-icon">🖱️✨</div>
      <h3>Mouse AI Ready</h3>
      <p>Press <kbd>Ctrl+Shift+Space</kbd> anywhere — an ask box opens right at your cursor about whatever you're pointing at. Or press <kbd>Ctrl+Shift+Q</kbd> for hover mode: pause the mouse and a <strong>✦ Ask AI</strong> button appears.</p>
      <div class="quick-actions">
        <button class="action-chip" data-action="capture">📸 Capture & Analyze</button>
        <button class="action-chip" data-action="ask">💬 Quick Ask</button>
        <button class="action-chip" data-action="search">🔍 Web Search</button>
      </div>
    </div>`;

  messagesEl.querySelectorAll(".action-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      const action = chip.dataset.action;
      if (action === "capture") handleCaptureAnalyze();
      else if (action === "ask") chatInput?.focus();
      else if (action === "search") {
        chatInput.value = "Search the web for: ";
        chatInput.focus();
      }
    });
  });
}

// ── Simple Markdown Renderer ───────────────────────────
function renderMarkdown(text) {
  if (!text) return "";

  let html = escapeHtml(text);

  // Code blocks (```...```)
  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
    return `<pre><code class="language-${lang}">${code.trim()}</code></pre>`;
  });

  // Inline code (`...`)
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");

  // Bold (**...**)
  html = html.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");

  // Italic (*...*)
  html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, "<em>$1</em>");

  // Headers
  html = html.replace(/^### (.+)$/gm, "<h3>$1</h3>");
  html = html.replace(/^## (.+)$/gm, "<h2>$1</h2>");
  html = html.replace(/^# (.+)$/gm, "<h1>$1</h1>");

  // Links [text](url)
  html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank">$1</a>');

  // Blockquotes
  html = html.replace(/^&gt; (.+)$/gm, "<blockquote>$1</blockquote>");

  // Unordered lists
  html = html.replace(/^[*\-] (.+)$/gm, "<li>$1</li>");
  html = html.replace(/(<li>.*<\/li>\n?)+/g, "<ul>$&</ul>");

  // Ordered lists
  html = html.replace(/^\d+\. (.+)$/gm, "<li>$1</li>");

  // Paragraphs (double newline)
  html = html.replace(/\n\n/g, "</p><p>");
  html = html.replace(/\n/g, "<br>");

  // Wrap in paragraph
  if (!html.startsWith("<")) html = `<p>${html}</p>`;

  return html;
}

function escapeHtml(text) {
  const div = document.createElement("div");
  div.textContent = text;
  return div.innerHTML;
}

// ── UI Helpers ─────────────────────────────────────────
function showLoading(show) {
  if (loadingOverlay) loadingOverlay.hidden = !show;
}

function showError(el, msg, ms = 5000) {
  if (el) {
    el.textContent = msg;
    el.hidden = false;
    setTimeout(() => (el.hidden = true), ms);
  }
}

function showSettings() {
  mainView.hidden = true;
  settingsPanel.hidden = false;

  // Load the current backend URL into the field.
  invoke("get_backend_url")
    .then((url) => {
      const input = $("#backend-url-input");
      if (input) input.value = url || "";
    })
    .catch(() => {});
  loadMcpServers();
}

async function handleSaveBackendUrl() {
  const input = $("#backend-url-input");
  const status = $("#backend-url-status");
  const url = input?.value?.trim();
  if (!url) return;

  try {
    const saved = await invoke("set_backend_url", { url });
    if (status) status.textContent = `✓ Saved: ${saved}`;
  } catch (err) {
    if (status) status.textContent = `Failed to save: ${err}`;
  }
}

async function loadMcpServers() {
  const input = $("#mcp-servers-input");
  try {
    const json = await invoke("get_mcp_servers");
    if (input && json && json.trim() !== "[]") input.value = json;
  } catch { /* ignore */ }
}

async function handleSaveMcp() {
  const input = $("#mcp-servers-input");
  const status = $("#mcp-status");
  try {
    const msg = await invoke("set_mcp_servers", { json: input?.value || "[]" });
    if (status) status.textContent = `✓ ${msg} MCP tools load next time the agent runs.`;
  } catch (err) {
    if (status) status.textContent = `✗ ${err?.message || err}`;
  }
}

async function handleTestBackend() {
  const status = $("#backend-url-status");
  const input = $("#backend-url-input");
  if (status) status.textContent = "Testing connection…";

  try {
    // Save whatever is in the field first so the test hits that URL.
    const url = input?.value?.trim();
    if (url) await invoke("set_backend_url", { url });

    const base = await withTimeout(invoke("test_backend"), 15000);
    if (status) status.textContent = `✓ Connected to ${base}`;
  } catch (err) {
    if (status) status.textContent = `✗ ${err?.message || err}`;
  }
}

function hideSettings() {
  settingsPanel.hidden = true;
  mainView.hidden = false;
}

function scrollToBottom() {
  requestAnimationFrame(() => {
    messagesContainer.scrollTop = messagesContainer.scrollHeight;
  });
}

function autoResizeTextarea() {
  if (!chatInput) return;
  chatInput.style.height = "auto";
  chatInput.style.height = Math.min(chatInput.scrollHeight, 120) + "px";
}
