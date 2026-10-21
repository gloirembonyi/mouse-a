// Kudu Quill preferences on this device: the personal dictionary, ignored issues, switched-off
// rules and picky mode. Kept in localStorage so they work offline and survive restarts; the account
// sync (backend /api/writing/dictionary + /prefs, and the paired browser extension) layers on top of
// this in a later step - the shape deliberately matches the server's prefs.
import { useSyncExternalStore } from "react";
import type { QuillOptions } from "./engine";

export interface QuillPrefs {
  enabled: boolean;
  picky: boolean;
  /** Lowercased words. */
  dictionary: string[];
  /** Rule ids the user switched off. */
  disabledRules: string[];
  /** "ruleId|lowercased text" - this exact issue, wherever it appears. */
  ignored: string[];
  /** Words removed here that the account still has; the next sync deletes them there too. */
  pendingRemove: string[];
  /** Picky/rules changed here and not yet pushed: this device wins the next sync. */
  prefsDirty: boolean;
}

const KEY = "mouse-ai-quill";
const DEFAULTS: QuillPrefs = { enabled: true, picky: false, dictionary: [], disabledRules: [], ignored: [], pendingRemove: [], prefsDirty: false };

function read(): QuillPrefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return DEFAULTS;
    const p = JSON.parse(raw) as Partial<QuillPrefs>;
    return {
      enabled: p.enabled !== false,
      picky: p.picky === true,
      dictionary: Array.isArray(p.dictionary) ? p.dictionary.map(String) : [],
      disabledRules: Array.isArray(p.disabledRules) ? p.disabledRules.map(String) : [],
      ignored: Array.isArray(p.ignored) ? p.ignored.map(String) : [],
      pendingRemove: Array.isArray(p.pendingRemove) ? p.pendingRemove.map(String) : [],
      prefsDirty: p.prefsDirty === true,
    };
  } catch {
    return DEFAULTS;
  }
}

let state = read();
const listeners = new Set<() => void>();

export function getQuillPrefs(): QuillPrefs {
  return state;
}

const localListeners = new Set<() => void>();
/** Fires when the USER changed something (not when a sync did) - sync.ts pushes it to the account. */
export const onLocalQuillChange = (cb: () => void) => {
  localListeners.add(cb);
  return () => { localListeners.delete(cb); };
};

/** `fromSync` = the account's values arriving: no dirty flag, no push back. */
export function updateQuillPrefs(patch: Partial<QuillPrefs>, fromSync = false) {
  const next = { ...state, ...patch };
  if (!fromSync && ("picky" in patch || "disabledRules" in patch)) next.prefsDirty = true;
  state = next;
  try { localStorage.setItem(KEY, JSON.stringify(state)); } catch { /* storage full or blocked */ }
  for (const l of listeners) l();
  if (!fromSync) for (const l of localListeners) l();
}

// Another window (the document editor, the Quill popup) changed the prefs: pick it up.
if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== KEY) return;
    state = read();
    for (const l of listeners) l();
  });
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => { listeners.delete(l); };
};

export const useQuillPrefs = () => useSyncExternalStore(subscribe, getQuillPrefs, getQuillPrefs);

export const toOptions = (p: QuillPrefs): QuillOptions => ({
  picky: p.picky,
  disabledRules: p.disabledRules,
  disabledCategories: [],
  dictionary: new Set(p.dictionary),
});

export const ignoreKey = (id: string, text: string) => `${id}|${text.toLowerCase()}`;

export const addWord = (word: string) => {
  const w = word.trim().toLowerCase();
  if (w && !state.dictionary.includes(w)) updateQuillPrefs({ dictionary: [...state.dictionary, w], pendingRemove: state.pendingRemove.filter((x) => x !== w) });
};
export const removeWord = (word: string) => {
  const w = word.trim().toLowerCase();
  updateQuillPrefs({ dictionary: state.dictionary.filter((x) => x !== w), pendingRemove: state.pendingRemove.includes(w) ? state.pendingRemove : [...state.pendingRemove, w] });
};
export const ignoreIssue = (id: string, text: string) => {
  const k = ignoreKey(id, text);
  if (!state.ignored.includes(k)) updateQuillPrefs({ ignored: [...state.ignored, k] });
};
export const disableRule = (id: string) => {
  if (!state.disabledRules.includes(id)) updateQuillPrefs({ disabledRules: [...state.disabledRules, id] });
};
