// Mouse AI "working" banner (shield.html). Mirrors agent-progress for whichever run is
// driving the screen, shows the current step, and offers Stop. Shown/hidden by the Rust
// side (shield.rs) around every local agent run.
(function () {
  "use strict";
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const stepEl = document.getElementById("bar-step");
  const pauseEl = document.getElementById("bar-pause");
  const stopBtn = document.getElementById("bar-stop");
  let source = null;

  const LABEL = {
    plan: "Planning",
    thinking: "Thinking",
    tool: "Working",
    "tool-result": "Checking the result",
    confirm: "Waiting for your approval",
    error: "Ran into a problem",
    done: "Done",
  };

  function setStep(kind, message) {
    const text = String(message || "").replace(/\s+/g, " ").trim();
    stepEl.textContent = text ? (text.length > 72 ? text.slice(0, 71) + "…" : text) : (LABEL[kind] || "Working");
    document.body.dataset.kind = kind || "";
    pauseEl.hidden = !(kind === "thinking" && /waiting[^.]*you/i.test(text));
  }

  listen("agent-progress", (e) => {
    const p = e.payload || {};
    if (!p.source) return;
    if (p.kind === "done") { setStep("done", "Done"); return; }
    source = p.source;
    setStep(p.kind, p.message);
  });
  listen("shield-show", (e) => {
    source = e.payload?.source || source;
    setStep("plan", e.payload?.task ? `Task: ${e.payload.task}` : "Starting…");
  });

  stopBtn.addEventListener("click", () => {
    invoke("agent_cancel_all").catch(() => {});
    setStep("done", "Stopping…");
  });
  listen("shield-stopping", () => setStep("done", "Stopping… (Esc)"));
})();
