import { useCallback, useEffect, useRef, useState } from "react";
import { agentCancel, agentRun, newId, onAgentProgress, sendChat } from "../lib/tauri";
import { friendlyError } from "../lib/format";
import { resolveAttachments } from "../lib/attachments";
import type { Attachment, UiMessage } from "../lib/chatTypes";
import type { AgentStep } from "../components/chat/AgentActivity";

interface UseChatOptions {
  onActivity?: () => void; // called whenever a run finishes (used to refresh the sidebar thread list)
  /** Folder the agent should work in (the active project), if any. */
  projectPath?: string | null;
}

const AGENT_MODE_KEY = "mouse-ai-agent-mode";

export function useChat({ onActivity, projectPath = null }: UseChatOptions = {}) {
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [agentMode, setAgentModeState] = useState(() => localStorage.getItem(AGENT_MODE_KEY) !== "off");
  const activeAgentMsgId = useRef<string | null>(null);
  const runStartedAt = useRef<number>(0);
  const projectRef = useRef(projectPath);
  projectRef.current = projectPath;

  const setAgentMode = useCallback((v: boolean) => {
    localStorage.setItem(AGENT_MODE_KEY, v ? "on" : "off");
    setAgentModeState(v);
  }, []);

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
  const appendMessage = useCallback((msg: UiMessage) => setMessages((prev) => [...prev, msg]), []);
  const removeMessage = useCallback((id: string) => setMessages((prev) => prev.filter((m) => m.id !== id)), []);

  /** Stops the running agent task (takes effect at its next step). */
  const stop = useCallback(() => {
    agentCancel("main").catch(() => {});
  }, []);

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

  const runPlainChat = useCallback(
    async (composedText: string, imageDataUrl: string | null) => {
      const thinkingId = newId();
      setMessages((prev) => [...prev, { id: thinkingId, kind: "thinking" }]);
      setIsLoading(true);
      const shouldSearch =
        composedText.toLowerCase().startsWith("search") || /latest|current|today|news/i.test(composedText);

      try {
        const result = await sendChat(composedText, shouldSearch, imageDataUrl);
        setMessages((prev) =>
          prev.filter((m) => m.id !== thinkingId).concat({ id: newId(), kind: "assistant", content: result.text }),
        );
      } catch (err) {
        setMessages((prev) =>
          prev
            .filter((m) => m.id !== thinkingId)
            .concat({ id: newId(), kind: "assistant", content: `⚠️ ${friendlyError(err)}` }),
        );
      } finally {
        setIsLoading(false);
        onActivity?.();
      }
    },
    [onActivity],
  );

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
        // The agent's relative paths resolve into a scratch dir (agent.rs), so
        // the project folder has to be spelled out for it to work there.
        const project = projectRef.current;
        const withProject = project
          ? `[Project folder: ${project} — work inside this folder and use absolute paths under it for files and shell commands.]\n\n${composedText}`
          : composedText;
        await runAgent(withProject, imageDataUrl);
      } else {
        await runPlainChat(composedText, imageDataUrl);
      }
    },
    [isLoading, agentMode, runAgent, runPlainChat],
  );

  return {
    messages,
    isLoading,
    agentMode,
    setAgentMode,
    send,
    stop,
    clear,
    addSystemMessage,
    loadMessages,
    appendMessage,
    removeMessage,
  };
}
