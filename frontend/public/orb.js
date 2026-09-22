// Ghost AI — the "thinking orb": a dotted canvas loading indicator with nine states,
// each saying what the assistant is actually doing (searching, solving, connecting…)
// rather than just "busy". Geometry comes from shared/orb-engine.js (vendored from the
// thinking-orbs package); this file is the rendering, theming and lifecycle around it.
//
// Used by content.js (in-page panel, arbitrary hostile pages), undercover.js and the
// dashboard, so it has to survive page CSS, re-injection, and having no engine at all.
(function (global) {
  "use strict";

  // content.js is force-reinjected on stale SPA tabs, which re-evaluates this file.
  // Tear down the previous instance's orbs so its rAF loop isn't left driving canvases
  // that are no longer in the document.
  try { global.GhostOrb?.destroyAll?.(); } catch { /* first load */ }

  const STATES = [
    "working",    // orbits — the neutral "busy"
    "searching",  // globe  — scanning / web research
    "solving",    // rubik  — multi-step reasoning
    "listening",  // wave   — mic open
    "connecting", // web    — wiring sources together
    "weaving",    // braid  — running a tool
    "composing",  // ribbon — writing the answer
    "breathing",  // ring   — idle / waiting on the user
    "shaping",    // morph  — making sense of an image
  ];

  // Announced to screen readers; the visible phase label is separate and live.
  const STATE_LABELS = {
    working: "Working", searching: "Searching", solving: "Thinking",
    listening: "Listening", connecting: "Connecting", weaving: "Working",
    composing: "Writing", breathing: "Waiting", shaping: "Analyzing",
  };

  // The engine ships two hand-tuned designs, not one design at two scales, and
  // resolvePreset throws for anything else. Arbitrary display sizes are reached by
  // scaling the canvas transform, never by asking the engine for an odd size.
  const PRESETS = [20, 64];
  const DPR_CAP = 2;
  // Matches the upstream React component's reduced-motion frame, so the still image
  // is the same pose everywhere. Several modes are a degenerate blob at t=0.
  const STATIC_T = 0.6;

  function engine() {
    return global.GhostOrbEngine;
  }

  /** True when the vendored engine is present and has the surface we rely on. */
  function isAvailable() {
    const E = engine();
    return !!(E && E.resolvePreset && E.MODE_DRAWS && E.STATE_TO_MODE);
  }

  function normalizeState(state) {
    const E = engine();
    return E && E.STATE_TO_MODE[state] ? state : "working";
  }

  // The 64 design is not "the 20 design, bigger" — it is a denser, richer piece of
  // geometry (a searching globe is 204 dots at 64 and a fraction of that at 20). So
  // which design to use is worth choosing independently of the display size: a 30px
  // orb drawn from the 64 design reads as detailed rather than sparse.
  function presetFor(size, explicit) {
    if (explicit === 20 || explicit === 64) return explicit;
    return size >= 28 ? PRESETS[1] : PRESETS[0];
  }

  function prefersReducedMotion() {
    try { return global.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true; }
    catch { return false; }
  }

  function currentDpr() {
    return Math.min(DPR_CAP, global.devicePixelRatio || 1);
  }

  // ── Theme ────────────────────────────────────────────────────────
  // shared/adaptive-theme.js tags the panel root with data-ghost-surface as the page
  // behind it changes brightness; that is the most accurate signal available in a
  // content script, because it describes the surface the orb is actually drawn on.
  // The undercover window sets it too. The dashboard does not load adaptive-theme at
  // all, so there we fall back to the OS preference.
  function resolveDark(el, theme) {
    if (theme === "dark") return true;
    if (theme === "light") return false;
    const host = el?.closest?.("[data-ghost-surface]");
    if (host) return host.dataset.ghostSurface !== "light";
    try {
      if (global.matchMedia?.("(prefers-color-scheme: light)").matches) return false;
    } catch { /* matchMedia unavailable */ }
    // #ghost-ai-root declares color-scheme: dark and the undercover default is dark,
    // so dark is the right guess when nothing else says otherwise.
    return true;
  }

  // ── Shared animation driver ──────────────────────────────────────
  // One rAF loop for every orb on the page. Several orbs each running their own loop
  // would multiply the per-frame cost for no benefit, and a content script has to be a
  // good guest on pages it does not own.
  const handles = new Set(); // every live orb
  const live = new Set();    // orbs that currently want frames
  let rafId = 0;
  let docHidden = false;
  let sharedObserver = null;

  function tick(now) {
    rafId = 0;
    for (const orb of live) orb._frame(now);
    schedule();
  }

  function schedule() {
    if (!rafId && live.size && !docHidden) rafId = requestAnimationFrame(tick);
  }

  function unschedule() {
    if (rafId) { cancelAnimationFrame(rafId); rafId = 0; }
  }

  function setLive(orb, wanted) {
    if (wanted) {
      if (!live.has(orb)) { live.add(orb); orb._last = 0; }
      schedule();
    } else if (live.delete(orb) && !live.size) {
      unschedule();
    }
  }

  function getObserver() {
    if (sharedObserver || typeof IntersectionObserver === "undefined") return sharedObserver;
    sharedObserver = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        const orb = entry.target.__ghostOrb;
        if (orb) orb._setVisible(entry.isIntersecting);
      }
    }, { threshold: 0 });
    return sharedObserver;
  }

  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      docHidden = document.hidden;
      // rAF already stalls in a background tab, but the undercover popup can be a real
      // always-on-top window whose frames keep coming when it is minimized.
      if (docHidden) unschedule(); else { for (const o of live) o._last = 0; schedule(); }
    });
  }

  // A DPR change (monitor switch, browser zoom) needs every canvas re-measured. The
  // one-shot media query is the reliable signal; the per-frame float compare below is
  // the backstop for the setups where it does not fire.
  function watchDpr() {
    try {
      const mq = global.matchMedia(`(resolution: ${global.devicePixelRatio || 1}dppx)`);
      mq.addEventListener("change", () => {
        for (const orb of handles) orb._resize();
        watchDpr();
      }, { once: true });
    } catch { /* unsupported — the per-frame check still covers it */ }
  }
  if (typeof global.matchMedia === "function") watchDpr();

  // ── Fallback markup ──────────────────────────────────────────────
  // Byte-identical to what showTyping() used to emit, so overlay.css's existing
  // .ghost-typing-dots rules animate it with no extra styling. This is what appears if
  // the engine file is missing (npm run sync:orbs never ran) or canvas is blocked.
  function buildDots() {
    const dots = document.createElement("span");
    dots.className = "ghost-typing-dots";
    dots.innerHTML = "<span></span><span></span><span></span>";
    return dots;
  }

  /**
   * Mounts an orb inside `container`.
   *
   * @param {Element} container
   * @param {object}  [opts]
   * @param {string}  [opts.state="working"]  one of STATES
   * @param {number}  [opts.size=22]          CSS px
   * @param {number}  [opts.preset]           force the 20 or 64 geometry
   * @param {number}  [opts.speed=1]          multiplier on the preset's own speed
   * @param {string}  [opts.label=""]         visible status text ("" = no label element)
   * @param {string}  [opts.theme="auto"]     "auto" | "dark" | "light"
   * @param {string}  [opts.ariaLabel]        defaults to the state's label
   * @param {number}  [opts.fps=30]           frame cap
   * @param {string}  [opts.className=""]     extra class on the wrapper
   * @returns {object} handle — same shape whether it rendered a canvas or the fallback
   */
  function create(container, opts = {}) {
    const {
      state = "working", size = 22, speed = 1, label = "",
      theme = "auto", ariaLabel = null, fps = 30, className = "", preset: presetOpt,
    } = opts;

    const wrap = document.createElement("span");
    wrap.className = "ghost-orb-wrap" + (className ? ` ${className}` : "");
    const orbEl = document.createElement("span");
    orbEl.className = "ghost-orb";
    orbEl.setAttribute("role", "img");
    wrap.appendChild(orbEl);

    let labelEl = null;
    if (label) {
      labelEl = document.createElement("span");
      labelEl.className = "ghost-orb-label";
      // The rotating phase text is the part worth announcing; the state's aria-label
      // is not a live region precisely so it does not also fire on every change.
      labelEl.setAttribute("aria-live", "polite");
      labelEl.setAttribute("aria-atomic", "true");
      labelEl.textContent = label;
      wrap.appendChild(labelEl);
    }

    container?.appendChild(wrap);

    const handle = {
      el: wrap, orbEl, labelEl, isFallback: false,
      setState() {}, setLabel() {}, setSpeed() {}, setTheme() {}, pause() {}, resume() {}, destroy() {},
    };

    // setLabel and destroy work identically in both modes, so callers never branch.
    handle.setLabel = (text) => {
      if (!labelEl || labelEl.textContent === text) return; // don't re-announce
      labelEl.textContent = text;
      // Restart the fade without the classic force-reflow hack.
      labelEl.classList.remove("ghost-orb-label-in");
      void labelEl.offsetWidth;
      labelEl.classList.add("ghost-orb-label-in");
    };

    function useFallback() {
      handle.isFallback = true;
      orbEl.textContent = "";
      orbEl.appendChild(buildDots());
      orbEl.setAttribute("aria-label", ariaLabel || STATE_LABELS[normalizeState(state)] || "Working");
      handle.destroy = () => { wrap.remove(); };
      return handle;
    }

    if (!isAvailable()) return useFallback();

    const canvas = document.createElement("canvas");
    canvas.className = "ghost-orb-canvas";
    canvas.setAttribute("aria-hidden", "true"); // the role/label live on orbEl
    // Inline + !important because a content script's stylesheet is ordered BEFORE the
    // page's own, so a common `canvas { max-width:100%; height:auto }` rule would win
    // and aspect-scale the orb into nothing. An element-level !important cannot lose.
    canvas.style.setProperty("width", `${size}px`, "important");
    canvas.style.setProperty("height", `${size}px`, "important");
    canvas.style.setProperty("display", "block", "important");
    orbEl.appendChild(canvas);
    wrap.style.setProperty("--ghost-orb-size", `${size}px`);

    const ctx = canvas.getContext("2d");
    if (!ctx) { canvas.remove(); return useFallback(); } // canvas-blocked / hardened page

    const E = engine();
    const preset = presetFor(size, presetOpt);
    let curState = normalizeState(state);
    let resolved = E.resolvePreset(curState, preset);
    let speedMul = speed;
    let themeOpt = theme;
    let dark = resolveDark(orbEl, themeOpt);
    let dpr = currentDpr();
    let reduced = prefersReducedMotion();
    let minFrameMs = 1000 / Math.max(1, fps);
    let userPaused = false;
    let visible = true; // optimistic, so the first frame paints before IO's first callback
    let destroyed = false;
    let painted = 0;
    let broken = false;

    orbEl.setAttribute("aria-label", ariaLabel || STATE_LABELS[curState] || "Working");
    orbEl.__ghostOrb = handle;

    function resize() {
      dpr = currentDpr();
      const backing = Math.max(1, Math.round(size * dpr));
      canvas.width = backing;   // also clears the canvas and resets the 2D state,
      canvas.height = backing;  // so the transform has to be re-applied after this
      paint();
    }
    handle._resize = resize;

    function paint() {
      if (destroyed || broken) return;
      try {
        const k = (size * dpr) / preset;
        ctx.setTransform(k, 0, 0, k, 0, 0);
        ctx.clearRect(0, 0, preset, preset);
        E.MODE_DRAWS[resolved.mode](ctx, preset, orbTime, dark, resolved.opts);
      } catch {
        // A painter that throws once will throw every frame — degrade instead of
        // spamming the page's console 30 times a second.
        broken = true;
        setLive(handle, false);
        canvas.remove();
        useFallback();
      }
    }

    let orbTime = reduced ? STATIC_T : 0;
    handle._last = 0;

    handle._frame = function (now) {
      const dt = handle._last ? Math.min(now - handle._last, 100) : 0; // clamp tab-switch gaps
      handle._last = now;
      orbTime += (dt / 1000) * resolved.speed * speedMul;
      if (now - painted < minFrameMs) return;
      painted = now;
      if (dpr !== currentDpr()) { resize(); return; }
      paint();
    };

    handle._setVisible = function (v) {
      visible = v;
      sync();
    };

    function sync() {
      setLive(handle, !destroyed && !broken && !reduced && visible && !userPaused && !docHidden);
    }

    // ── theme reactivity ──
    let themeObserver = null;
    let colorSchemeMq = null;
    function refreshTheme() {
      const next = resolveDark(orbEl, themeOpt);
      if (next === dark) return;
      dark = next;
      paint(); // repaint immediately — matters most when reduced motion means no loop
    }
    function watchTheme() {
      themeObserver?.disconnect();
      themeObserver = null;
      const host = orbEl.closest?.("[data-ghost-surface]");
      if (host && typeof MutationObserver !== "undefined") {
        themeObserver = new MutationObserver(refreshTheme);
        themeObserver.observe(host, { attributes: true, attributeFilter: ["data-ghost-surface"] });
      } else if (!colorSchemeMq && typeof global.matchMedia === "function") {
        try {
          colorSchemeMq = global.matchMedia("(prefers-color-scheme: dark)");
          colorSchemeMq.addEventListener("change", refreshTheme);
        } catch { colorSchemeMq = null; }
      }
    }
    watchTheme();

    // Reduced-motion can be toggled while the orb is on screen.
    let motionMq = null;
    try {
      motionMq = global.matchMedia("(prefers-reduced-motion: reduce)");
      motionMq.addEventListener("change", (e) => {
        reduced = e.matches;
        if (reduced) { orbTime = STATIC_T; paint(); }
        sync();
      });
    } catch { /* unsupported */ }

    getObserver()?.observe(orbEl);
    handles.add(handle);

    resize();   // first paint
    sync();

    handle.setState = function (next) {
      const s = normalizeState(next);
      if (s === curState) return;
      curState = s;
      resolved = E.resolvePreset(curState, preset);
      orbEl.setAttribute("aria-label", ariaLabel || STATE_LABELS[curState] || "Working");
      // The adaptive-theme host may only have appeared after this orb was created
      // (content.js themes the panel after the scripts load) — pick it up now.
      if (!themeObserver) watchTheme();
      refreshTheme();
      paint();
    };
    handle.setSpeed = function (n) { speedMul = Number(n) || 1; };
    handle.setTheme = function (t) {
      themeOpt = t;
      themeObserver?.disconnect();
      themeObserver = null;
      if (themeOpt === "auto") watchTheme();
      refreshTheme();
      paint();
    };
    handle.pause = function () { userPaused = true; sync(); };
    handle.resume = function () { userPaused = false; sync(); };
    handle.destroy = function () {
      if (destroyed) return;
      destroyed = true;
      setLive(handle, false);
      handles.delete(handle);
      sharedObserver?.unobserve(orbEl);
      themeObserver?.disconnect();
      try { colorSchemeMq?.removeEventListener("change", refreshTheme); } catch { /* ignore */ }
      delete orbEl.__ghostOrb;
      wrap.remove();
    };

    return handle;
  }

  /** Destroys every live orb — used by content.js cleanup and on re-injection. */
  function destroyAll() {
    for (const orb of [...handles]) {
      try { orb.destroy(); } catch { /* keep going */ }
    }
    handles.clear();
    live.clear();
    unschedule();
  }

  global.GhostOrb = { create, isAvailable, destroyAll, STATES, STATE_LABELS };
})(typeof window !== "undefined" ? window : globalThis);
