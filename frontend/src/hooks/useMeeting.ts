import { useCallback, useEffect, useRef, useState } from "react";
import {
  meetingAddEntry,
  meetingAssist,
  meetingClear,
  meetingResearch,
  meetingRestore,
  sessionDelete,
  sessionGet,
  sessionList,
  type SessionSummary,
  onMeetingProfile,
  onMeetingResearchProgress,
  meetingGetProfile,
  meetingSaveNotes,
  meetingSetProfile,
  meetingStart,
  meetingStatus,
  meetingStop,
  meetingTranscript,
  newId,
  onMeetingAnswerDelta,
  onMeetingLevel,
  onMeetingPartial,
  onMeetingStatus,
  onMeetingTranscribing,
  onMeetingTranscript,
  type MeetingAction,
  type MeetingMode,
  type MeetingProfile,
  type MeetingStatus,
  type TranscriptEntry,
} from "../lib/tauri";
import { TURN_QUIET_MS, lastTheirTurn, looksLikeQuestion, needsAnswer, openQuestion, type AnsweredMark } from "../lib/turns";

export interface AssistCard {
  id: string;
  action: MeetingAction;
  label: string;
  text: string;
  pending: boolean;
  error?: string;
  /** When the request went out - drives the elapsed timer while it loads. */
  startedAt: number;
  /** What the other side had just said when this was asked, for context. */
  context?: string;
  /** Thumbnail of the screen the answer was based on, when one was attached. */
  screen?: string | null;
}

const EMPTY_PROFILE: MeetingProfile = { about: "", projects: "", session_context: "", style: "", sources: [], research: "", skip_context: false };
const IDLE: MeetingStatus = { live: false, mode: "", mic: false, system: false, started_at_ms: null, warning: null };

export const ACTION_LABELS: Record<MeetingAction, string> = {
  assist: "Assist",
  insights: "Live insights",
  suggest: "What should I say?",
  summary: "Summary so far",
  followups: "Follow-up questions",
  explain: "Explain terms",
  notes: "Meeting notes",
  practice_question: "Next question",
  practice_feedback: "Feedback on my answer",
  screen: "Solve my screen",
  ask: "Question",
};

/** The in-call copilot bar (copilot window) is on unless the user turned it
 *  off. localStorage is shared by every window of the app, so both the main
 *  window and the bar read the same switch. */
const BAR_KEY = "mouse-ai-copilot-bar";
export const copilotBarEnabled = () => localStorage.getItem(BAR_KEY) !== "off";
export const setCopilotBarEnabled = (on: boolean) => localStorage.setItem(BAR_KEY, on ? "on" : "off");
export const AUTO_ANSWER_KEY = "mouse-ai-auto-answer";

export { looksLikeQuestion, lastTheirTurn, openQuestion };

function errText(err: unknown) {
  return err instanceof Error ? err.message : String(err);
}

/** All copilot state lives here (not in the view) so a running session, its
 *  transcript and the Ctrl+Shift+Enter hotkey survive switching views. */
export function useMeeting() {
  const [status, setStatus] = useState<MeetingStatus>(IDLE);
  const [transcript, setTranscript] = useState<TranscriptEntry[]>([]);
  const [cards, setCards] = useState<AssistCard[]>([]);
  const [profile, setProfile] = useState<MeetingProfile>(EMPTY_PROFILE);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [includeScreen, setIncludeScreen] = useState(false);
  const includeScreenRef = useRef(includeScreen);
  includeScreenRef.current = includeScreen;
  const transcriptRef = useRef(transcript);
  transcriptRef.current = transcript;
  const [levels, setLevels] = useState({ mic: 0, system: 0 });
  const [transcribing, setTranscribing] = useState(false);
  /** Words being spoken right now, per side (live transcription only). */
  const [partials, setPartials] = useState<{ me: string; them: string }>({ me: "", them: "" });
  const [autoAnswer, setAutoAnswerState] = useState(() => localStorage.getItem(AUTO_ANSWER_KEY) === "on");
  const setAutoAnswer = useCallback((v: boolean) => {
    localStorage.setItem(AUTO_ANSWER_KEY, v ? "on" : "off");
    setAutoAnswerState(v);
  }, []);

  // Saved calls (kept on this PC). A call still "not ended" when the app opens means the
  // app closed mid-call - offered back as a one-tap recovery.
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const refreshSessions = useCallback(
    () => sessionList().then(setSessions).catch(() => {}),
    [],
  );

  useEffect(() => {
    meetingStatus().then(setStatus).catch(() => {});
    meetingTranscript().then(setTranscript).catch(() => {});
    void refreshSessions();
    meetingGetProfile().then(setProfile).catch(() => {});
    const unTranscript = onMeetingTranscript((entry) =>
      setTranscript((prev) => (prev.some((e) => e.id === entry.id) ? prev : [...prev, entry])),
    );
    const unStatus = onMeetingStatus((s) => {
      setStatus(s);
      if (!s.live) {
        setLevels({ mic: 0, system: 0 });
        setTranscribing(false);
        setPartials({ me: "", them: "" });
      }
    });
    const unPartial = onMeetingPartial((p) => setPartials((prev) => (prev[p.speaker] === p.text ? prev : { ...prev, [p.speaker]: p.text })));
    const unLevel = onMeetingLevel(setLevels);
    const unBusy = onMeetingTranscribing(setTranscribing);
    // Streamed answers: grow the pending card as the words arrive.
    const unDelta = onMeetingAnswerDelta(({ id, delta }) =>
      setCards((prev) => prev.map((c) => (c.id === id && c.pending ? { ...c, text: c.text + delta } : c))),
    );
    return () => {
      unPartial.then((fn) => fn());
      unDelta.then((fn) => fn());
      unTranscript.then((fn) => fn());
      unStatus.then((fn) => fn());
      unLevel.then((fn) => fn());
      unBusy.then((fn) => fn());
    };
  }, [refreshSessions]);

  // What the last start used, so Resume needs no questions.
  const lastStartRef = useRef<{ mode: MeetingMode; mic: boolean; system: boolean }>({ mode: "meeting", mic: true, system: true });

  const start = useCallback(async (mode: MeetingMode, mic: boolean, system: boolean, resume = false) => {
    setStarting(true);
    setError(null);
    try {
      if (!resume) {
        setTranscript([]);
        setCards([]);
      }
      lastStartRef.current = { mode, mic, system };
      setStatus(await meetingStart(mode, mic, system, copilotBarEnabled(), resume));
    } catch (err) {
      setError(errText(err));
    } finally {
      setStarting(false);
    }
  }, []);

  /** Continue the paused session: same mode and sources, same transcript and clock. */
  const resume = useCallback(async () => {
    const last = lastStartRef.current;
    const mode = (status.mode || last.mode) as MeetingMode;
    await start(mode, last.mic, mode === "practice" ? false : last.system, true);
  }, [start, status.mode]);

  const stop = useCallback(async () => {
    try {
      setStatus(await meetingStop());
      void refreshSessions();
    } catch (err) {
      setError(errText(err));
    }
  }, [refreshSessions]);

  /** Open a saved call as the paused session: transcript and answers come back, Resume continues it. */
  const openSession = useCallback(
    async (id: string) => {
      try {
        const { status: next, session } = await meetingRestore(id);
        setStatus(next);
        setTranscript(session.transcript);
        setCards(
          [...session.cards].reverse().map((c) => ({
            id: c.id,
            action: c.action,
            label: c.action === "ask" && c.question ? c.question : ACTION_LABELS[c.action] ?? "Answer",
            text: c.text,
            pending: false,
            startedAt: c.at_ms,
            context: c.question && c.action !== "ask" ? c.question : undefined,
          })),
        );
        lastStartRef.current = { mode: session.mode, mic: true, system: session.mode !== "practice" };
        setError(null);
        void refreshSessions();
      } catch (err) {
        setError(errText(err));
      }
    },
    [refreshSessions],
  );

  const deleteSession = useCallback(
    async (id: string) => {
      await sessionDelete(id).catch(() => {});
      void refreshSessions();
    },
    [refreshSessions],
  );

  /** A saved call as Markdown, for Copy / export. */
  const exportSession = useCallback(async (id: string) => {
    const s = await sessionGet(id);
    const when = s.started_ms ? new Date(s.started_ms).toLocaleString() : "";
    const lines = [`# ${s.title}`, `${s.mode} · ${when}`, "", "## Transcript"];
    for (const e of s.transcript) {
      const who = e.speaker === "them" ? "Them" : e.speaker === "me" ? "Me" : e.speaker === "coach" ? "Coach" : "Note";
      lines.push(`**${who}:** ${e.text}`);
    }
    if (s.cards.length) {
      lines.push("", "## Answers");
      for (const c of s.cards) {
        lines.push(`### ${c.question || ACTION_LABELS[c.action] || "Answer"}`, c.text, "");
      }
    }
    return lines.join("\n");
  }, []);

  const assist = useCallback(async (action: MeetingAction, question: string | null = null) => {
    const id = newId();
    const label = action === "ask" && question ? question : ACTION_LABELS[action];
    // Question lock: "What should I say?" answers the newest question they asked that
    // I have not already replied to - and the card shows exactly which one it was.
    const locked = action === "suggest" || action === "assist" ? (openQuestion(transcriptRef.current) ?? lastTheirTurn(transcriptRef.current)) : null;
    const heard = action === "suggest" && question ? question : locked?.text;
    const context = heard && heard.length > 140 ? `${heard.slice(0, 140)}…` : heard;
    if (action === "suggest" && !question && locked && looksLikeQuestion(locked.text)) question = locked.text;
    setCards((prev) => [{ id, action, label, text: "", pending: true, startedAt: Date.now(), context }, ...prev].slice(0, 30));
    try {
      const { text, screen } = await meetingAssist(action, question, includeScreenRef.current, id);
      setCards((prev) => prev.map((c) => (c.id === id ? { ...c, text, screen, pending: false } : c)));
    } catch (err) {
      setCards((prev) => prev.map((c) => (c.id === id ? { ...c, pending: false, error: errText(err) } : c)));
    }
  }, []);

  // Auto-answer: when the other side's newest line reads as a question, ask
  // for a suggested reply without the user having to press anything. Waits a
  // beat (questions often span two chunks) and never stacks requests.
  const cardsRef = useRef(cards);
  cardsRef.current = cards;
  const answeredRef = useRef<AnsweredMark | null>(null);
  const partialsRef = useRef(partials);
  partialsRef.current = partials;
  useEffect(() => {
    // While the in-call bar is up it does the auto-answering, right over the call.
    if (!autoAnswer || !status.live || status.mode === "practice" || copilotBarEnabled()) return;
    const turn = lastTheirTurn(transcript);
    if (!turn || !looksLikeQuestion(turn.text) || !needsAnswer(turn, answeredRef.current)) return;
    // Answer the finished question, not the first half: wait for a quiet beat and for
    // the live partial to clear.
    const t = setTimeout(() => {
      if (cardsRef.current.some((c) => c.pending) || partialsRef.current.them) return;
      const prev = answeredRef.current;
      answeredRef.current = { id: turn.id, len: turn.text.length, count: prev && prev.id === turn.id ? prev.count + 1 : 1 };
      void assist("suggest");
    }, TURN_QUIET_MS);
    return () => clearTimeout(t);
  }, [transcript, partials.them, autoAnswer, status.live, status.mode, assist]);

  const addEntry = useCallback(async (speaker: "them" | "me" | "note", text: string) => {
    try {
      await meetingAddEntry(speaker, text);
    } catch (err) {
      setError(errText(err));
    }
  }, []);

  const clear = useCallback(async () => {
    await meetingClear().catch(() => {});
    setTranscript([]);
    setCards([]);
  }, []);

  const saveProfile = useCallback(async (next: MeetingProfile) => {
    await meetingSetProfile(next);
    setProfile(next);
  }, []);

  // Links → brief. Progress lines come from Rust while it reads each page.
  const [researching, setResearching] = useState<string | null>(null);
  const research = useCallback(async (urls: string[]) => {
    setResearching("Starting…");
    try {
      const next = await meetingResearch(urls);
      setProfile(next);
      return next;
    } finally {
      setResearching(null);
    }
  }, []);
  useEffect(() => {
    const unP = onMeetingProfile(setProfile);
    const unR = onMeetingResearchProgress((step) => setResearching((cur) => (cur === null ? null : step)));
    return () => {
      unP.then((fn) => fn());
      unR.then((fn) => fn());
    };
  }, []);

  /** The one switch both surfaces show: ground answers in my context, or not. */
  const setUseContext = useCallback(
    (use: boolean) => saveProfile({ ...profile, skip_context: !use }),
    [profile, saveProfile],
  );

  const saveNotes = useCallback(
    (notes: string, includeTranscript: boolean) => meetingSaveNotes(notes, includeTranscript),
    [],
  );

  return {
    status,
    /** Paused with something to continue (the server says so; the local transcript is the fallback). */
    resumable: !status.live && (status.resumable ?? transcript.length > 0),
    resume,
    transcript,
    cards,
    profile,
    starting,
    error,
    setError,
    includeScreen,
    setIncludeScreen,
    levels,
    transcribing,
    partials,
    autoAnswer,
    setAutoAnswer,
    start,
    stop,
    sessions,
    refreshSessions,
    openSession,
    deleteSession,
    exportSession,
    assist,
    addEntry,
    clear,
    saveProfile,
    research,
    researching,
    setUseContext,
    useContext: !profile.skip_context,
    saveNotes,
  };
}
