import { useCallback, useEffect, useRef, useState } from "react";
import {
  meetingAddEntry,
  meetingAssist,
  meetingClear,
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

const EMPTY_PROFILE: MeetingProfile = { about: "", projects: "", session_context: "", style: "" };
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

const QUESTION_START =
  /^(?:(?:so|and|okay|ok|alright|right|well|now|then|great|cool|um|uh)[,\s]+)*(?:what|what's|how|why|when|where|who|which|can you|could you|would you|will you|do you|did you|have you|are you|were you|is there|is it|tell me|walk me|talk me|describe|explain|give me|share|let's say|imagine|suppose)\b/;

/** Heuristic for "the other side just asked something" - drives auto-answer.
 *  Also checks the last sentence, since a question often follows a preamble
 *  ("We use Kafka a lot. How would you handle backpressure"). */
export function looksLikeQuestion(text: string) {
  const t = text.trim().toLowerCase();
  if (t.length < 8) return false;
  if (t.includes("?")) return true;
  const sentences = t.split(/(?<=[.!])\s+/);
  return QUESTION_START.test(t) || QUESTION_START.test(sentences[sentences.length - 1] ?? "");
}

/** The other side's latest turn - consecutive "them" lines joined, since
 *  Whisper often splits one question across two chunks - or null if the
 *  newest line isn't theirs. */
export function lastTheirTurn(transcript: TranscriptEntry[]): { id: number; text: string } | null {
  const last = transcript[transcript.length - 1];
  if (!last || last.speaker !== "them") return null;
  const parts: string[] = [];
  for (let i = transcript.length - 1; i >= 0 && transcript[i].speaker === "them" && parts.length < 3; i--) {
    parts.unshift(transcript[i].text);
  }
  return { id: last.id, text: parts.join(" ") };
}

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

  useEffect(() => {
    meetingStatus().then(setStatus).catch(() => {});
    meetingTranscript().then(setTranscript).catch(() => {});
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
  }, []);

  const start = useCallback(async (mode: MeetingMode, mic: boolean, system: boolean) => {
    setStarting(true);
    setError(null);
    try {
      setTranscript([]);
      setCards([]);
      setStatus(await meetingStart(mode, mic, system, copilotBarEnabled()));
    } catch (err) {
      setError(errText(err));
    } finally {
      setStarting(false);
    }
  }, []);

  const stop = useCallback(async () => {
    try {
      setStatus(await meetingStop());
    } catch (err) {
      setError(errText(err));
    }
  }, []);

  const assist = useCallback(async (action: MeetingAction, question: string | null = null) => {
    const id = newId();
    const label = action === "ask" && question ? question : ACTION_LABELS[action];
    const heard = action === "suggest" || action === "assist" ? lastTheirTurn(transcriptRef.current)?.text : undefined;
    const context = heard && heard.length > 140 ? `${heard.slice(0, 140)}…` : heard;
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
  const answeredRef = useRef(0);
  useEffect(() => {
    // While the in-call bar is up it does the auto-answering, right over the call.
    if (!autoAnswer || !status.live || status.mode === "practice" || copilotBarEnabled()) return;
    const turn = lastTheirTurn(transcript);
    if (!turn || turn.id <= answeredRef.current || !looksLikeQuestion(turn.text)) return;
    const t = setTimeout(() => {
      if (cardsRef.current.some((c) => c.pending)) return;
      answeredRef.current = turn.id;
      void assist("suggest");
    }, 250);
    return () => clearTimeout(t);
  }, [transcript, autoAnswer, status.live, status.mode, assist]);

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

  const saveNotes = useCallback(
    (notes: string, includeTranscript: boolean) => meetingSaveNotes(notes, includeTranscript),
    [],
  );

  return {
    status,
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
    assist,
    addEntry,
    clear,
    saveProfile,
    saveNotes,
  };
}
