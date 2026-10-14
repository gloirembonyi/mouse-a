import { useEffect, useId, useRef } from "react";
import clsx from "clsx";

// The aura: one fluid iridescent wash (teal -> electric blue -> violet -> magenta) that
// circles every character. Its speed and brightness ARE the state - a slow drift at
// rest, a quicker swirl while thinking, a bright spin while working, a breath in time
// with the voice while listening (index.css, .bot-aura).
const IRIS = ["#36e3c9", "#4f8bff", "#9b6cff", "#ec4ad7", "#36e3c9"];

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
function bindGaze() {
  if (gazeBound || typeof document === "undefined") return;
  gazeBound = true;
  document.addEventListener(
    "pointermove",
    (e) => {
      gazeX = e.clientX;
      gazeY = e.clientY;
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
  const r = mood === "listening" ? 6.4 : 5.6;
  return (
    <g className="bot-eyes" fill={ink}>
      <ellipse cx="40" cy={y} rx={r * 0.82} ry={r} />
      <ellipse cx="60" cy={y} rx={r * 0.82} ry={r} />
      <circle cx={40 + 1.6} cy={y - 2} r="1.4" fill="#fff" opacity="0.85" />
      <circle cx={60 + 1.6} cy={y - 2} r="1.4" fill="#fff" opacity="0.85" />
    </g>
  );
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
            <feDropShadow dx="0" dy="4" stdDeviation="4" floodColor={c.dark} floodOpacity="0.35" />
          </filter>
          <linearGradient id={`iris-${uid}`} gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="100" y2="100">
            {IRIS.map((col, i) => (
              <stop key={i} offset={`${(i / (IRIS.length - 1)) * 100}%`} stopColor={col} />
            ))}
          </linearGradient>
          {fuzzy && (
            <filter id={`blur-${uid}`} x="-40%" y="-40%" width="180%" height="180%">
              <feGaussianBlur stdDeviation="3.5" />
            </filter>
          )}
        </defs>
        <g className="bot-aura" style={{ pointerEvents: "none" }}>
          {fuzzy && (
            <circle cx="50" cy="55" r="43" fill="none" stroke={`url(#iris-${uid})`} strokeWidth="9" opacity="0.45" filter={`url(#blur-${uid})`} />
          )}
          <circle
            cx="50"
            cy="55"
            r="43"
            fill="none"
            stroke={`url(#iris-${uid})`}
            strokeWidth={fuzzy ? 3.2 : 4}
            strokeLinecap="round"
            strokeDasharray={fuzzy ? "210 60" : "240 30"}
          />
        </g>
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
