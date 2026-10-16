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

const LEGACY: Record<string, string> = { ghost: "kudu", clover: "pilot" };
const DRAWN = new Set(["flower", "square"]);
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

const PALETTE: Record<string, { light: string; base: string; dark: string }> = {
  clover: { light: "#bfe0ff", base: "#5aa4ec", dark: "#2f6fc0" },
  flower: { light: "#b8f7d4", base: "#25cf7c", dark: "#0f8f52" },
  square: { light: "#ffc2ee", base: "#e44fc6", dark: "#a2258b" },
};

function DrawnBody({ kind, fill }: { kind: string; fill: string }) {
  if (kind === "clover") {
    return (
      <g fill={fill}>
        <circle cx="50" cy="31" r="21" /><circle cx="31" cy="51" r="21" /><circle cx="69" cy="51" r="21" />
        <circle cx="50" cy="69" r="21" /><circle cx="50" cy="50" r="24" />
      </g>
    );
  }
  if (kind === "flower") {
    return (
      <g fill={fill}>
        {[0, 72, 144, 216, 288].map((a) => {
          const r = (a - 90) * (Math.PI / 180);
          return <circle key={a} cx={50 + Math.cos(r) * 20} cy={52 + Math.sin(r) * 20} r="18" />;
        })}
        <circle cx="50" cy="52" r="24" />
      </g>
    );
  }
  return <rect x="20" y="20" width="60" height="62" rx="20" fill={fill} />;
}

function DrawnEyes({ mood }: { mood: BotMood }) {
  const y = 50;
  const ink = "#14213d";
  if (mood === "happy") return (<g fill="none" stroke={ink} strokeWidth="3.4" strokeLinecap="round"><path d={`M35 ${y + 2} q5 -7 10 0`} /><path d={`M55 ${y + 2} q5 -7 10 0`} /></g>);
  if (mood === "sleeping") return (<g fill="none" stroke={ink} strokeWidth="3" strokeLinecap="round"><path d={`M35 ${y} q5 4 10 0`} /><path d={`M55 ${y} q5 4 10 0`} /></g>);
  if (mood === "error") return (<g fill="none" stroke={ink} strokeWidth="3" strokeLinecap="round"><path d={`M36 ${y - 3} l8 6 M44 ${y - 3} l-8 6`} /><path d={`M56 ${y - 3} l8 6 M64 ${y - 3} l-8 6`} /></g>);
  const r = mood === "listening" ? 6.6 : mood === "working" ? 4.9 : 5.6;
  const rx = r * (mood === "working" ? 1 : 0.82);
  const ey = mood === "thinking" ? y - 1.5 : y;
  return (
    <g className="bot-eyes" fill={ink}>
      <ellipse cx="40" cy={ey} rx={rx} ry={r} /><ellipse cx="60" cy={ey} rx={rx} ry={r} />
      <circle cx={41.6} cy={ey - 2} r="1.4" fill="#fff" opacity="0.85" /><circle cx={61.6} cy={ey - 2} r="1.4" fill="#fff" opacity="0.85" />
    </g>
  );
}

/** The original felt bots: clover (meetings), flower (browser agent), square (helpers). */
function DrawnBot({ kind, mood, size, uid }: { kind: string; mood: BotMood; size: number; uid: string }) {
  const c = PALETTE[kind];
  const fuzzy = size >= 28;
  return (
    <svg viewBox="0 0 100 100" width={size} height={size} overflow="visible">
      <defs>
        <radialGradient id={`shade-${uid}`} cx="36%" cy="30%" r="75%">
          <stop offset="0%" stopColor={c.light} /><stop offset="45%" stopColor={c.base} /><stop offset="100%" stopColor={c.dark} />
        </radialGradient>
        <filter id={`felt-${uid}`} x="-15%" y="-15%" width="130%" height="130%">
          <feTurbulence type="fractalNoise" baseFrequency="1.6" numOctaves="2" seed="3" result="noise" />
          <feDisplacementMap in="SourceGraphic" in2="noise" scale="3.2" xChannelSelector="R" yChannelSelector="G" />
        </filter>
        <filter id={`shadow-${uid}`} x="-30%" y="-30%" width="160%" height="170%">
          <feDropShadow dx="0" dy="2" stdDeviation="2.5" floodColor={c.dark} floodOpacity="0.2" />
        </filter>
      </defs>
      <ellipse className="bot-ground" cx="50" cy="92" rx="22" ry="4.5" fill={c.dark} opacity="0.22" />
      <g className="bot-body" filter={`url(#shadow-${uid})`}>
        <g filter={fuzzy ? `url(#felt-${uid})` : undefined}><DrawnBody kind={kind} fill={`url(#shade-${uid})`} /></g>
        <ellipse cx="38" cy="30" rx="10" ry="6" fill="#fff" opacity="0.28" transform="rotate(-25 38 30)" />
        <DrawnEyes mood={mood} />
      </g>
    </svg>
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
  const isDrawn = DRAWN.has(kind);
  const character = isDrawn ? kind : characterOf(kind);
  const ref = useRef<HTMLSpanElement>(null);
  // The markup is built once; only the eyes slot changes with the mood (as in the
  // extension's engine), so the ears/horns/props keep their running animations.
  const [html] = useState(() => (isDrawn ? "" : buildSvg(uid, size, mood, { prefix: "bot", variant, character })));
  const first = useRef(true);
  useEffect(() => {
    if (first.current || isDrawn) { first.current = false; return; }
    const slot = ref.current?.querySelector(".bot-eyes-slot");
    if (!slot) return;
    const headPx = variant === "body" ? size * 0.62 : size;
    const dark = !!(CHARACTERS as Record<string, { dark?: boolean }>)[character]?.dark || character === "cloak";
    slot.innerHTML = character === "pilot"
      ? eyesMarkup(mood, size, "bot", { sclera: false })
      : eyesMarkup(variant === "mark" ? "still" : mood, headPx, "bot", dark && headPx >= 20 ? { sclera: true } : undefined);
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
      {...(isDrawn ? {} : { dangerouslySetInnerHTML: { __html: html } })}
    >
      {isDrawn ? <DrawnBot kind={kind} mood={mood} size={size} uid={uid} /> : null}
    </span>
  );
}
