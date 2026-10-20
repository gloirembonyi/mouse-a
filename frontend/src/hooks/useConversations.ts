import { useCallback, useEffect, useRef, useState } from "react";
import { deleteConversation, getConversations, getHistoryChanges } from "../lib/tauri";
import type { Conversation } from "../lib/types";

/** Kudu Core's placeholder until the first exchange has been titled. */
const DEFAULT_TITLE = /^(thothai|kudu) chat$/i;
const TITLE_RECHECKS_MS = [3500, 9000, 18000];
/** One history across apps: chats from the browser extension (and deletes made there)
 *  arrive through /history/changes - polled while this window is visible. */
const SYNC_EVERY_MS = 20_000;

function byUpdatedDesc(a: Conversation, b: Conversation) {
  return new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime();
}

export function useConversations() {
  const [threads, setThreads] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const recheck = useRef<{ timer: ReturnType<typeof setTimeout> | null; tries: number }>({ timer: null, tries: 0 });
  const cursor = useRef<string | null>(null);

  const refresh = useCallback(async (fromRecheck = false) => {
    try {
      const conversations = (await getConversations(50)) ?? [];
      setThreads(conversations);
      setError(null);
      // The server names a chat right AFTER its first answer, so the refresh that follows
      // a reply usually still sees the placeholder. Look again a few times while a chat
      // touched in the last few minutes is unnamed - then it shows its real title.
      const r = recheck.current;
      if (!fromRecheck) r.tries = 0;
      const recent = Date.now() - 5 * 60 * 1000;
      const pending = conversations.some(
        (c) => DEFAULT_TITLE.test((c.title || "").trim()) && new Date(c.updated_at ?? 0).getTime() > recent,
      );
      if (r.timer) clearTimeout(r.timer);
      r.timer = null;
      if (pending && r.tries < TITLE_RECHECKS_MS.length) {
        const delay = TITLE_RECHECKS_MS[r.tries++];
        r.timer = setTimeout(() => void refresh(true), delay);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  /** Pull what changed since the last sync and merge it in (new, updated, renamed, deleted). */
  const sync = useCallback(async () => {
    try {
      const out = await getHistoryChanges(cursor.current);
      const first = cursor.current === null;
      cursor.current = out.now;
      if (first) return; // the initial list comes from refresh(); from now on only deltas
      if (!out.conversations.length && !out.deleted.length) return;
      const gone = new Set(out.deleted);
      setThreads((prev) => {
        const map = new Map(prev.map((t) => [t.id, t]));
        for (const id of gone) map.delete(id);
        for (const c of out.conversations) map.set(c.id, { ...map.get(c.id), ...c });
        return [...map.values()].sort(byUpdatedDesc).slice(0, 100);
      });
    } catch {
      // Offline / signed out: the next tick tries again.
    }
  }, []);

  useEffect(() => {
    void sync();
    const tick = () => {
      if (document.visibilityState === "visible") void sync();
    };
    const timer = setInterval(tick, SYNC_EVERY_MS);
    const onFocus = () => void sync();
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [sync]);

  // Optimistic: drop it from the list immediately rather than waiting on a
  // refetch, then reconcile with the server in the background in case the
  // delete actually failed.
  const remove = useCallback(
    async (id: string) => {
      setThreads((prev) => prev.filter((t) => t.id !== id));
      try {
        await deleteConversation(id);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        refresh();
      }
    },
    [refresh],
  );

  return { threads, activeId, setActiveId, refresh, remove, error };
}
