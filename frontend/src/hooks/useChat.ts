import { useCallback, useEffect, useRef, useState } from "react";
import { agentRun, newId, onAgentProgress, sendChat } from "../lib/tauri";
import { friendlyError } from "../lib/format";
import { resolveAttachments } from "../lib/attachments";
import type { Attachment, UiMessage } from "../lib/chatTypes";
import type { AgentStep } from "../components/chat/AgentActivity";

interface UseChatOptions {
  onActivity?: () => void; // called whenever a run finishes (used to refresh the sidebar thread list)
}

export function useChat({ onActivity }: UseChatOptions = {}) {
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [agentMode, setAgentMode] = useState(true);
  const activeAgentMsgId = useRef<string | null>(null);
  const runStartedAt = useRef<number>(0);

  useEffect(() => {
    const unlisten = onAgentProgress((p) => {
      if (p.source && p.source !== "main") return;
      const id = activeAgentMsgId.current;
      if (!id) return;

      setMessages((prev) =>
        prev.map((m) => {
          if (m.id !== id || m.kind !== "agent") return m;
          if (p.kind === "done") return m;
          const step: AgentStep = { kind: p.kind, message: p.message ?? "", requestId: p.request_id };
          return { ...m, steps: [...m.steps, step] };
        }),
      );
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const clear = useCallback(() => setMessages([]), []);

  const addSystemMessage = useCallback((content: string) => {
    setMessages((prev) => [...prev, { id: newId(), kind: "system", content }]);
  }, []);

  const loadMessages = useCallback((msgs: UiMessage[]) => setMessages(msgs), []);

  const runAgent = useCallback(
    async (composedText: string, imageDataUrl: string | null) => {
      const id = newId();
      activeAgentMsgId.current = id;
      runStartedAt.current = Date.now();
      setMessages((prev) => [...prev, { id, kind: "agent", steps: [], status: "running" }]);
      setIsLoading(true);

      try {
        const result = await agentRun(composedText, "main", imageDataUrl);
        const elapsedMs = Date.now() - runStartedAt.current;
        setMessages((prev) => [
          ...prev.map((m) => (m.id === id && m.kind === "agent" ? { ...m, status: "done" as const, elapsedMs } : m)),
          { id: newId(), kind: "assistant", content: result.text || "(done)" },
        ]);
      } catch (err) {
        const elapsedMs = Date.now() - runStartedAt.current;
        setMessages((prev) => [
          ...prev.map((m) => (m.id === id && m.kind === "agent" ? { ...m, status: "failed" as const, elapsedMs } : m)),
          { id: newId(), kind: "assistant", content: `⚠️ ${friendlyError(err)}` },
        ]);
      } finally {
        setIsLoading(false);
        activeAgentMsgId.current = null;
        onActivity?.();
      }
    },
    [onActivity],
  );

  const runPlainChat = useCallback(async (composedText: string, imageDataUrl: string | null) => {
    const thinkingId = newId();
    setMessages((prev) => [...prev, { id: thinkingId, kind: "thinking" }]);
    setIsLoading(true);
    const shouldSearch =
      composedText.toLowerCase().startsWith("search") ||
      /latest|current|today|news/i.test(composedText);

    try {
      const result = await sendChat(composedText, shouldSearch, imageDataUrl);
      setMessages((prev) =>
        prev
          .filter((m) => m.id !== thinkingId)
          .concat({ id: newId(), kind: "assistant", content: result.text }),
      );
    } catch (err) {
      setMessages((prev) =>
        prev
          .filter((m) => m.id !== thinkingId)
          .concat({ id: newId(), kind: "assistant", content: `❌ Error: ${friendlyError(err)}` }),
      );
    } finally {
      setIsLoading(false);
    }
  }, []);

  const send = useCallback(
    async (text: string, attachments: Attachment[] = []) => {
      const trimmed = text.trim();
      if ((!trimmed && attachments.length === 0) || isLoading) return;
      setMessages((prev) => [
        ...prev,
        { id: newId(), kind: "user", content: trimmed, attachments: attachments.length ? attachments : undefined },
      ]);

      const { contextText, imageDataUrl } = await resolveAttachments(attachments);
      const composedText = contextText ? `${contextText}\n\n${trimmed}` : trimmed;

      if (agentMode) {
        await runAgent(composedText, imageDataUrl);
      } else {
        await runPlainChat(composedText, imageDataUrl);
      }
    },
    [isLoading, agentMode, runAgent, runPlainChat],
  );

  return { messages, isLoading, agentMode, setAgentMode, send, clear, addSystemMessage, loadMessages };
}
