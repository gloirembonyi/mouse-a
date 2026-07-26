import { useCallback, useState } from "react";
import { getConversations } from "../lib/tauri";
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

  return { threads, activeId, setActiveId, refresh, error };
}
