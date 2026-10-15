import { useCallback, useEffect, useRef, useState } from "react";
import {
  meetingAddEntry,
  meetingAssist,
  meetingClear,
  meetingResearch,
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

const FILLER = "(?:(?:so|and|okay|ok|alright|right|well|now|then|great|cool|um|uh|yeah|yes|no|but|also|next|first|finally|lastly)[,\\s]+)*";
const QUESTION_START = new RegExp(
  "^" +
    FILLER +
    "(?:what|what's|whats|how|how's|why|when|where|who|who's|which|whose|can you|could you|would you|will you|do you|did you|does|have you|has|are you|were you|is there|is it|is that|isn't|aren't|don't you|shall we|should we|" +
    "tell me|tell us|walk me|walk us|take me|take us|talk me|run me|describe|explain|give me|give us|give an example|share|let's say|imagine|suppose|say|introduce|" +
    "any (?:experience|thoughts|questions|idea)|rate your|so your|your (?:biggest|greatest|main|last|current|most))\\b",
);
// Asks that sit mid-sentence or lack a question word: "I'd like to know how…",
// "we're curious about…", "you've worked with Kafka, right".
const QUESTION_ANYWHERE =
  /\b(?:i(?:'d| would) like to (?:know|hear|understand)|i want to (?:know|hear|understand)|i'?m curious|we'?re curious|curious (?:to|about|how|what|if)|can you (?:tell|walk|talk|explain|describe|share|give)|could you (?:tell|walk|talk|explain|describe|share|give)|tell (?:me|us) (?:about|a bit|a little|more|how|why|what)|walk (?:me|us) through|a time when|an example of|what would you|how would you|how do you|how did you|how have you|why did you|why do you|why would you|what do you think|what are your|what is your|what's your|do you have (?:any|experience)|have you (?:ever|worked|used|done|built)|are you familiar|how familiar|how comfortable|how long|how many|how much|where do you see|why should we|why do you want|what interests you|biggest (?:challenge|weakness|strength|achievement)|questions for (?:me|us)|right\??$|correct\??$|isn'?t it\??$|don'?t you think\??$|would you say)\b/;

/** Heuristic for "the other side just asked something" - drives auto-answer.
 *  Speech recognition rarely emits a "?", so the shape of the sentence has to do:
 *  question openers (with filler before them), interview prompts ("tell me about a
 *  time", "walk us through"), and asks buried mid-sentence ("I'd like to know how…").
 *  The last sentence is checked on its own, since a question often follows a
 *  preamble ("We use Kafka a lot. How would you handle backpressure"). */
export function looksLikeQuestion(text: string) {
  const t = text.trim().toLowerCase().replace(/\s+/g, " ");
  if (t.length < 8) return false;
  if (t.includes("?")) return true;
  const sentences = t.split(/(?<=[.!])\s+/).filter(Boolean);
  const last = sentences[sentences.length - 1] ?? t;
  const tail = sentences.slice(-2).join(" ");
  return QUESTION_START.test(t) || QUESTION_START.test(last) || QUESTION_ANYWHERE.test(tail);
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
