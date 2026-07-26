import { useCallback, useState } from "react";
import { deleteConversation, getConversations } from "../lib/tauri";
import type { Conversation } from "../lib/types";

export function useConversations() {
  const [threads, setThreads] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const conversations = await getConversations(50);
      setThreads(conversations ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

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
