// Kudu Quill account sync: the personal dictionary and the picky / switched-off-rules prefs are the
// SAME data the browser extension keeps (backend /api/writing/dictionary and /prefs), so a word added
// in Chrome stops being flagged in a document here and the other way round.
//
// Offline-first: everything works from the local copy (store.ts). Syncing is a reconcile that runs at
// startup, when the connection returns, every few minutes and shortly after a local change. When the
// network or the sign-in is missing it fails quietly and tries again at the next trigger, so edits
// made offline are pushed later.
import { invoke } from "@tauri-apps/api/core";
import { getQuillPrefs, onLocalQuillChange, updateQuillPrefs } from "./store";

const api = <T>(method: string, path: string, body?: unknown) => invoke<T>("writing_api", { method, path, body });

interface RemotePrefs {
  prefs?: { picky?: boolean; disabledRules?: string[] };
}

const sameSet = (a: string[], b: string[]) => a.length === b.length && a.every((x) => b.includes(x));

let busy = false;

export async function reconcileQuill(): Promise<void> {
  if (busy) return;
  busy = true;
  try {
    const local = getQuillPrefs();

    // Dictionary: deletions made here go first, then the union, minus anything still pending removal.
    const remoteRes = await api<{ words?: { word: string }[] }>("GET", "/api/writing/dictionary");
    const remote = new Set((remoteRes.words ?? []).map((w) => String(w.word).toLowerCase()));
    const stillPending: string[] = [];
    for (const w of local.pendingRemove) {
      try {
        await api("DELETE", `/api/writing/dictionary/${encodeURIComponent(w)}`);
        remote.delete(w);
      } catch {
        stillPending.push(w);
      }
    }
    const toPush = local.dictionary.filter((w) => !remote.has(w));
    if (toPush.length) {
      try {
        await api("POST", "/api/writing/dictionary", { words: toPush });
        toPush.forEach((w) => remote.add(w));
      } catch {
        // Over the plan's word limit, or the connection dropped: keep the words locally.
      }
    }
    const dictionary = [...new Set([...local.dictionary, ...remote])].filter((w) => !stillPending.includes(w)).sort();

    // Prefs: whichever side changed last wins - this device while it has unpushed changes.
    let picky = local.picky;
    let disabledRules = local.disabledRules;
    let prefsDirty = local.prefsDirty;
    if (local.prefsDirty) {
      await api("PUT", "/api/writing/prefs", { picky: local.picky, disabledRules: local.disabledRules });
      prefsDirty = false;
    } else {
      const rp = await api<RemotePrefs>("GET", "/api/writing/prefs");
      if (typeof rp.prefs?.picky === "boolean") picky = rp.prefs.picky;
      if (Array.isArray(rp.prefs?.disabledRules)) disabledRules = rp.prefs.disabledRules.map(String);
    }

    const now = getQuillPrefs();
    const changed =
      !sameSet(now.dictionary, dictionary) || !sameSet(now.pendingRemove, stillPending) || now.picky !== picky ||
      !sameSet(now.disabledRules, disabledRules) || now.prefsDirty !== prefsDirty;
    if (changed) updateQuillPrefs({ dictionary, pendingRemove: stillPending, picky, disabledRules, prefsDirty }, true);
  } catch {
    // Offline, signed out or the server is down: the local copy keeps working; retry at the next trigger.
  } finally {
    busy = false;
  }
}

let started = false;
/** Safe to call from every window; only the first call per window starts the triggers. */
export function startQuillSync() {
  if (started) return;
  started = true;
  void reconcileQuill();
  window.addEventListener("online", () => void reconcileQuill());
  window.setInterval(() => void reconcileQuill(), 5 * 60 * 1000);
  let timer = 0;
  onLocalQuillChange(() => {
    clearTimeout(timer);
    timer = window.setTimeout(() => void reconcileQuill(), 2000);
  });
}
