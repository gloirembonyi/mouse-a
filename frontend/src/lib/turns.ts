import type { TranscriptEntry } from "./tauri";

/** Speech recognition cuts a sentence wherever the speaker pauses, so the raw
 *  transcript is a list of fragments. A "turn" is what a person would call one
 *  thing said: consecutive lines from the same side, close together in time. */
export interface Turn {
  /** Id of the FIRST line - stable while the turn grows, so it keys React lists and "already answered". */
  id: number;
  /** Id of the newest line in the turn. */
  lastId: number;
  speaker: TranscriptEntry["speaker"];
  text: string;
  at_ms: number;
}

/** Lines further apart than this are separate turns (the speaker finished and later resumed). */
export const TURN_GAP_MS = 3_000;
/** How long the other side must be quiet before their turn counts as finished. */
export const TURN_QUIET_MS = 900;

const MERGEABLE = new Set<TranscriptEntry["speaker"]>(["them", "me"]);

export function mergeTurns(entries: TranscriptEntry[]): Turn[] {
  const turns: Turn[] = [];
  for (const e of entries) {
    const prev = turns[turns.length - 1];
    if (prev && MERGEABLE.has(e.speaker) && prev.speaker === e.speaker && e.at_ms - prev.at_ms <= TURN_GAP_MS) {
      prev.text = `${prev.text} ${e.text}`.replace(/\s+/g, " ").trim();
      prev.lastId = e.id;
      prev.at_ms = e.at_ms;
    } else {
      turns.push({ id: e.id, lastId: e.id, speaker: e.speaker, text: e.text, at_ms: e.at_ms });
    }
  }
  return turns;
}

/** The other side's newest turn, or null when the newest line is not theirs. */
export function lastTheirTurn(entries: TranscriptEntry[]): Turn | null {
  const turns = mergeTurns(entries);
  const last = turns[turns.length - 1];
  return last && last.speaker === "them" ? last : null;
}

/** The newest THEM turn that looks like a question and was not answered by a later ME turn. */
export function openQuestion(entries: TranscriptEntry[]): Turn | null {
  const turns = mergeTurns(entries);
  for (let i = turns.length - 1; i >= 0; i--) {
    const t = turns[i];
    if (t.speaker === "me" && t.text.split(/\s+/).length >= 6) return null; // they already replied
    if (t.speaker === "them" && looksLikeQuestion(t.text)) return t;
  }
  return null;
}

export interface AnsweredMark {
  id: number;
  len: number;
  count: number;
}

/** A turn needs an answer when it is newer than the last answered one, or it grew a lot after
 *  being answered (the first answer was based on half the question) - at most one re-answer. */
export function needsAnswer(turn: Turn, mark: AnsweredMark | null) {
  if (!mark || turn.id > mark.id) return true;
  return turn.id === mark.id && mark.count < 2 && turn.text.length > mark.len + 25;
}

// A sentence that stops on one of these is cut off mid-thought ("…is going to", "…and the").
const DANGLING =
  /\b(?:is going to|are going to|going to|gonna|and|or|but|so|the|a|an|of|that|which|because|if|when|then|than|as|by|into|about|my|your|our|their)$/;

/** True when the words read as a finished thought, not a cut-off fragment. */
export function looksComplete(text: string) {
  const t = text.trim().toLowerCase().replace(/[,;:\s]+$/, "");
  if (!t) return false;
  if (/[?.!]$/.test(t)) return true;
  return !DANGLING.test(t);
}

const FILLER = "(?:(?:so|and|okay|ok|alright|right|well|now|then|great|cool|um|uh|yeah|yes|no|but|also|next|first|finally|lastly)[,\\s]+)*";
const AUX = "(?:did|do|does|is|are|was|were|would|will|can|could|should|have|has|had|'s|'re)";
const QUESTION_START = new RegExp(
  "^" +
    FILLER +
    "(?:what|what's|whats|how|how's|why|who|who's|which|whose|" +
    `(?:when|where)\\s+${AUX}|` +
    "can you|could you|would you|will you|do you|did you|does (?:it|that|this|the|your|a|an)|have you|has (?:it|that|this|the|your)|are you|were you|is there|is it|is that|isn't|aren't|don't you|shall we|should we|" +
    "tell me|tell us|walk me|walk us|take me|take us|talk me|run me|describe|explain|give me|give us|give an example|" +
    "any (?:experience|thoughts|questions|idea)|rate your|so your|your (?:biggest|greatest|main|last|current|most))\\b",
);
// Asks that sit mid-sentence or lack a question word: "I'd like to know how…",
// "we're curious about…", "you've worked with Kafka, right".
const QUESTION_ANYWHERE =
  /\b(?:i(?:'d| would) like to (?:know|hear|understand)|i want to (?:know|hear|understand)|i'?m curious|we'?re curious|curious (?:to|about|how|what|if)|can you (?:tell|walk|talk|explain|describe|share|give)|could you (?:tell|walk|talk|explain|describe|share|give)|tell (?:me|us) (?:about|a bit|a little|more|how|why|what)|walk (?:me|us) through|a time when|an example of|what would you|how would you|how do you|how did you|how have you|why did you|why do you|why would you|what do you think|what are your|what is your|what's your|do you have (?:any|experience)|have you (?:ever|worked|used|done|built)|are you familiar|how familiar|how comfortable|where do you see|why should we|why do you want|what interests you|biggest (?:challenge|weakness|strength|achievement)|questions for (?:me|us)|right\??$|correct\??$|isn'?t it\??$|don'?t you think\??$|would you say)\b/;

/** "The other side just asked me something." Speech recognition rarely emits a "?",
 *  so the shape of the sentence has to do: question openers, interview prompts and
 *  asks buried mid-sentence - but never a cut-off fragment, and a lecture clause
 *  like "When we execute this query, the engine…" is not a question. */
export function looksLikeQuestion(text: string) {
  const t = text.trim().toLowerCase().replace(/\s+/g, " ");
  if (t.length < 8) return false;
  if (t.endsWith("?")) return true;
  if (!looksComplete(t)) return false;
  if (t.includes("?")) return true;
  const sentences = t.split(/(?<=[.!])\s+/).filter(Boolean);
  const last = sentences[sentences.length - 1] ?? t;
  const tail = sentences.slice(-2).join(" ");
  return QUESTION_START.test(t) || QUESTION_START.test(last) || QUESTION_ANYWHERE.test(tail);
}
