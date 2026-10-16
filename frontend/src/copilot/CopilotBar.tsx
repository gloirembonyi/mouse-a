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
  Globe,
  MessageSquareText,
  NotebookPen,
  Pause,
  Play,
  RotateCw,
  ScanText,
  Sparkles,
  Square,
  WandSparkles,
  Wrench,
  X,
  Zap,
} from "lucide-react";
import { emitTo } from "@tauri-apps/api/event";
import { Markdown } from "../components/chat/Markdown";
import { BotAvatar } from "../components/ui/BotAvatar";
import { BrandMark } from "../components/ui/primitives";
import { AgentPanel, CuesView, InsertBar } from "./CopilotTools";
import { useCopilotAgent, useInsert } from "./copilotHooks";
import { ACTION_LABELS, AUTO_ANSWER_KEY, lastTheirTurn, looksLikeQuestion, useMeeting, type AssistCard } from "../hooks/useMeeting";
import {
  copilotFocus,
  copilotHide,
  copilotResize,
  meetingAssist,
  onCopilotAssist,
  onCopilotContext,
  onStealthChanged,
  type CopilotContext,
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
  { action: "screen", label: "Solve screen", icon: <ScanText size={14} /> },
  { action: "followups", label: "Follow-ups", icon: <MessageSquareText size={14} /> },
  { action: "summary", label: "Recap", icon: <RotateCw size={14} /> },
];

const LABEL_OVERRIDES: Partial<Record<MeetingAction, string>> = { summary: "Recap" };

/** Phases shown while an answer is on its way - mirrors the extension's orb labels. */
const PHASES = ["Catching the last words", "Reading the room", "Thinking", "Writing"];
const SCREEN_PHASES = ["Capturing your screen", "Reading what's on it", "Working it out", "Writing"];

const LS = {
  insights: "mouse-ai-copilot-insights",
  expanded: "mouse-ai-copilot-expanded",
  opacity: "mouse-ai-copilot-opacity",
};

export function CopilotBar() {
  const meeting = useMeeting();
  const { status, transcript, cards, levels, transcribing, partials } = meeting;
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
  /** "do" = the input goes to the desktop agent (files, terminal, browser) instead of a chat answer. */
  const [doMode, setDoMode] = useState(false);
  const [view, setView] = useState<"answers" | "cues">("answers");
  const [opacity, setOpacity] = useState(() => Number(localStorage.getItem(LS.opacity)) || 1);
  /** A page handed over from the browser extension (Kudu Cloak → Kudu Desktop). */
  const [pageCtx, setPageCtx] = useState<CopilotContext | null>(null);
  const agent = useCopilotAgent();
  const ins = useInsert();

  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // ── Window sizing: the transparent window hugs the glass ──
  // The "..." menu is absolutely positioned (it opens below the input row), so it
  // is measured too - otherwise the window edge cut it off.
  const fitRef = useRef<() => void>(() => {});
  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const fit = () => {
      const r = el.getBoundingClientRect();
      const menu = el.querySelector<HTMLElement>(".cp-menu");
      const bottom = Math.max(r.bottom, menu ? menu.getBoundingClientRect().bottom + 8 : 0);
      void copilotResize(BAR_WIDTH, Math.ceil(bottom - r.top) + SHADOW_PAD);
    };
    fitRef.current = fit;
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  useLayoutEffect(() => {
    fitRef.current();
  }, [menuOpen, pageCtx]);

  // Kudu Cloak from the browser: the extension sends the page; the bar opens on it.
  useEffect(() => {
    const un = onCopilotContext((c) => {
      setPageCtx(c);
      setExpanded(true);
      setView("answers");
      setDoMode(false);
      void copilotFocus();
      setTimeout(() => inputRef.current?.focus(), 80);
    });
    return () => void un.then((fn) => fn());
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
      setView("answers");
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
  // Two triggers. The fast one watches the sentence they are STILL saying (the live
  // partial): once it reads as a question and has not changed for 700 ms, the answer
  // starts before they finish. The final transcript line then arrives and is matched
  // against what was already answered, so one question never gets two answers.
  const answeredRef = useRef(0);
  const earlyRef = useRef<{ text: string; at: number } | null>(null);
  const cardsRef = useRef(cards);
  cardsRef.current = cards;
  const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9 ]+/g, " ").replace(/\s+/g, " ").trim();
  const sameAsk = (a: string, b: string) => {
    const x = norm(a);
    const y = norm(b);
    if (!x || !y) return false;
    const head = Math.min(28, x.length, y.length);
    return x.slice(0, head) === y.slice(0, head) || x.includes(y.slice(0, 40)) || y.includes(x.slice(0, 40));
  };
  useEffect(() => {
    if (!autoAnswer || !live || status.mode === "practice") return;
    const text = partials.them;
    if (!text || text.length < 20 || !looksLikeQuestion(text)) return;
    const early = earlyRef.current;
    if (early && Date.now() - early.at < 20_000 && sameAsk(early.text, text)) return;
    const t = setTimeout(() => {
      if (cardsRef.current.some((c) => c.pending)) return;
      earlyRef.current = { text, at: Date.now() };
      runRef.current("suggest");
    }, 700);
    return () => clearTimeout(t);
  }, [partials.them, autoAnswer, live, status.mode]);
  useEffect(() => {
    if (!autoAnswer || !live || status.mode === "practice") return;
    const turn = lastTheirTurn(transcript);
    if (!turn || turn.id <= answeredRef.current || !looksLikeQuestion(turn.text)) return;
    const early = earlyRef.current;
    if (early && Date.now() - early.at < 20_000 && sameAsk(early.text, turn.text)) {
      answeredRef.current = turn.id; // already answered from the live partial
      return;
    }
    const t = setTimeout(() => {
      if (cardsRef.current.some((c) => c.pending)) return;
      answeredRef.current = turn.id;
      runRef.current("suggest");
    }, 250);
    return () => clearTimeout(t);
  }, [transcript, autoAnswer, live, status.mode]);

  // With auto-answer off (or a question it did not catch), one tap answers it.
  const pendingAsk = (() => {
    if (!live) return null;
    const turn = lastTheirTurn(transcript);
    if (!turn || turn.id <= answeredRef.current || !looksLikeQuestion(turn.text)) return null;
    if (cards.some((c) => c.pending)) return null;
    return turn;
  })();

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
        const { text } = await meetingAssist("insights", null, false);
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

  const fix = (task: string) => {
    setExpanded(true);
    setView("answers");
    void agent.start(task);
  };

  /** The browser page rides along with the question while the chip is attached. */
  const withPage = (q: string) =>
    pageCtx
      ? `[Page from the browser: ${pageCtx.title || "untitled"} (${pageCtx.url})]\n` +
        (pageCtx.selection ? `Selected text:\n${pageCtx.selection}\n\n` : "") +
        `${pageCtx.text}\n\nQuestion: ${q}`
      : q;

  const send = () => {
    const q = draft.trim();
    if (doMode) {
      setDraft("");
      return fix(withPage(q));
    }
    if (!q && !pageCtx) return run("assist");
    setDraft("");
    run("ask", withPage(q || "What should I know about this page? Keep it short."));
  };

  const card = cards[Math.min(index, Math.max(cards.length - 1, 0))];
  const lastHeard = [...transcript].reverse().find((e) => e.speaker === "them" || e.speaker === "me");
  const speaking = Math.max(levels.mic, levels.system) > 0.15;
  const busy = cards.some((c) => c.pending) || insightBusy;

  return (
    <div
      ref={rootRef}
      className="copilot-root"
      style={{ opacity }}
      onKeyDown={(e) => e.key === "Escape" && setExpanded(false)}
    >
      {/* ── The pill ── */}
      <div className={clsx("cp-pill glass", busy && "is-busy")} data-tauri-drag-region>
        <button
          type="button"
          className="cp-avatar"
          style={{ "--lvl": live ? Math.max(levels.mic, levels.system) : 0 } as React.CSSProperties}
          onClick={() => setExpanded((v) => !v)}
          title={live ? "Listening - click to show or hide" : "Kudu Pilot"}
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

        {ins.busy && (
          <button type="button" className="cp-chip cp-typing" onClick={ins.cancel} title="Stop inserting">
            <Square size={10} fill="currentColor" />
            {ins.busy.mode === "type" ? "Typing" : "Pasting"} {ins.busy.label}…
          </button>
        )}
        {agent.run?.running && (
          <span className="cp-chip cp-agent-chip" title={agent.run.task}>
            <Wrench size={12} />
            Agent working…
          </span>
        )}
        {live ? (
          <>
            <LiveClock startedAt={status.started_at_ms} />
            <button
              type="button"
              className="cp-stop"
              onClick={() => void meeting.stop()}
              title="Pause listening - the transcript stays, Resume continues it"
              aria-label="Pause listening"
            >
              <Pause size={12} fill="currentColor" />
            </button>
            <button
              type="button"
              className="cp-icon"
              onClick={() => {
                void (async () => {
                  await meeting.stop();
                  await copilotHide();
                })();
              }}
              title="Stop listening and close the bar (Ctrl+Shift+\ brings it back)"
              aria-label="Stop and close"
            >
              <X size={15} />
            </button>
          </>
        ) : (
          <>
            {meeting.resumable ? (
              <>
                <button
                  type="button"
                  className="cp-chip cp-chip-accent"
                  disabled={meeting.starting}
                  onClick={() => void meeting.resume()}
                  title="Continue this meeting: same transcript, same clock"
                >
                  <Play size={12} fill="currentColor" />
                  {meeting.starting ? "Starting…" : "Resume"}
                </button>
                <button
                  type="button"
                  className="cp-chip"
                  disabled={meeting.starting}
                  onClick={() => void meeting.start("meeting", true, true)}
                  title="Start over with an empty transcript"
                >
                  New
                </button>
              </>
            ) : (
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
            )}
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
          {(meeting.error || status.warning || ins.error) && (
            <div className="cp-warning" role="status" onClick={ins.clearError}>
              {ins.error || meeting.error || status.warning}
            </div>
          )}

          {view === "cues" ? (
            <CuesView onInsert={ins.insert} />
          ) : (
            <>
              <AgentPanel agent={agent} onInsert={ins.insert} />

              <Insight
                live={live}
                on={insightsOn && status.mode !== "practice"}
                insight={insight}
                busy={insightBusy}
                lastHeard={
                  partials.them
                    ? `Them: ${partials.them}…`
                    : partials.me
                      ? `You: ${partials.me}…`
                      : lastHeard
                        ? `${lastHeard.speaker === "them" ? "Them" : "You"}: ${lastHeard.text}`
                        : null
                }
              />

              {pendingAsk && (
                <button
                  type="button"
                  className="cp-chip cp-chip-accent cp-answer-now"
                  onClick={() => {
                    answeredRef.current = pendingAsk.id;
                    run("suggest");
                  }}
                  title={pendingAsk.text}
                >
                  <WandSparkles size={13} />
                  Answer what they just asked
                </button>
              )}

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
                  {card.context && (
                    <p className="cp-context" title={card.context}>
                      <span>They asked</span>
                      {card.context}
                    </p>
                  )}
                  {card.pending && card.text ? (
                    <div className="cp-answer is-streaming">
                      <Markdown>{card.text}</Markdown>
                    </div>
                  ) : card.pending ? (
                    <Thinking card={card} />
                  ) : card.error ? (
                    <p className="cp-error">
                      {card.error}{" "}
                      <button type="button" onClick={() => run(card.action, card.action === "ask" ? card.label : null)}>
                        Try again
                      </button>
                    </p>
                  ) : (
                    <div className="cp-answer">
                      {card.screen && <ScreenThumb src={card.screen} />}
                      <Markdown>{card.text}</Markdown>
                      <CopyButton text={card.text} />
                      <InsertBar text={card.text} onInsert={ins.insert} />
                    </div>
                  )}
                </div>
              ) : (
                !insight &&
                !agent.run && (
                  <p className="cp-empty">
                    {live
                      ? "I'm listening. Ctrl+Shift+Enter when you're put on the spot, Ctrl+Shift+S to solve what's on screen, Ctrl+Alt+arrows to move me."
                      : "Press Listen when the call starts. I hear both sides, can read your screen, and stay out of your screen share."}
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
                <span className="cp-dot" aria-hidden />
                <button
                  type="button"
                  onClick={() => fix("")}
                  disabled={agent.run?.running}
                  title="The agent looks at your screen and project, finds the bug and fixes it - quietly, in the background"
                >
                  <Wrench size={14} />
                  Fix it
                </button>
              </div>
            </>
          )}

          <div className="cp-input">
            {pageCtx && (
              <div className="cp-ctx" title={pageCtx.url}>
                <Globe size={13} />
                <span className="cp-ctx-label">From your browser: {pageCtx.title || pageCtx.url}</span>
                <button type="button" className="cp-ctx-x" onClick={() => setPageCtx(null)} aria-label="Detach the page">
                  <X size={12} />
                </button>
              </div>
            )}
            <textarea
              ref={inputRef}
              rows={1}
              value={draft}
              placeholder={
                doMode
                  ? "Tell the agent what to do - find the bug, run the tests, check the docs… (files, terminal, browser)"
                  : "Ask about your screen or conversation, or Ctrl ↵ for Assist"
              }
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
              <button
                type="button"
                className={clsx("cp-toggle", doMode && "is-on")}
                onClick={() => setDoMode((v) => !v)}
                title="Do: send your message to the agent, which can read files, run commands and use the browser"
                aria-pressed={doMode}
              >
                <Wrench size={13} />
                Do
              </button>
              <button
                type="button"
                className={clsx("cp-toggle", view === "cues" && "is-on")}
                onClick={() => {
                  setExpanded(true);
                  setView((v) => (v === "cues" ? "answers" : "cues"));
                }}
                title="Cue cards: your private notes and code for a demo or lesson"
                aria-pressed={view === "cues"}
              >
                <NotebookPen size={13} />
                Cues
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
                    useContext={meeting.useContext}
                    onUseContext={(v) => void meeting.setUseContext(v)}
                    autoAnswer={autoAnswer}
                    onAutoAnswer={(v) => {
                      localStorage.setItem(AUTO_ANSWER_KEY, v ? "on" : "off");
                      setAutoAnswer(v);
                    }}
                    opacity={opacity}
                    onOpacity={(v) => {
                      localStorage.setItem(LS.opacity, String(v));
                      setOpacity(v);
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
                disabled={doMode ? agent.run?.running : cards[0]?.pending}
                title={doMode ? "Send to the agent (Enter)" : draft.trim() ? "Ask (Enter)" : "Assist (Ctrl+Enter)"}
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

function Thinking({ card }: { card: AssistCard }) {
  const phases = card.action === "screen" ? SCREEN_PHASES : PHASES;
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(t);
  }, []);
  const elapsed = Math.max(0, now - card.startedAt);
  const phase = Math.min(Math.floor(elapsed / 1400), phases.length - 1);
  return (
    <div className="cp-thinking" role="status">
      <span className="cp-phase-row">
        <span className="cp-phase" key={phase}>
          {phases[phase]}…
        </span>
        {elapsed > 2500 && <span className="cp-elapsed tabular-nums">{(elapsed / 1000).toFixed(0)}s</span>}
      </span>
      <span className="cp-skel" style={{ width: "92%" }} />
      <span className="cp-skel" style={{ width: "78%" }} />
      <span className="cp-skel" style={{ width: "54%" }} />
    </div>
  );
}

/** What the copilot looked at - click to see it larger. */
function ScreenThumb({ src }: { src: string }) {
  const [big, setBig] = useState(false);
  return (
    <button
      type="button"
      className={clsx("cp-shot", big && "is-big")}
      onClick={() => setBig((v) => !v)}
      title={big ? "Shrink" : "The screen this answer is based on - click to enlarge"}
    >
      <img src={src} alt="Captured screen" draggable={false} />
      {!big && <span>Screen</span>}
    </button>
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
  useContext: boolean;
  onUseContext: (v: boolean) => void;
  autoAnswer: boolean;
  onAutoAnswer: (v: boolean) => void;
  insights: boolean;
  onInsights: (v: boolean) => void;
  opacity: number;
  onOpacity: (v: number) => void;
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
      <MenuSwitch label="Use my context" hint="About me, projects, this session, research brief" on={props.useContext} set={props.onUseContext} />
      <MenuSwitch label="Auto-answer questions" hint="Drafts a reply when you're asked" on={props.autoAnswer} set={props.onAutoAnswer} />
      <MenuSwitch label="Live insights" hint="Topic, key points, what to say next" on={props.insights} set={props.onInsights} />
      <label className="cp-menu-item cp-menu-range">
        <span className="flex flex-col items-start">
          <span>Opacity</span>
          <span className="cp-menu-hint">See the call through the bar</span>
        </span>
        <input
          type="range"
          min={0.45}
          max={1}
          step={0.05}
          value={props.opacity}
          onChange={(e) => props.onOpacity(Number(e.target.value))}
          aria-label="Bar opacity"
        />
      </label>
      <button
        type="button"
        className="cp-menu-item"
        onClick={() => {
          props.onOpenFull();
          props.onClose();
        }}
      >
        Open Kudu Pilot (transcript & notes)
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
