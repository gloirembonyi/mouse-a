import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import {
  AlertTriangle,
  ArrowUp,
  BookOpen,
  Briefcase,
  Check,
  Copy,
  FileText,
  GraduationCap,
  HelpCircle,
  Lightbulb,
  ListChecks,
  Lock,
  Mic,
  Monitor,
  Pause,
  Play,
  Save,
  ScanText,
  Sparkles,
  Trash2,
  UserCircle2,
  Users,
  Volume2,
  X,
} from "lucide-react";
import { ThinkingOrb } from "thinking-orbs";
import { Group, Panel, Separator } from "react-resizable-panels";
import { Markdown } from "../chat/Markdown";
import { IconButton, Kbd, Switch } from "../ui/primitives";
import { BotAvatar } from "../ui/BotAvatar";
import { copilotBarEnabled, looksLikeQuestion, setCopilotBarEnabled, type AssistCard, type useMeeting } from "../../hooks/useMeeting";
import { copilotShow, type MeetingAction, type MeetingMode, type MeetingProfile, type TranscriptEntry } from "../../lib/tauri";

interface MeetingViewProps {
  meeting: ReturnType<typeof useMeeting>;
  onClose: () => void;
}

const MODES: { id: MeetingMode; label: string; hint: string; icon: React.ReactNode }[] = [
  { id: "meeting", label: "Meeting", hint: "Live notes, summaries and replies in a work call", icon: <Users size={16} /> },
  { id: "interview", label: "Interview", hint: "Answers grounded in your real experience", icon: <Briefcase size={16} /> },
  { id: "practice", label: "Practice", hint: "Mock interview with honest feedback", icon: <GraduationCap size={16} /> },
];

const LIVE_ACTIONS: { action: MeetingAction; label: string; icon: React.ReactNode }[] = [
  { action: "suggest", label: "What should I say?", icon: <Sparkles size={13} /> },
  { action: "summary", label: "Summary", icon: <FileText size={13} /> },
  { action: "screen", label: "Solve my screen", icon: <ScanText size={13} /> },
  { action: "followups", label: "Follow-ups", icon: <HelpCircle size={13} /> },
  { action: "explain", label: "Explain terms", icon: <BookOpen size={13} /> },
];

const PRACTICE_ACTIONS: { action: MeetingAction; label: string; icon: React.ReactNode }[] = [
  { action: "practice_question", label: "Next question", icon: <HelpCircle size={13} /> },
  { action: "practice_feedback", label: "Feedback on my answer", icon: <Lightbulb size={13} /> },
  { action: "suggest", label: "Model answer", icon: <Sparkles size={13} /> },
];

type InputTarget = "ask" | "them" | "me" | "note";

export function MeetingView({ meeting, onClose }: MeetingViewProps) {
  const { status } = meeting;
  const [mode, setMode] = useState<MeetingMode>(status.live && status.mode ? status.mode : "meeting");
  const [mic, setMic] = useState(true);
  const [system, setSystem] = useState(true);
  const [contextOpen, setContextOpen] = useState(false);

  const activeMode: MeetingMode = status.live && status.mode ? status.mode : mode;
  const hasProfile = Boolean(meeting.profile.about.trim() || meeting.profile.projects.trim());
  const hasSession = status.live || meeting.transcript.length > 0 || meeting.cards.length > 0;

  // Practice is just you and the coach - there's no "other side" to capture.
  useEffect(() => {
    setSystem(mode !== "practice");
  }, [mode]);

  const start = () => void meeting.start(mode, mic, mode === "practice" ? false : system);

  return (
    <div className="relative flex h-full flex-col">
      {/* Header */}
      <div className="flex h-12 flex-shrink-0 items-center gap-3 border-b border-border-soft px-4">
        <BotAvatar
          kind="pilot"
          mood={status.live ? (meeting.levels.mic > 0.15 || meeting.levels.system > 0.15 ? "listening" : "idle") : "still"}
          size={26}
        />
        <h2 className="text-[14px] font-semibold">Kudu Pilot</h2>
        {status.live && <LiveClock startedAt={status.started_at_ms} />}
        <div className="ml-auto flex items-center gap-1.5">
          <button
            onClick={() => setContextOpen(true)}
            className={clsx(
              "flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[12.5px] transition hover:bg-bg-hover",
              hasProfile ? "text-text-secondary" : "text-warning",
            )}
            title="What Kudu Pilot knows about you"
          >
            <UserCircle2 size={14} />
            {hasProfile ? "My context" : "Add my context"}
          </button>
          {hasSession && !status.live && (
            <>
              {meeting.resumable && (
                <button
                  onClick={() => void meeting.resume()}
                  disabled={meeting.starting}
                  className="flex h-8 items-center gap-1.5 rounded-lg btn-brand px-3 text-[12.5px] font-medium disabled:opacity-50"
                  title="Continue this meeting: same transcript, same clock"
                >
                  <Play size={12} fill="currentColor" />
                  Resume
                </button>
              )}
              <button
                onClick={start}
                disabled={meeting.starting}
                className={clsx(
                  "flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12.5px] font-medium disabled:opacity-50",
                  meeting.resumable ? "border border-border-soft text-text-secondary hover:bg-bg-hover" : "btn-brand",
                )}
                title={meeting.resumable ? "Start over with an empty transcript" : "Start listening"}
              >
                {!meeting.resumable && <Play size={12} fill="currentColor" />}
                New session
              </button>
            </>
          )}
          {status.live && (
            <button
              onClick={() => void meeting.stop()}
              className="flex h-8 items-center gap-1.5 rounded-lg bg-danger px-3 text-[12.5px] font-medium text-white transition hover:opacity-90"
              title="Pause listening. The transcript stays; Resume picks up where you left off"
            >
              <Pause size={11} fill="currentColor" />
              Pause
            </button>
          )}
          <IconButton title="Back to chat" onClick={onClose}>
            <X size={16} />
          </IconButton>
        </div>
      </div>

      {(meeting.error || status.warning) && (
        <div className="mx-4 mt-3 flex items-start gap-2 rounded-lg border border-warning/30 bg-warning-soft px-3 py-2 text-[12px] text-warning">
          <AlertTriangle size={14} className="mt-px flex-shrink-0" />
          <span className="flex-1">{meeting.error || status.warning}</span>
          {meeting.error && (
            <button onClick={() => meeting.setError(null)} className="opacity-70 hover:opacity-100" aria-label="Dismiss">
              <X size={13} />
            </button>
          )}
        </div>
      )}

      {hasSession ? (
        <Group orientation="horizontal" className="min-h-0 flex-1">
          <Panel id="meeting-transcript" defaultSize="46%" minSize="28%" className="min-w-0">
            <TranscriptPane meeting={meeting} mode={activeMode} />
          </Panel>
          <Separator className="resize-handle resize-handle-v" />
          <Panel id="meeting-copilot" minSize="34%" className="min-w-0">
            <CopilotPane meeting={meeting} mode={activeMode} hasProfile={hasProfile} onEditContext={() => setContextOpen(true)} />
          </Panel>
        </Group>
      ) : (
        <SetupScreen
          mode={mode}
          onMode={setMode}
          mic={mic}
          onMic={setMic}
          system={system}
          onSystem={setSystem}
          hasProfile={hasProfile}
          onEditContext={() => setContextOpen(true)}
          starting={meeting.starting}
          onStart={start}
        />
      )}

      {contextOpen && (
        <ContextDrawer
          profile={meeting.profile}
          onSave={meeting.saveProfile}
          onResearch={meeting.research}
          researching={meeting.researching}
          onClose={() => setContextOpen(false)}
        />
      )}
    </div>
  );
}

// ── Setup ─────────────────────────────────────────────────────────

function SetupScreen(props: {
  mode: MeetingMode;
  onMode: (m: MeetingMode) => void;
  mic: boolean;
  onMic: (v: boolean) => void;
  system: boolean;
  onSystem: (v: boolean) => void;
  hasProfile: boolean;
  onEditContext: () => void;
  starting: boolean;
  onStart: () => void;
}) {
  const { mode } = props;
  return (
    <div className="scrollbar-thin flex min-h-0 flex-1 flex-col items-center overflow-y-auto px-6 py-10">
      <div className="animate-fade-up flex w-full max-w-[620px] flex-col items-center">
        <BotAvatar kind="pilot" mood="idle" size={88} variant="body" className="mb-5" title="Kudu Pilot" />
        <h1 className="mb-1.5 text-center text-[24px] font-semibold tracking-[-0.02em]">Your second brain for calls</h1>
        <p className="mb-8 max-w-[440px] text-center text-[13px] text-text-muted">
          Live transcript of both sides, and answers that only use your real experience. Visible to you, never to the other side.
        </p>

        <div className="mb-6 grid w-full grid-cols-3 gap-2">
          {MODES.map((m) => (
            <button
              key={m.id}
              onClick={() => props.onMode(m.id)}
              className={clsx(
                "flex flex-col gap-1.5 rounded-2xl border p-3.5 text-left transition",
                mode === m.id
                  ? "border-accent bg-accent-soft ring-2 ring-accent-soft"
                  : "border-border-default hover:border-border-strong hover:bg-bg-card",
              )}
            >
              <span className={mode === m.id ? "text-accent" : "text-text-muted"}>{m.icon}</span>
              <span className="text-[13px] font-medium">{m.label}</span>
              <span className="text-[11.5px] leading-snug text-text-muted">{m.hint}</span>
            </button>
          ))}
        </div>

        <div className="mb-6 w-full divide-y divide-border-soft rounded-2xl border border-border-default">
          <SourceRow
            icon={<Mic size={15} />}
            title="My microphone"
            body="Your side of the conversation"
            on={props.mic}
            onChange={props.onMic}
          />
          {mode !== "practice" && (
            <SourceRow
              icon={<Volume2 size={15} />}
              title="Computer audio"
              body="The other participants (Zoom, Meet, Teams…) - use headphones for the cleanest transcript"
              on={props.system}
              onChange={props.onSystem}
            />
          )}
          <button onClick={props.onEditContext} className="flex w-full items-center gap-3 px-4 py-3 text-left transition hover:bg-bg-card">
            <span className={props.hasProfile ? "text-success" : "text-warning"}>
              {props.hasProfile ? <Check size={15} /> : <UserCircle2 size={15} />}
            </span>
            <span className="min-w-0 flex-1">
              <span className="block text-[13px] font-medium">{props.hasProfile ? "Your context is set" : "Add your context"}</span>
              <span className="block text-[11.5px] text-text-muted">
                {props.hasProfile
                  ? "Background, projects and session notes. Click to edit."
                  : "Paste your resume or projects so answers use your real experience."}
              </span>
            </span>
          </button>
        </div>

        <button
          onClick={props.onStart}
          disabled={props.starting || (!props.mic && (mode === "practice" || !props.system))}
          className="btn-brand flex h-11 items-center gap-2 rounded-full px-6 text-[14px] font-medium disabled:opacity-40"
        >
          {props.starting ? (
            <ThinkingOrb state="connecting" size={20} theme="auto" aria-label="Starting" />
          ) : (
            <Play size={14} fill="currentColor" />
          )}
          {props.starting ? "Starting…" : mode === "practice" ? "Start practice" : "Start listening"}
        </button>
        <p className="mt-4 flex items-center gap-1.5 text-[11.5px] text-text-muted">
          <Lock size={11} />
          Audio stays in memory and is only sent for transcription. Nothing is recorded to disk.
        </p>
        <p className="mt-2 flex items-center gap-1 text-[11.5px] text-text-muted">
          Mid-call, press <Kbd>Ctrl</Kbd>
          <Kbd>Shift</Kbd>
          <Kbd>Enter</Kbd> from any app for a suggested reply.
        </p>
      </div>
    </div>
  );
}

function SourceRow({
  icon,
  title,
  body,
  on,
  onChange,
}: {
  icon: React.ReactNode;
  title: string;
  body: string;
  on: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex cursor-pointer items-center gap-3 px-4 py-3" onClick={() => onChange(!on)}>
      <span className={on ? "text-accent" : "text-text-muted"}>{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="block text-[13px] font-medium">{title}</span>
        <span className="block text-[11.5px] text-text-muted">{body}</span>
      </span>
      <Switch on={on} onChange={onChange} label={title} />
    </div>
  );
}

// ── Live header bits ──────────────────────────────────────────────

function LiveClock({ startedAt }: { startedAt: number | null }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const secs = startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0;
  const clock = `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}`;
  return (
    <span className="flex items-center gap-1.5 rounded-full bg-danger-soft px-2.5 py-0.5 text-[11.5px] font-medium tabular-nums text-danger">
      <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-danger" />
      Live · {clock}
    </span>
  );
}

/** Five-bar level meter driven by the backend's ~4 Hz `meeting-level` events. */
function LevelMeter({ level, active, label, icon }: { level: number; active: boolean; label: string; icon: React.ReactNode }) {
  const bars = [0.35, 0.65, 1, 0.65, 0.35];
  return (
    <span
      className={clsx("flex items-center gap-1.5 text-[11.5px]", active ? "text-text-secondary" : "text-text-muted line-through")}
      title={active ? label : `${label} (off)`}
    >
      {icon}
      <span className="flex h-3.5 items-center gap-[2px]">
        {bars.map((w, i) => (
          <span
            key={i}
            className={clsx("w-[3px] rounded-full transition-[height] duration-200", level > 0.12 ? "bg-accent" : "bg-border-strong")}
            style={{ height: `${Math.max(3, Math.min(14, 3 + level * w * 14))}px` }}
          />
        ))}
      </span>
    </span>
  );
}

// ── Transcript ────────────────────────────────────────────────────

function TranscriptPane({ meeting, mode }: { meeting: ReturnType<typeof useMeeting>; mode: MeetingMode }) {
  const { status, transcript: entries, levels, transcribing, partials } = meeting;
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);
  const hasPartial = !!(partials.them || partials.me);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current) el.scrollTop = el.scrollHeight;
  }, [entries.length, transcribing, partials.them, partials.me]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-10 flex-shrink-0 items-center gap-3 px-4">
        <span className="text-[12px] font-medium text-text-muted">Transcript</span>
        {status.live && (
          <span className="flex items-center gap-3">
            <LevelMeter level={levels.mic} active={status.mic} label="Your mic" icon={<Mic size={12} />} />
            {mode !== "practice" && (
              <LevelMeter level={levels.system} active={status.system} label="Computer audio" icon={<Volume2 size={12} />} />
            )}
          </span>
        )}
        {entries.length > 0 && !status.live && (
          <IconButton title="Clear session" size="sm" className="ml-auto" onClick={meeting.clear}>
            <Trash2 size={13} />
          </IconButton>
        )}
      </div>
      <div
        ref={scrollRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        className="scrollbar-thin flex min-h-0 flex-1 flex-col gap-2.5 overflow-y-auto px-4 pb-4"
      >
        {entries.length === 0 && !transcribing && !hasPartial ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 py-8 text-center">
            {status.live && (
              <BotAvatar
                kind="pilot"
                mood={levels.mic > 0.15 || levels.system > 0.15 ? "listening" : "idle"}
                size={56}
                title="Listening"
              />
            )}
            <p className="max-w-[260px] text-[12.5px] text-text-muted">
              {status.live
                ? mode === "practice"
                  ? "Press “Next question”, then answer out loud."
                  : status.streaming
                    ? "Listening… words appear as they are spoken."
                    : "Listening… lines appear a few seconds after each pause."
                : "No transcript yet."}
            </p>
          </div>
        ) : (
          entries.map((e) => (
            <TranscriptLine
              key={e.id}
              entry={e}
              onAnswer={mode === "practice" ? undefined : (text) => void meeting.assist("ask", `Help me answer this, in my own voice: "${text}"`)}
            />
          ))
        )}
        {(["them", "me"] as const).map((sp) =>
          partials[sp] ? (
            <div key={sp} className={`flex ${sp === "me" ? "justify-end" : "justify-start"}`}>
              <div className="live-partial max-w-[85%] rounded-2xl px-3 py-2 text-[13px] text-text-muted">
                <span className="live-partial-dot" aria-hidden />
                <span className="live-partial-who">{sp === "me" ? "You" : "Them"}</span> {partials[sp]}
              </div>
            </div>
          ) : null,
        )}
        {transcribing && (
          <div className="flex items-center gap-2 px-1 text-[12px]">
            <ThinkingOrb state="composing" size={20} theme="auto" aria-label="Transcribing" />
            <span className="shimmer-text">Transcribing…</span>
          </div>
        )}
      </div>
    </div>
  );
}

const SPEAKER: Record<TranscriptEntry["speaker"], string> = { them: "Them", me: "You", coach: "Coach", note: "Note" };

function TranscriptLine({ entry, onAnswer }: { entry: TranscriptEntry; onAnswer?: (text: string) => void }) {
  const time = new Date(entry.at_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (entry.speaker === "note") {
    return (
      <div className="animate-fade-up self-center rounded-full border border-dashed border-border-default px-3 py-1 text-[12px] italic text-text-secondary">
        {entry.text}
      </div>
    );
  }
  const mine = entry.speaker === "me";
  const question = entry.speaker === "them" && looksLikeQuestion(entry.text);
  return (
    <div className={clsx("animate-fade-up group flex max-w-[88%] flex-col gap-0.5", mine ? "items-end self-end" : "items-start self-start")}>
      <span className="px-1 text-[10.5px] text-text-muted">
        <span className={clsx("font-medium", entry.speaker === "coach" ? "text-warning" : mine ? "text-accent" : "text-text-secondary")}>
          {SPEAKER[entry.speaker]}
        </span>{" "}
        · {time}
      </span>
      <div
        className={clsx(
          "rounded-2xl px-3.5 py-2 text-[13px] leading-relaxed",
          mine ? "rounded-br-md bg-accent-soft text-text-primary" : "rounded-bl-md bg-bg-tertiary text-text-primary",
          question && "ring-1 ring-accent/40",
          entry.speaker === "coach" && "border border-warning/30 bg-warning-soft",
        )}
      >
        {question && (
          <span className="mb-1 mr-1.5 inline-flex items-center rounded-full bg-accent-soft px-1.5 py-px align-middle text-[10px] font-semibold uppercase tracking-wide text-accent">
            Question
          </span>
        )}
        {entry.text}
      </div>
      {question && onAnswer && (
        <button
          onClick={() => onAnswer(entry.text)}
          className="ml-1 mt-0.5 flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium text-accent opacity-70 transition hover:bg-accent-soft hover:opacity-100"
        >
          <Sparkles size={11} />
          Answer this
        </button>
      )}
    </div>
  );
}

// ── Copilot (answers) ─────────────────────────────────────────────

function CopilotPane({
  meeting,
  mode,
  hasProfile,
  onEditContext,
}: {
  meeting: ReturnType<typeof useMeeting>;
  mode: MeetingMode;
  hasProfile: boolean;
  onEditContext: () => void;
}) {
  const { status } = meeting;
  const actions = mode === "practice" ? PRACTICE_ACTIONS : LIVE_ACTIONS;
  const [input, setInput] = useState("");
  const [target, setTarget] = useState<InputTarget>(mode === "practice" ? "me" : "ask");

  useEffect(() => setTarget(mode === "practice" ? "me" : "ask"), [mode]);

  const submit = () => {
    const text = input.trim();
    if (!text) return;
    setInput("");
    if (target === "ask") void meeting.assist("ask", text);
    else void meeting.addEntry(target, text);
  };

  const targets: { id: InputTarget; label: string }[] = [
    { id: "ask", label: "Ask Kudu" },
    ...(mode !== "practice" ? [{ id: "them" as const, label: "They said" }] : []),
    { id: "me", label: mode === "practice" ? "My answer" : "I said" },
    { id: "note", label: "Note" },
  ];

  return (
    <div className="flex h-full flex-col bg-bg-secondary">
      <div className="flex flex-shrink-0 flex-col gap-2 px-4 pb-2 pt-3">
        <div className="flex flex-wrap gap-1.5">
          {actions.map((a, i) => (
            <button
              key={a.action + a.label}
              onClick={() => void meeting.assist(a.action)}
              className={clsx(
                "flex h-8 items-center gap-1.5 rounded-full px-3 text-[12.5px] font-medium transition",
                i === 0
                  ? "btn-brand"
                  : "border border-border-default text-text-secondary hover:bg-bg-hover hover:text-text-primary",
              )}
            >
              {a.icon}
              {a.label}
            </button>
          ))}
          {mode !== "practice" && !status.live && meeting.transcript.length > 0 && (
            <button
              onClick={() => void meeting.assist("notes")}
              className="flex h-8 items-center gap-1.5 rounded-full border border-accent/40 bg-accent-soft px-3 text-[12.5px] font-medium text-accent transition hover:bg-accent/20"
            >
              <ListChecks size={13} />
              Meeting notes
            </button>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[11.5px] text-text-muted">
          {mode !== "practice" && (
            <label className="flex cursor-pointer items-center gap-1.5" title="Suggest a reply automatically whenever the other side asks a question">
              <Switch on={meeting.autoAnswer} onChange={meeting.setAutoAnswer} label="Auto-answer questions" />
              Auto-answer questions
            </label>
          )}
          <label className="flex cursor-pointer items-center gap-1.5" title="Attach a screenshot (shared slides, a coding question…) to each request">
            <Switch on={meeting.includeScreen} onChange={meeting.setIncludeScreen} label="Include my screen" />
            <Monitor size={12} />
            Include screen
          </label>
          <label className="flex cursor-pointer items-center gap-1.5" title="Ground answers in My context (about me, projects, this session, research brief)">
            <Switch on={meeting.useContext} onChange={(v) => void meeting.setUseContext(v)} label="Use my context" />
            <Lock size={12} />
            Use my context
          </label>
          {mode !== "practice" && <CopilotBarToggle live={status.live} />}
        </div>
      </div>

      <div className="scrollbar-thin flex min-h-0 flex-1 flex-col gap-2.5 overflow-y-auto px-4 pb-3">
        {meeting.cards.length === 0 ? (
          <div className="flex flex-col gap-2 rounded-2xl border border-dashed border-border-default p-4 text-[12.5px] text-text-muted">
            <p>
              {mode === "practice"
                ? "The coach asks role-specific questions and scores your answers using only your real experience."
                : "Press “What should I say?” - or Ctrl+Shift+Enter from any app - for a reply to whatever was just asked."}
            </p>
            {!hasProfile && (
              <button onClick={onEditContext} className="w-fit text-warning underline-offset-2 hover:underline">
                Add your background so answers use your real experience →
              </button>
            )}
          </div>
        ) : (
          meeting.cards.map((c, i) => <AnswerCard key={c.id} card={c} latest={i === 0} onSave={meeting.saveNotes} />)
        )}
      </div>

      <div className="flex-shrink-0 px-4 pb-4">
        <div className="rounded-2xl border border-border-default bg-bg-elevated focus-within:border-border-strong">
          <div className="flex gap-1 px-2 pt-2">
            {targets.map((t) => (
              <button
                key={t.id}
                onClick={() => setTarget(t.id)}
                className={clsx(
                  "rounded-full px-2.5 py-0.5 text-[11.5px] font-medium transition",
                  target === t.id ? "bg-bg-hover text-text-primary" : "text-text-muted hover:text-text-secondary",
                )}
              >
                {t.label}
              </button>
            ))}
          </div>
          <div className="flex items-end gap-2 p-2 pl-3">
            <textarea
              rows={1}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  submit();
                }
              }}
              placeholder={
                target === "ask"
                  ? "Ask anything about this conversation…"
                  : target === "note"
                    ? "Add a private note to the transcript…"
                    : "Type what was said, if the audio missed it…"
              }
              className="max-h-28 min-w-0 flex-1 resize-none bg-transparent py-1 text-[13px] outline-none placeholder:text-text-muted"
            />
            <button
              onClick={submit}
              disabled={!input.trim()}
              className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-full btn-brand disabled:opacity-25"
              title="Send (Enter)"
            >
              <ArrowUp size={14} strokeWidth={2.4} />
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function AnswerCard({
  card,
  latest,
  onSave,
}: {
  card: AssistCard;
  latest: boolean;
  onSave: (notes: string, includeTranscript: boolean) => Promise<string>;
}) {
  const [copied, setCopied] = useState(false);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | string>("idle");
  const [withTranscript, setWithTranscript] = useState(false);

  const copy = async () => {
    await navigator.clipboard.writeText(card.text).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const save = async () => {
    setSaveState("saving");
    try {
      await onSave(card.text, withTranscript);
      setSaveState("saved");
    } catch (err) {
      setSaveState(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div
      data-active={card.pending}
      className={clsx(
        "border-beam animate-fade-up relative flex flex-col gap-2 rounded-2xl border bg-bg-primary p-3.5",
        latest ? "border-border-default shadow-[0_6px_20px_-12px_rgba(0,0,0,0.35)]" : "border-border-soft opacity-80 hover:opacity-100",
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="flex min-w-0 items-center gap-1.5 text-[11.5px] font-medium text-accent">
          <Sparkles size={12} className="flex-shrink-0" />
          <span className="truncate">{card.label}</span>
        </span>
        {!card.pending && !card.error && (
          <IconButton title={copied ? "Copied" : "Copy"} size="sm" onClick={copy}>
            {copied ? <Check size={12} /> : <Copy size={12} />}
          </IconButton>
        )}
      </div>
      {card.context && (
        <p className="line-clamp-2 border-l-2 border-accent/40 pl-2.5 text-[12px] text-text-muted" title={card.context}>
          <span className="mr-1.5 text-[10px] font-semibold uppercase tracking-wider">They asked</span>
          {card.context}
        </p>
      )}
      {latest && !card.pending && !card.error && (card.action === "suggest" || card.action === "ask") && (
        <span className="-mb-1 text-[10.5px] font-semibold uppercase tracking-wider text-text-muted">Say this</span>
      )}
      {card.pending ? (
        <div className="flex items-center gap-2.5 py-1 text-[13px]">
          <BotAvatar kind="pilot" mood="thinking" size={26} />
          <span className="shimmer-text font-medium">
            {card.action === "screen" ? "Reading your screen…" : "Writing your answer…"}
          </span>
        </div>
      ) : card.error ? (
        <p className="text-[12.5px] text-danger">{card.error}</p>
      ) : (
        <div className={clsx(latest ? "text-[15px] leading-relaxed" : "text-[13px]")}>
          {card.screen && (
            <a href={card.screen} target="_blank" rel="noreferrer" title="The screen this answer is based on">
              <img
                src={card.screen}
                alt="Captured screen"
                className="float-right mb-2 ml-3 w-36 rounded-lg border border-border-soft shadow-sm"
              />
            </a>
          )}
          <Markdown>{card.text}</Markdown>
        </div>
      )}
      {card.action === "notes" && !card.pending && !card.error && (
        <div className="mt-1 flex flex-wrap items-center gap-3 border-t border-border-soft pt-2.5">
          <button
            onClick={save}
            disabled={saveState === "saving" || saveState === "saved"}
            className="flex items-center gap-1.5 rounded-lg border border-border-default px-2.5 py-1 text-[12px] transition hover:bg-bg-hover disabled:opacity-60"
          >
            {saveState === "saved" ? <Check size={12} /> : <Save size={12} />}
            {saveState === "saved" ? "Saved to chats" : "Save to chats"}
          </button>
          <label className="flex cursor-pointer items-center gap-1.5 text-[11.5px] text-text-muted">
            <Switch on={withTranscript} onChange={setWithTranscript} label="Include transcript" />
            include transcript
          </label>
          {saveState !== "idle" && saveState !== "saving" && saveState !== "saved" && (
            <span className="text-[11.5px] text-danger">{saveState}</span>
          )}
        </div>
      )}
    </div>
  );
}

// ── Context drawer ────────────────────────────────────────────────

function ContextDrawer({
  profile,
  onSave,
  onResearch,
  researching,
  onClose,
}: {
  profile: MeetingProfile;
  onSave: (p: MeetingProfile) => Promise<void>;
  onResearch: (urls: string[]) => Promise<MeetingProfile>;
  researching: string | null;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState(profile);
  const [status, setStatus] = useState("");
  const [links, setLinks] = useState(() => profile.sources.map((x) => x.url).join("\n"));
  useEffect(() => setDraft(profile), [profile]);

  const runResearch = async () => {
    const urls = links.split(/\s+/).map((u) => u.trim()).filter(Boolean);
    if (!urls.length) {
      setStatus("Paste one or more links first.");
      return;
    }
    setStatus("");
    try {
      // Save the text fields first so the brief is written with them in mind.
      await onSave({ ...draft, sources: profile.sources });
      const next = await onResearch(urls);
      setDraft(next);
    } catch (err) {
      setStatus(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  type TextKey = "about" | "projects" | "session_context" | "style" | "research";
  const field = (key: TextKey, label: string, placeholder: string, rows = 3) => (
    <label className="flex flex-col gap-1.5">
      <span className="text-[12px] font-medium text-text-secondary">{label}</span>
      <textarea
        value={draft[key]}
        onChange={(e) => setDraft({ ...draft, [key]: e.target.value })}
        rows={rows}
        placeholder={placeholder}
        className="w-full resize-y rounded-xl border border-border-default bg-bg-primary px-3 py-2 text-[13px] outline-none focus:border-border-focus"
      />
    </label>
  );

  return (
    <div className="absolute inset-0 z-20 flex justify-end bg-black/30" onMouseDown={onClose}>
      <div
        onMouseDown={(e) => e.stopPropagation()}
        className="animate-fade-up flex h-full w-full max-w-[440px] flex-col border-l border-border-default bg-bg-elevated shadow-pop"
      >
        <div className="flex h-12 flex-shrink-0 items-center justify-between border-b border-border-soft px-4">
          <h3 className="text-[14px] font-semibold">My context</h3>
          <IconButton title="Close" onClick={onClose}>
            <X size={16} />
          </IconButton>
        </div>
        <div className="scrollbar-thin flex flex-1 flex-col gap-4 overflow-y-auto p-4">
          <p className="flex items-start gap-2 rounded-xl bg-bg-card p-3 text-[12px] text-text-muted">
            <Lock size={13} className="mt-0.5 flex-shrink-0" />
            Stored only on this computer. The copilot grounds every suggestion in this and won't invent experience you don't have.
          </p>
          {field("about", "About me", "Role, years of experience, key skills - or paste your resume.", 5)}
          {field("projects", "Projects & responsibilities", "What you're working on, your part in it, results and numbers you'd cite.", 5)}
          {field("session_context", "This session", "Agenda, who you're meeting, or the job description you're interviewing for.", 4)}
          {field("style", "Answer style", "e.g. concise and confident, under 40 words, plain English", 1)}

          {/* Links → research brief */}
          <div className="flex flex-col gap-1.5 rounded-xl border border-border-soft bg-bg-card p-3">
            <span className="text-[12px] font-medium text-text-secondary">Links to read</span>
            <span className="text-[11.5px] text-text-muted">
              Job post, company page, product docs, a profile, a repo. Kudu Scout fetches each one and writes a brief Kudu Pilot will answer from.
            </span>
            <textarea
              value={links}
              onChange={(e) => setLinks(e.target.value)}
              rows={3}
              placeholder={"https://…\nhttps://…"}
              className="w-full resize-y rounded-xl border border-border-default bg-bg-primary px-3 py-2 font-mono text-[12.5px] outline-none focus:border-border-focus"
            />
            <div className="flex items-center gap-3">
              <button
                onClick={() => void runResearch()}
                disabled={!!researching}
                className="flex h-8 items-center gap-1.5 rounded-lg btn-brand px-3 text-[12.5px] font-medium disabled:opacity-50"
              >
                <Sparkles size={13} />
                {researching ? "Researching…" : draft.research ? "Research again" : "Fetch & research"}
              </button>
              {researching && <span className="text-[12px] text-text-muted">{researching}</span>}
            </div>
            {draft.sources.length > 0 && (
              <ul className="mt-1 flex flex-col gap-1 text-[12px]">
                {draft.sources.map((src) => (
                  <li key={src.url} className="flex items-start gap-2 text-text-secondary">
                    <span className={clsx("mt-1 h-1.5 w-1.5 flex-shrink-0 rounded-full", src.error ? "bg-danger" : "bg-success")} />
                    <span className="min-w-0 flex-1 truncate" title={src.url}>
                      {src.title || src.url}
                      <span className="text-text-muted"> · {src.error ? src.error : `${src.chars.toLocaleString()} chars read`}</span>
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
          {field("research", "Research brief", "Written from the links above. Edit anything that is off; Kudu Pilot treats this as fact.", 8)}
          <label className="flex cursor-pointer items-center justify-between gap-3 rounded-xl border border-border-soft bg-bg-card px-3 py-2.5">
            <span className="flex flex-col">
              <span className="text-[13px]">Use my context in answers</span>
              <span className="text-[11.5px] text-text-muted">Off = answer from the conversation and screen only. Also in the in-call bar's menu.</span>
            </span>
            <Switch on={!draft.skip_context} onChange={(v) => setDraft({ ...draft, skip_context: !v })} label="Use my context" />
          </label>
        </div>
        <div className="flex flex-shrink-0 items-center gap-3 border-t border-border-soft p-4">
          <button
            onClick={async () => {
              try {
                await onSave(draft);
                onClose();
              } catch (err) {
                setStatus(err instanceof Error ? err.message : String(err));
              }
            }}
            className="rounded-lg btn-brand px-4 py-2 text-[13px] font-medium"
          >
            Save context
          </button>
          <button onClick={onClose} className="rounded-lg px-3 py-2 text-[13px] text-text-secondary hover:bg-bg-hover">
            Cancel
          </button>
          {status && <span className="text-[12px] text-danger">{status}</span>}
        </div>
      </div>
    </div>
  );
}

/** The floating in-call bar (copilot window): opens over the call when a
 *  session starts, hidden from screen share. Ctrl+Shift+\ shows/hides it. */
function CopilotBarToggle({ live }: { live: boolean }) {
  const [on, setOn] = useState(copilotBarEnabled);
  return (
    <span className="flex items-center gap-2">
      <label className="flex cursor-pointer items-center gap-1.5" title="Float the Pilot bar over your call - invisible to screen share (Ctrl+Shift+\)">
        <Switch
          on={on}
          onChange={(v) => {
            setCopilotBarEnabled(v);
            setOn(v);
          }}
          label="In-call bar"
        />
        In-call bar
      </label>
      {live && on && (
        <button onClick={() => void copilotShow()} className="text-accent underline-offset-2 hover:underline">
          Show bar
        </button>
      )}
    </span>
  );
}
