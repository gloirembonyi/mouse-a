import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import clsx from "clsx";
import {
  ArrowUp,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Copy,
  Ellipsis,
  MessageSquareText,
  Play,
  RotateCw,
  Sparkles,
  Square,
  WandSparkles,
  X,
  Zap,
} from "lucide-react";
import { emitTo } from "@tauri-apps/api/event";
import { Markdown } from "../components/chat/Markdown";
import { BotAvatar } from "../components/ui/BotAvatar";
import { BrandMark } from "../components/ui/primitives";
import { ACTION_LABELS, AUTO_ANSWER_KEY, looksLikeQuestion, useMeeting } from "../hooks/useMeeting";
import {
  copilotFocus,
  copilotHide,
  copilotResize,
  meetingAssist,
  onCopilotAssist,
  onStealthChanged,
  showMainWindow,
  stealthGet,
  stealthSet,
  type MeetingAction,
} from "../lib/tauri";

// The in-call copilot bar: a floating pill over the call that unfolds into
// a glass panel with the latest answer, a live read of the conversation and
// one-click actions. It shares the transcript and every model call with the
// main window's MeetingView through meeting.rs - only its cards are local.

const BAR_WIDTH = 680;
/** Room around the glass for its drop shadow inside the transparent window. */
const SHADOW_PAD = 28;
/** Live insights refresh once this much new conversation has come in… */
const INSIGHT_MIN_CHARS = 220;
/** …and never more often than this. */
const INSIGHT_MIN_GAP_MS = 20_000;

const ACTIONS: { action: MeetingAction; label: string; icon: React.ReactNode }[] = [
  { action: "assist", label: "Assist", icon: <Sparkles size={14} /> },
  { action: "suggest", label: "What should I say?", icon: <WandSparkles size={14} /> },
  { action: "followups", label: "Follow-up questions", icon: <MessageSquareText size={14} /> },
  { action: "summary", label: "Recap", icon: <RotateCw size={14} /> },
];

const LABEL_OVERRIDES: Partial<Record<MeetingAction, string>> = { summary: "Recap" };

/** Phases shown while an answer is on its way - mirrors the extension's orb labels. */
const PHASES = ["Catching the last words", "Reading the room", "Thinking", "Writing"];

const LS = {
  insights: "mouse-ai-copilot-insights",
  expanded: "mouse-ai-copilot-expanded",
};

export function CopilotBar() {
  const meeting = useMeeting();
  const { status, transcript, cards, levels, transcribing } = meeting;
  const live = status.live;

  const [expanded, setExpanded] = useState(() => localStorage.getItem(LS.expanded) !== "off");
  const [index, setIndex] = useState(0); // 0 = newest card
  const [draft, setDraft] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const [stealth, setStealth] = useState(true);
  const [insightsOn, setInsightsOn] = useState(() => localStorage.getItem(LS.insights) !== "off");
  const [insight, setInsight] = useState<{ text: string; at: number } | null>(null);
  const [insightBusy, setInsightBusy] = useState(false);
  const [autoAnswer, setAutoAnswer] = useState(() => localStorage.getItem(AUTO_ANSWER_KEY) === "on");

  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // ── Window sizing: the transparent window hugs the glass ──
  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const fit = () => void copilotResize(BAR_WIDTH, Math.ceil(el.getBoundingClientRect().height) + SHADOW_PAD);
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    localStorage.setItem(LS.expanded, expanded ? "on" : "off");
  }, [expanded]);

  useEffect(() => {
    stealthGet().then(setStealth).catch(() => {});
    const un = onStealthChanged(setStealth);
    // Settings written by the main window (same origin → same localStorage).
    const onStorage = (e: StorageEvent) => {
      if (e.key === AUTO_ANSWER_KEY) setAutoAnswer(e.newValue === "on");
    };
    window.addEventListener("storage", onStorage);
    return () => {
      un.then((fn) => fn());
      window.removeEventListener("storage", onStorage);
    };
  }, []);

  const { assist } = meeting;
  const run = useCallback(
    (action: MeetingAction, question: string | null = null) => {
      setExpanded(true);
      setIndex(0);
      void assist(action, question);
    },
    [assist],
  );

  // Ctrl+Shift+Enter (global, routed here by lib.rs while a call is live).
  const runRef = useRef(run);
  runRef.current = run;
  useEffect(() => {
    const un = onCopilotAssist((action) => runRef.current(action || "suggest"));
    return () => void un.then((fn) => fn());
  }, []);

  // ── Auto-answer: the other side asked something → draft a reply ──
  const answeredRef = useRef(0);
  const cardsRef = useRef(cards);
  cardsRef.current = cards;
  useEffect(() => {
    if (!autoAnswer || !live || status.mode === "practice") return;
    const last = transcript[transcript.length - 1];
    if (!last || last.speaker !== "them" || last.id <= answeredRef.current || !looksLikeQuestion(last.text)) return;
    const t = setTimeout(() => {
      if (cardsRef.current.some((c) => c.pending)) return;
      answeredRef.current = last.id;
      runRef.current("suggest");
    }, 600);
    return () => clearTimeout(t);
  }, [transcript, autoAnswer, live, status.mode]);

  // ── Live insights: a rolling read of the call as it grows ──
  const insightMark = useRef({ chars: 0, at: 0 });
  const insightBusyRef = useRef(false);
  const spokenChars = transcript.reduce((n, e) => (e.speaker === "them" || e.speaker === "me" ? n + e.text.length : n), 0);
  useEffect(() => {
    if (!insightsOn || !live || status.mode === "practice" || insightBusyRef.current) return;
    const mark = insightMark.current;
    if (spokenChars - mark.chars < INSIGHT_MIN_CHARS) return;
    const wait = Math.max(0, mark.at + INSIGHT_MIN_GAP_MS - Date.now());
    const t = setTimeout(async () => {
      if (insightBusyRef.current) return;
      insightBusyRef.current = true;
      setInsightBusy(true);
      insightMark.current = { chars: spokenChars, at: Date.now() };
      try {
        const text = await meetingAssist("insights", null, false);
        setInsight({ text, at: Date.now() });
      } catch {
        // A missed refresh is fine - the next one catches up.
      } finally {
        insightBusyRef.current = false;
        setInsightBusy(false);
      }
    }, wait);
    return () => clearTimeout(t);
  }, [spokenChars, insightsOn, live, status.mode]);

  // A new session starts with a clean read.
  useEffect(() => {
    if (live) return;
    insightMark.current = { chars: 0, at: 0 };
  }, [live]);
  useEffect(() => {
    if (live && transcript.length === 0) setInsight(null);
  }, [live, transcript.length]);

  const send = () => {
    const q = draft.trim();
    if (!q) return run("assist");
    setDraft("");
    run("ask", q);
  };

  const card = cards[Math.min(index, Math.max(cards.length - 1, 0))];
  const lastHeard = [...transcript].reverse().find((e) => e.speaker === "them" || e.speaker === "me");
  const speaking = Math.max(levels.mic, levels.system) > 0.15;
  const busy = cards.some((c) => c.pending) || insightBusy;

  return (
    <div ref={rootRef} className="copilot-root" onKeyDown={(e) => e.key === "Escape" && setExpanded(false)}>
      {/* ── The pill ── */}
      <div className="cp-pill glass" data-tauri-drag-region>
        <button
          type="button"
          className="cp-avatar"
          style={{ "--lvl": live ? Math.max(levels.mic, levels.system) : 0 } as React.CSSProperties}
          onClick={() => setExpanded((v) => !v)}
          title={live ? "Listening - click to show or hide" : "Mouse AI copilot"}
        >
          {live ? (
            <BotAvatar kind="clover" mood={busy ? "thinking" : speaking ? "listening" : "idle"} size={28} />
          ) : (
            <BrandMark size={28} mood="idle" />
          )}
        </button>

        <button type="button" className="cp-chip" onClick={() => setExpanded((v) => !v)}>
          <ChevronDown size={15} className={clsx("transition-transform duration-300", !expanded && "rotate-180")} />
          {expanded ? "Hide" : "Show"}
        </button>

        {live ? (
          <>
            <LiveClock startedAt={status.started_at_ms} />
            <button
              type="button"
              className="cp-stop"
              onClick={() => void meeting.stop()}
              title="End listening"
              aria-label="End listening"
            >
              <Square size={12} fill="currentColor" />
            </button>
            <button
              type="button"
              className="cp-icon"
              onClick={() => void copilotHide()}
              title="Hide bar - keeps listening (Ctrl+Shift+\ brings it back)"
              aria-label="Hide bar"
            >
              <X size={15} />
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="cp-chip cp-chip-accent"
              disabled={meeting.starting}
              onClick={() => void meeting.start("meeting", true, true)}
              title="Listen to this call (your mic + the other side)"
            >
              <Play size={12} fill="currentColor" />
              {meeting.starting ? "Starting…" : "Listen"}
            </button>
            <button
              type="button"
              className="cp-icon"
              onClick={() => void copilotHide()}
              title="Close bar (Ctrl+Shift+\)"
              aria-label="Close bar"
            >
              <X size={15} />
            </button>
          </>
        )}
        {(transcribing || busy) && <span className="cp-scan" aria-hidden />}
      </div>

      {/* ── The panel ── */}
      {expanded && (
        <div className="cp-panel glass" onMouseDown={() => void copilotFocus()}>
          {(meeting.error || status.warning) && (
            <div className="cp-warning" role="status">
              {meeting.error || status.warning}
            </div>
          )}

          <Insight
            live={live}
            on={insightsOn && status.mode !== "practice"}
            insight={insight}
            busy={insightBusy}
            lastHeard={lastHeard ? `${lastHeard.speaker === "them" ? "Them" : "You"}: ${lastHeard.text}` : null}
          />

          {card ? (
            <div className="cp-card">
              <div className="cp-card-head">
                <span className="cp-bubble">{card.action === "ask" ? card.label : LABEL_OVERRIDES[card.action] ?? ACTION_LABELS[card.action]}</span>
                {cards.length > 1 && (
                  <div className="cp-nav">
                    <button type="button" disabled={index >= cards.length - 1} onClick={() => setIndex((i) => i + 1)} aria-label="Older answer">
                      <ChevronLeft size={14} />
                    </button>
                    <span className="tabular-nums">
                      {cards.length - index}/{cards.length}
                    </span>
                    <button type="button" disabled={index === 0} onClick={() => setIndex((i) => i - 1)} aria-label="Newer answer">
                      <ChevronRight size={14} />
                    </button>
                  </div>
                )}
              </div>
              {card.pending ? (
                <Thinking />
              ) : card.error ? (
                <p className="cp-error">
                  {card.error}{" "}
                  <button type="button" onClick={() => run(card.action, card.action === "ask" ? card.label : null)}>
                    Try again
                  </button>
                </p>
              ) : (
                <div className="cp-answer">
                  <Markdown>{card.text}</Markdown>
                  <CopyButton text={card.text} />
                </div>
              )}
            </div>
          ) : (
            !insight && (
              <p className="cp-empty">
                {live
                  ? "I'm listening. Pick an action, ask anything, or press Ctrl+Shift+Enter when you're put on the spot."
                  : "Press Listen when the call starts. I hear both sides and stay out of your screen share."}
              </p>
            )
          )}

          <div className="cp-actions">
            {ACTIONS.map((a, i) => (
              <span key={a.action} className="contents">
                {i > 0 && <span className="cp-dot" aria-hidden />}
                <button type="button" onClick={() => run(a.action)} disabled={cards[0]?.pending}>
                  {a.icon}
                  {a.label}
                </button>
              </span>
            ))}
          </div>

          <div className="cp-input">
            <textarea
              ref={inputRef}
              rows={1}
              value={draft}
              placeholder="Ask about your screen or conversation, or Ctrl ↵ for Assist"
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                  e.preventDefault();
                  run("assist", null);
                } else if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  send();
                }
              }}
            />
            <div className="cp-input-row">
              <button
                type="button"
                className={clsx("cp-toggle", meeting.includeScreen && "is-on")}
                onClick={() => meeting.setIncludeScreen(!meeting.includeScreen)}
                title="Smart: also look at your screen (slides, a shared doc, a coding question)"
                aria-pressed={meeting.includeScreen}
              >
                <Zap size={13} />
                Smart
              </button>
              <div className="relative">
                <button type="button" className="cp-icon" onClick={() => setMenuOpen((v) => !v)} aria-label="More options">
                  <Ellipsis size={16} />
                </button>
                {menuOpen && (
                  <MoreMenu
                    onClose={() => setMenuOpen(false)}
                    stealth={stealth}
                    onStealth={(v) => void stealthSet(v).then(setStealth)}
                    autoAnswer={autoAnswer}
                    onAutoAnswer={(v) => {
                      localStorage.setItem(AUTO_ANSWER_KEY, v ? "on" : "off");
                      setAutoAnswer(v);
                    }}
                    insights={insightsOn}
                    onInsights={(v) => {
                      localStorage.setItem(LS.insights, v ? "on" : "off");
                      setInsightsOn(v);
                    }}
                    onOpenFull={() => {
                      void showMainWindow();
                      void emitTo("main", "open-meeting-view");
                    }}
                  />
                )}
              </div>
              {stealth && <span className="cp-stealth">Hidden from screen share</span>}
              <button
                type="button"
                className="cp-send"
                onClick={send}
                disabled={cards[0]?.pending}
                title={draft.trim() ? "Ask (Enter)" : "Assist (Ctrl+Enter)"}
                aria-label="Send"
              >
                <ArrowUp size={16} strokeWidth={2.4} />
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Pieces ───────────────────────────────────────────────────────

function Insight({
  live,
  on,
  insight,
  busy,
  lastHeard,
}: {
  live: boolean;
  on: boolean;
  insight: { text: string; at: number } | null;
  busy: boolean;
  lastHeard: string | null;
}) {
  if (!live && !insight) return null;
  return (
    <section className="cp-insight" aria-live="polite">
      <div className="cp-insight-head">
        <span className={clsx("cp-live-dot", live && "is-live")} />
        <span>{live ? "Live" : "Last call"}</span>
        {busy && <span className="cp-insight-busy">updating…</span>}
      </div>
      {on && insight ? (
        <Markdown className="cp-insight-body">{insight.text}</Markdown>
      ) : (
        lastHeard && <p className="cp-caption">{lastHeard}</p>
      )}
    </section>
  );
}

function Thinking() {
  const [phase, setPhase] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setPhase((p) => Math.min(p + 1, PHASES.length - 1)), 1400);
    return () => clearInterval(t);
  }, []);
  return (
    <div className="cp-thinking" role="status">
      <span className="cp-phase" key={phase}>
        {PHASES[phase]}…
      </span>
      <span className="cp-skel" style={{ width: "92%" }} />
      <span className="cp-skel" style={{ width: "78%" }} />
      <span className="cp-skel" style={{ width: "54%" }} />
    </div>
  );
}

function CopyButton({ text }: { text: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      className="cp-copy"
      onClick={() => {
        void navigator.clipboard.writeText(text);
        setDone(true);
        setTimeout(() => setDone(false), 1200);
      }}
      aria-label="Copy answer"
      title="Copy"
    >
      {done ? <Check size={13} /> : <Copy size={13} />}
    </button>
  );
}

function MoreMenu(props: {
  onClose: () => void;
  stealth: boolean;
  onStealth: (v: boolean) => void;
  autoAnswer: boolean;
  onAutoAnswer: (v: boolean) => void;
  insights: boolean;
  onInsights: (v: boolean) => void;
  onOpenFull: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) props.onClose();
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [props]);
  return (
    <div ref={ref} className="cp-menu glass" role="menu">
      <MenuSwitch label="Hide from screen share" hint="Invisible in shares & recordings" on={props.stealth} set={props.onStealth} />
      <MenuSwitch label="Auto-answer questions" hint="Drafts a reply when you're asked" on={props.autoAnswer} set={props.onAutoAnswer} />
      <MenuSwitch label="Live insights" hint="Topic, key points, what to say next" on={props.insights} set={props.onInsights} />
      <button
        type="button"
        className="cp-menu-item"
        onClick={() => {
          props.onOpenFull();
          props.onClose();
        }}
      >
        Open full copilot (transcript & notes)
      </button>
    </div>
  );
}

function MenuSwitch({ label, hint, on, set }: { label: string; hint: string; on: boolean; set: (v: boolean) => void }) {
  return (
    <button type="button" role="menuitemcheckbox" aria-checked={on} className="cp-menu-item" onClick={() => set(!on)}>
      <span className="flex flex-col items-start">
        <span>{label}</span>
        <span className="cp-menu-hint">{hint}</span>
      </span>
      <span className={clsx("cp-switch", on && "is-on")} aria-hidden>
        <span />
      </span>
    </button>
  );
}

function LiveClock({ startedAt }: { startedAt: number | null }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const secs = startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0;
  const mm = String(Math.floor(secs / 60)).padStart(2, "0");
  const ss = String(secs % 60).padStart(2, "0");
  return (
    <span className="cp-clock">
      <span className="cp-live-dot is-live" />
      {mm}:{ss}
    </span>
  );
}
