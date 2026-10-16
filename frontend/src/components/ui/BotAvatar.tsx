import { useEffect, useId, useRef, useState } from "react";
import clsx from "clsx";
import { CHARACTERS, buildSvg, eyesMarkup } from "../../lib/kudu-art.generated";

// The Kudu family: one plush kudu per product, drawn by the shared art module
// (src/lib/kudu-art.generated.ts, synced from the extension's shared/kudu-art.js) and
// animated by src/kudu-art.generated.css. Which character stands for which agent is
// decided once in lib/bots.ts.
//
//   kudu    the brand / Kudu Desktop      agent   the autonomous worker (scarf)
//   connect the browser agent (mint)      lens    Ask at cursor (monocle)
//   pilot   meetings (headset)            scout   web research (scanning ears)
//   quill   writing (pencil)              cloak   hidden window (mask)
//
// Old kind names from before the rebrand still work: clover → pilot, flower → connect,
// square → agent, ghost → kudu.

export type BotKind = "kudu" | "agent" | "connect" | "lens" | "pilot" | "scout" | "quill" | "cloak" | "clover" | "flower" | "square" | "ghost";
/** "still" = no motion at all (for long lists - only the newest bot animates). */
export type BotMood = "idle" | "still" | "thinking" | "working" | "listening" | "happy" | "sleeping" | "error";
export type BotVariant = "mascot" | "body" | "mark" | "glyph";

const LEGACY: Record<string, string> = { clover: "pilot", flower: "connect", square: "agent", ghost: "kudu" };
export function characterOf(kind: BotKind): string {
  const c = LEGACY[kind] || kind;
  return c in CHARACTERS ? c : "kudu";
}

// ── Presence: gaze, glances, ear flicks ────────────────────────────────────────
// Eyes follow the pointer by a few percent - the small thing that makes a character
// feel present. One document listener for every avatar, rAF-throttled. Coming close to
// a kudu flicks the ear on your side.
type Live = HTMLElement & { __flickAt?: number; __inside?: boolean };
const live = new Set<Live>();
let gazeBound = false;
let gazeRaf = 0;
let gazeX = 0;
let gazeY = 0;

function flickEar(el: Live, side: "l" | "r") {
  if (el.classList.contains("bot-sleeping") || el.classList.contains("bot-still")) return;
  const now = Date.now();
  if (now - (el.__flickAt || 0) < 1200) return;
  el.__flickAt = now;
  const ear = el.querySelector<SVGGElement>(`.bot-ear-${side}`);
  if (!ear || ear.style.animation) return;
  ear.style.animation = `bot-ear-flick-${side} .55s ease-out`;
  ear.addEventListener("animationend", () => { ear.style.animation = ""; }, { once: true });
}

function applyGaze() {
  gazeRaf = 0;
  for (const el of live) {
    const r = el.getBoundingClientRect();
    if (!r.width) continue;
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    const dx = Math.max(-1, Math.min(1, (gazeX - cx) / Math.max(160, window.innerWidth / 3)));
    const dy = Math.max(-1, Math.min(1, (gazeY - cy) / Math.max(120, window.innerHeight / 3)));
    el.style.setProperty("--bot-ex", `${(dx * 4).toFixed(2)}%`);
    el.style.setProperty("--bot-ey", `${(dy * 3).toFixed(2)}%`);
    const pad = 8;
    const inside = gazeX >= r.left - pad && gazeX <= r.right + pad && gazeY >= r.top - pad && gazeY <= r.bottom + pad;
    if (inside && !el.__inside) flickEar(el, gazeX < cx ? "l" : "r");
    el.__inside = inside;
  }
}
// Idle behaviour: when the pointer has been still, glance somewhere and come back,
// double-blink, or flick an ear - so a character that is just sitting there feels alive.
let lastPointerAt = 0;
let glanceTimer: number | undefined;
function scheduleGlance() {
  window.clearTimeout(glanceTimer);
  glanceTimer = window.setTimeout(() => {
    const idleFor = Date.now() - lastPointerAt;
    for (const el of live) {
      if (idleFor < 2500 || el.classList.contains("bot-sleeping")) continue;
      const roll = Math.random();
      if (roll < 0.5) {
        el.style.setProperty("--bot-ex", `${(Math.random() * 8 - 4).toFixed(2)}%`);
        el.style.setProperty("--bot-ey", `${(Math.random() * 5 - 3).toFixed(2)}%`);
        window.setTimeout(() => {
          if (Date.now() - lastPointerAt > 2000) { el.style.setProperty("--bot-ex", "0%"); el.style.setProperty("--bot-ey", "0%"); }
        }, 700 + Math.random() * 900);
      } else if (roll < 0.75) {
        const eyes = el.querySelector<SVGGElement>(".bot-eyes");
        if (eyes && !el.classList.contains("bot-thinking")) { eyes.style.animation = "bot-blink .35s 2"; window.setTimeout(() => { eyes.style.animation = ""; }, 800); }
      } else {
        el.__flickAt = 0;
        flickEar(el, Math.random() < 0.5 ? "l" : "r");
      }
    }
    scheduleGlance();
  }, 2800 + Math.random() * 4200);
}
function bindGaze() {
  if (gazeBound || typeof document === "undefined") return;
  gazeBound = true;
  scheduleGlance();
  document.addEventListener(
    "pointermove",
    (e) => {
      gazeX = e.clientX;
      gazeY = e.clientY;
      lastPointerAt = Date.now();
      if (!gazeRaf) gazeRaf = requestAnimationFrame(applyGaze);
    },
    { passive: true, capture: true },
  );
}

export function BotAvatar({
  kind = "kudu",
  mood = "idle",
  size = 32,
  className,
  title,
  variant = "mascot",
}: {
  kind?: BotKind;
  mood?: BotMood;
  size?: number;
  className?: string;
  title?: string;
  /** "body" = the whole plush figure (48 px and up), "mark" = the logo tile, "glyph" = one colour. */
  variant?: BotVariant;
}) {
  const uid = useId().replace(/:/g, "");
  const character = characterOf(kind);
  const ref = useRef<HTMLSpanElement>(null);
  // The markup is built once; only the eyes slot changes with the mood (as in the
  // extension's engine), so the ears/horns/props keep their running animations.
  const [html] = useState(() => buildSvg(uid, size, mood, { prefix: "bot", variant, character }));
  const first = useRef(true);
  useEffect(() => {
    if (first.current) { first.current = false; return; }
    const slot = ref.current?.querySelector(".bot-eyes-slot");
    if (!slot) return;
    const headPx = variant === "body" ? size * 0.62 : size;
    const dark = !!(CHARACTERS as Record<string, { dark?: boolean }>)[character]?.dark || character === "cloak";
    slot.innerHTML = eyesMarkup(variant === "mark" ? "still" : mood, headPx, "bot", dark && headPx >= 20 ? { sclera: true } : undefined);
  }, [mood, size, variant, character]);

  const prevMood = useRef(mood);
  const [waking, setWaking] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || size < 20 || mood === "still" || (variant !== "mascot" && variant !== "body")) return;
    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    live.add(el);
    bindGaze();
    return () => {
      live.delete(el);
    };
  }, [size, mood, variant]);
  // Waking up: a stretch, perked ears and a double blink on top of the new mood.
  useEffect(() => {
    const was = prevMood.current;
    prevMood.current = mood;
    if (was !== "sleeping" || mood === "sleeping") return;
    setWaking(true);
    const t = window.setTimeout(() => setWaking(false), 900);
    return () => window.clearTimeout(t);
  }, [mood]);

  return (
    <span
      ref={ref}
      className={clsx("bot inline-flex flex-shrink-0", `bot-${character}`, `bot-${mood}`, waking && "bot-waking", className)}
      style={{ width: size, height: size }}
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      title={title}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
