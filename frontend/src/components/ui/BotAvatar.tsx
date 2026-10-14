import { useEffect, useId, useRef } from "react";
import clsx from "clsx";

// Eyes follow the pointer by a few percent - the small thing that makes a character
// feel present. One document listener for every avatar, rAF-throttled.
const gazeTargets = new Set<HTMLElement>();
let gazeBound = false;
let gazeRaf = 0;
let gazeX = 0;
let gazeY = 0;
function applyGaze() {
  gazeRaf = 0;
  for (const el of gazeTargets) {
    const r = el.getBoundingClientRect();
    if (!r.width) continue;
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    const dx = Math.max(-1, Math.min(1, (gazeX - cx) / Math.max(160, window.innerWidth / 3)));
    const dy = Math.max(-1, Math.min(1, (gazeY - cy) / Math.max(120, window.innerHeight / 3)));
    el.style.setProperty("--bot-ex", `${(dx * 4).toFixed(2)}%`);
    el.style.setProperty("--bot-ey", `${(dy * 3).toFixed(2)}%`);
  }
}
// Idle behaviour: when the pointer has been still, glance somewhere and come back, or
// double-blink - so a character that is just sitting there still feels alive.
let lastPointerAt = 0;
let glanceTimer: number | undefined;
function scheduleGlance() {
  window.clearTimeout(glanceTimer);
  glanceTimer = window.setTimeout(() => {
    const idleFor = Date.now() - lastPointerAt;
    for (const el of gazeTargets) {
      if (idleFor < 2500 || el.classList.contains("bot-sleeping")) continue;
      if (Math.random() < 0.6) {
        el.style.setProperty("--bot-ex", `${(Math.random() * 8 - 4).toFixed(2)}%`);
        el.style.setProperty("--bot-ey", `${(Math.random() * 5 - 3).toFixed(2)}%`);
        window.setTimeout(() => {
          if (Date.now() - lastPointerAt > 2000) { el.style.setProperty("--bot-ex", "0%"); el.style.setProperty("--bot-ey", "0%"); }
        }, 700 + Math.random() * 900);
      } else {
        const eyes = el.querySelector<SVGGElement>(".bot-eyes");
        if (eyes) { eyes.style.animation = "bot-blink .35s 2"; window.setTimeout(() => { eyes.style.animation = ""; }, 800); }
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

// Soft, fuzzy 3D-looking bot characters (after libraries.dev "Bot avatars"),
// drawn in SVG so they stay crisp at any size and animate with plain CSS:
//   • the felt edge is an feTurbulence + feDisplacementMap filter,
//   • the 3D shading is a radial-gradient overlay + an inner highlight,
//   • the eyes blink / look around / squint per `mood` (keyframes in index.css).
//
// One character per agent so the user can tell who is doing what:
//   ghost  → Mouse AI / Thothai (the logo)    flower → the browser agent
//   clover → the meeting copilot              square → background helpers

export type BotKind = "clover" | "flower" | "ghost" | "square";
/** "still" = no motion at all (for long lists - only the newest bot animates). */
export type BotMood = "idle" | "still" | "thinking" | "working" | "listening" | "happy" | "sleeping" | "error";

const PALETTE: Record<BotKind, { light: string; base: string; dark: string }> = {
  clover: { light: "#bfe0ff", base: "#5aa4ec", dark: "#2f6fc0" },
  flower: { light: "#b8f7d4", base: "#25cf7c", dark: "#0f8f52" },
  ghost: { light: "#ffffff", base: "#dfe5f2", dark: "#a9b4cc" },
  square: { light: "#ffc2ee", base: "#e44fc6", dark: "#a2258b" },
};

function Body({ kind, fill }: { kind: BotKind; fill: string }) {
  switch (kind) {
    case "clover":
      return (
        <g fill={fill}>
          <circle cx="50" cy="31" r="21" />
          <circle cx="31" cy="51" r="21" />
          <circle cx="69" cy="51" r="21" />
          <circle cx="50" cy="69" r="21" />
          <circle cx="50" cy="50" r="24" />
        </g>
      );
    case "flower":
      return (
        <g fill={fill}>
          {[0, 72, 144, 216, 288].map((a) => {
            const r = (a - 90) * (Math.PI / 180);
            return <circle key={a} cx={50 + Math.cos(r) * 20} cy={52 + Math.sin(r) * 20} r="18" />;
          })}
          <circle cx="50" cy="52" r="24" />
        </g>
      );
    case "ghost":
      return (
        <path
          fill={fill}
          d="M22 50c0-18 12.5-32 28-32s28 14 28 32v30c0 2.5-2.6 3.8-4.6 2.3l-4.3-3.3a3 3 0 0 0-3.6 0l-5.4 4a3 3 0 0 1-3.6 0l-5.4-4a3 3 0 0 0-3.6 0l-5.4 4a3 3 0 0 1-3.6 0l-5.4-4a3 3 0 0 0-3.6 0l-4.3 3.3c-2 1.5-4.6.2-4.6-2.3z"
        />
      );
    case "square":
      return <rect x="20" y="20" width="60" height="62" rx="20" fill={fill} />;
  }
}

function Eyes({ mood, kind }: { mood: BotMood; kind: BotKind }) {
  const y = kind === "ghost" ? 47 : 50;
  const ink = kind === "ghost" ? "#1d2440" : "#14213d";
  if (mood === "happy") {
    return (
      <g fill="none" stroke={ink} strokeWidth="3.4" strokeLinecap="round">
        <path d={`M35 ${y + 2} q5 -7 10 0`} />
        <path d={`M55 ${y + 2} q5 -7 10 0`} />
      </g>
    );
  }
  if (mood === "sleeping") {
    return (
      <g fill="none" stroke={ink} strokeWidth="3" strokeLinecap="round">
        <path d={`M35 ${y} q5 4 10 0`} />
        <path d={`M55 ${y} q5 4 10 0`} />
      </g>
    );
  }
  if (mood === "error") {
    return (
      <g fill="none" stroke={ink} strokeWidth="3" strokeLinecap="round">
        <path d={`M36 ${y - 3} l8 6 M44 ${y - 3} l-8 6`} />
        <path d={`M56 ${y - 3} l8 6 M64 ${y - 3} l-8 6`} />
      </g>
    );
  }
  // Eyes carry the emotion: wide when listening, narrowed and set when working, lifted
  // when thinking (they also drift up-left via bot-look), plain otherwise.
  const r = mood === "listening" ? 6.6 : mood === "working" ? 4.9 : 5.6;
  const rx = r * (mood === "working" ? 1 : 0.82);
  const ey = mood === "thinking" ? y - 1.5 : y;
  return (
    <>
      <g className="bot-eyes" fill={ink}>
        <ellipse cx="40" cy={ey} rx={rx} ry={r} />
        <ellipse cx="60" cy={ey} rx={rx} ry={r} />
        <circle cx={40 + 1.6} cy={ey - 2} r="1.4" fill="#fff" opacity="0.85" />
        <circle cx={60 + 1.6} cy={ey - 2} r="1.4" fill="#fff" opacity="0.85" />
      </g>
      <Mouth mood={mood} ink={ink} y={y} />
    </>
  );
}

// A mouth only when there is a feeling to show; at rest the character stays the logo.
function Mouth({ mood, ink, y }: { mood: BotMood; ink: string; y: number }) {
  const common = { fill: "none", stroke: ink, strokeWidth: 2.6, strokeLinecap: "round" as const, opacity: 0.78 };
  const my = y + 13;
  switch (mood) {
    case "listening": return <path className="bot-mouth" d={`M45 ${my} q5 3 10 0`} {...common} />;
    case "working": return <path className="bot-mouth" d={`M45.5 ${my + 0.5} h9`} {...common} />;
    case "thinking": return <circle className="bot-mouth" cx="51" cy={my + 1} r="2.1" {...common} />;
    case "happy": return <path className="bot-mouth" d={`M43 ${my - 1} q7 7 14 0`} {...common} strokeWidth={2.8} />;
    case "error": return <path className="bot-mouth" d={`M43 ${my + 1} q3.5 -3 7 0 t7 0`} {...common} />;
    default: return null;
  }
}

export function BotAvatar({
  kind = "clover",
  mood = "idle",
  size = 32,
  className,
  title,
}: {
  kind?: BotKind;
  mood?: BotMood;
  size?: number;
  className?: string;
  title?: string;
}) {
  const uid = useId().replace(/:/g, "");
  const c = PALETTE[kind];
  // The felt texture only reads at larger sizes; at chip size it just blurs.
  const fuzzy = size >= 28;
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || size < 20 || mood === "still") return;
    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    gazeTargets.add(el);
    bindGaze();
    return () => {
      gazeTargets.delete(el);
    };
  }, [size, mood]);
  return (
    <span
      ref={ref}
      className={clsx("bot inline-flex flex-shrink-0", `bot-${mood}`, className)}
      style={{ width: size, height: size }}
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      title={title}
    >
      <svg viewBox="0 0 100 100" width={size} height={size} overflow="visible">
        <defs>
          <radialGradient id={`shade-${uid}`} cx="36%" cy="30%" r="75%">
            <stop offset="0%" stopColor={c.light} />
            <stop offset="45%" stopColor={c.base} />
            <stop offset="100%" stopColor={c.dark} />
          </radialGradient>
          <filter id={`felt-${uid}`} x="-15%" y="-15%" width="130%" height="130%">
            <feTurbulence type="fractalNoise" baseFrequency="1.6" numOctaves="2" seed="3" result="noise" />
            <feDisplacementMap in="SourceGraphic" in2="noise" scale="3.2" xChannelSelector="R" yChannelSelector="G" />
          </filter>
          <filter id={`shadow-${uid}`} x="-30%" y="-30%" width="160%" height="170%">
            <feDropShadow dx="0" dy="2" stdDeviation="2.5" floodColor={c.dark} floodOpacity="0.2" />
          </filter>
        </defs>
        {/* A grounded shadow, outside the body group: it stays on the floor and shrinks
            as the body lifts, instead of jumping with it. */}
        <ellipse className="bot-ground" cx="50" cy={kind === "ghost" ? 90 : 92} rx="22" ry="4.5" fill={c.dark} opacity="0.22" />
        <g className="bot-body" filter={`url(#shadow-${uid})`}>
          <g filter={fuzzy ? `url(#felt-${uid})` : undefined}>
            <Body kind={kind} fill={`url(#shade-${uid})`} />
          </g>
          <ellipse cx="38" cy="30" rx="10" ry="6" fill="#fff" opacity="0.28" transform="rotate(-25 38 30)" />
          <Eyes mood={mood} kind={kind} />
        </g>
      </svg>
    </span>
  );
}
