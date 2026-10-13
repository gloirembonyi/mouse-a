import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { agentCancel, agentRun, newId, onAgentProgress, sendChat, setConversationId } from "../lib/tauri";
import { friendlyError } from "../lib/format";
import { resolveAttachments } from "../lib/attachments";
import type { Attachment, UiMessage } from "../lib/chatTypes";
import type { AgentStep } from "../components/chat/AgentActivity";

interface UseChatOptions {
  onActivity?: () => void; // called whenever a run finishes (used to refresh the sidebar thread list)
  /** Folder the agent should work in (the active project), if any. */
  projectPath?: string | null;
}

/** One chat: its transcript and (at most) one in-flight run. Sessions live
 *  in memory, so a run keeps going - and keeps receiving its steps - while
 *  the user is looking at a different chat, and is still there when they
 *  come back. */
export interface ChatSession {
  key: string;
  messages: UiMessage[];
  running: boolean;
  /** The agent message receiving live steps, while running. */
  agentMsgId: string | null;
  startedAt: number;
  /** Backend conversation this chat is logged to (null until the first reply). */
  conversationId: string | null;
  title: string;
  /** Finished while the user was looking elsewhere. */
  unseen: boolean;
  /** The run is waiting on an Allow/Deny prompt. */
  needsYou: boolean;
}

const AGENT_MODE_KEY = "mouse-ai-agent-mode";
const SOURCE_PREFIX = "main:";

const emptySession = (conversationId: string | null = null, messages: UiMessage[] = []): ChatSession => ({
  key: newId(),
  messages,
  running: false,
  agentMsgId: null,
  startedAt: 0,
  conversationId,
  title: "",
  unseen: false,
  needsYou: false,
});

function titleFrom(text: string) {
  const t = text.replace(/\s+/g, " ").trim();
  return t.length > 60 ? `${t.slice(0, 59)}…` : t || "New chat";
}

export function useChat({ onActivity, projectPath = null }: UseChatOptions = {}) {
  const [sessions, setSessions] = useState<Record<string, ChatSession>>(() => {
    const s = emptySession();
    return { [s.key]: s };
  });
  const [currentKey, setCurrentKey] = useState<string>(() => Object.keys(sessions)[0]);
  const [agentMode, setAgentModeState] = useState(() => localStorage.getItem(AGENT_MODE_KEY) !== "off");
  const projectRef = useRef(projectPath);
  projectRef.current = projectPath;
  const currentRef = useRef(currentKey);
  currentRef.current = currentKey;
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const onActivityRef = useRef(onActivity);
  onActivityRef.current = onActivity;

  const setAgentMode = useCallback((v: boolean) => {
    localStorage.setItem(AGENT_MODE_KEY, v ? "on" : "off");
    setAgentModeState(v);
  }, []);

  const update = useCallback((key: string, fn: (s: ChatSession) => ChatSession) => {
    setSessions((prev) => (prev[key] ? { ...prev, [key]: fn(prev[key]) } : prev));
  }, []);

  // Live agent steps → whichever session started that run ("main:<key>").
  useEffect(() => {
    const unlisten = onAgentProgress((p) => {
      if (!p.source?.startsWith(SOURCE_PREFIX) || p.kind === "done") return;
      const key = p.source.slice(SOURCE_PREFIX.length);
      setSessions((prev) => {
        const s = prev[key];
        if (!s?.agentMsgId) return prev;
        const step: AgentStep = { kind: p.kind, message: p.message ?? "", requestId: p.request_id };
        return {
          ...prev,
          [key]: {
            ...s,
            needsYou: p.kind === "confirm",
            messages: s.messages.map((m) => (m.id === s.agentMsgId && m.kind === "agent" ? { ...m, steps: [...m.steps, step] } : m)),
          },
        };
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const current = sessions[currentKey];

  /** Start a fresh chat. Running chats keep going in the background. */
  const clear = useCallback(() => {
    const s = emptySession();
    setSessions((prev) => {
      // Drop idle, empty or long-unvisited chats so memory doesn't grow forever.
      const kept = Object.values(prev).filter((x) => x.running || x.unseen || x.messages.length > 0);
      const trimmed = kept.length > 20 ? kept.filter((x) => x.running || x.unseen).slice(-20) : kept;
      return Object.fromEntries([...trimmed, s].map((x) => [x.key, x]));
    });
    setCurrentKey(s.key);
  }, []);

  /** Show a chat loaded from history (replacing the view, not other sessions). */
  const loadMessages = useCallback((msgs: UiMessage[], conversationId: string | null = null) => {
    const firstUser = msgs.find((m) => m.kind === "user");
    const firstText = firstUser && "content" in firstUser ? firstUser.content : "";
    const s = { ...emptySession(conversationId, msgs), title: titleFrom(firstText) };
    setSessions((prev) => ({ ...prev, [s.key]: s }));
    setCurrentKey(s.key);
  }, []);

  /** If a live session already holds this conversation, switch to it. */
  const openConversation = useCallback((conversationId: string) => {
    const hit = Object.values(sessionsRef.current).find((s) => s.conversationId === conversationId);
    if (!hit) return false;
    setCurrentKey(hit.key);
    update(hit.key, (s) => ({ ...s, unseen: false }));
    return true;
  }, [update]);

  const openSession = useCallback((key: string) => {
    if (!sessionsRef.current[key]) return;
    setCurrentKey(key);
    update(key, (s) => ({ ...s, unseen: false }));
    void setConversationId(sessionsRef.current[key].conversationId).catch(() => {});
  }, [update]);

  const addSystemMessage = useCallback(
    (content: string) => update(currentRef.current, (s) => ({ ...s, messages: [...s.messages, { id: newId(), kind: "system", content }] })),
    [update],
  );
  const appendMessage = useCallback(
    (msg: UiMessage) => update(currentRef.current, (s) => ({ ...s, messages: [...s.messages, msg] })),
    [update],
  );
  const removeMessage = useCallback(
    (id: string) => update(currentRef.current, (s) => ({ ...s, messages: s.messages.filter((m) => m.id !== id) })),
    [update],
  );

  /** Stops the current chat's run (takes effect at its next step). */
  const stop = useCallback(() => {
    agentCancel(SOURCE_PREFIX + currentRef.current).catch(() => {});
  }, []);

  const finish = useCallback(
    (key: string, patch: (s: ChatSession) => ChatSession) => {
      update(key, (s) => ({ ...patch(s), running: false, agentMsgId: null, needsYou: false, unseen: currentRef.current !== key }));
      onActivityRef.current?.();
    },
    [update],
  );

  const send = useCallback(
    async (text: string, attachments: Attachment[] = []) => {
      const key = currentRef.current;
      const session = sessionsRef.current[key];
      const trimmed = text.trim();
      if (!session || (!trimmed && attachments.length === 0) || session.running) return;

      const agentMsgId = agentMode ? newId() : null;
      const thinkingId = newId();
      update(key, (s) => ({
        ...s,
        running: true,
        agentMsgId,
        startedAt: Date.now(),
        title: s.title || titleFrom(trimmed),
        messages: [
          ...s.messages,
          { id: newId(), kind: "user", content: trimmed, attachments: attachments.length ? attachments : undefined },
          agentMsgId ? { id: agentMsgId, kind: "agent", steps: [], status: "running" } : { id: thinkingId, kind: "thinking" },
        ],
      }));

      // The backend conversation is per-app state; point it at THIS chat
      // right before starting (the user may have been in another chat).
      await setConversationId(session.conversationId).catch(() => {});

      const { contextText, imageDataUrl } = await resolveAttachments(attachments);
      const composedText = contextText ? `${contextText}\n\n${trimmed}` : trimmed;
      const startedAt = Date.now();

      if (agentMode) {
        // The agent's relative paths resolve into a scratch dir (agent.rs), so
        // the project folder has to be spelled out for it to work there.
        const project = projectRef.current;
        const task = project
          ? `[Project folder: ${project} - work inside this folder and use absolute paths under it for files and shell commands.]\n\n${composedText}`
          : composedText;
        try {
          const result = await agentRun(task, SOURCE_PREFIX + key, imageDataUrl);
          const elapsedMs = Date.now() - startedAt;
          finish(key, (s) => ({
            ...s,
            conversationId: result.conversation_id ?? s.conversationId,
            messages: [
              ...s.messages.map((m) => (m.id === agentMsgId && m.kind === "agent" ? { ...m, status: "done" as const, elapsedMs } : m)),
              { id: newId(), kind: "assistant", content: result.text || "(done)" },
            ],
          }));
        } catch (err) {
          const elapsedMs = Date.now() - startedAt;
          finish(key, (s) => ({
            ...s,
            messages: [
              ...s.messages.map((m) => (m.id === agentMsgId && m.kind === "agent" ? { ...m, status: "failed" as const, elapsedMs } : m)),
              { id: newId(), kind: "assistant", content: `⚠️ ${friendlyError(err)}` },
            ],
          }));
        }
      } else {
        const shouldSearch = composedText.toLowerCase().startsWith("search") || /latest|current|today|news/i.test(composedText);
        try {
          const result = await sendChat(composedText, shouldSearch, imageDataUrl);
          finish(key, (s) => ({
            ...s,
            conversationId: result.conversation_id ?? s.conversationId,
            messages: s.messages.filter((m) => m.id !== thinkingId).concat({ id: newId(), kind: "assistant", content: result.text }),
          }));
        } catch (err) {
          finish(key, (s) => ({
            ...s,
            messages: s.messages
              .filter((m) => m.id !== thinkingId)
              .concat({ id: newId(), kind: "assistant", content: `⚠️ ${friendlyError(err)}` }),
          }));
        }
      }
    },
    [agentMode, update, finish],
  );

  /** Chats with a run going, or that finished while you were elsewhere. */
  const backgroundSessions = useMemo(
    () =>
      Object.values(sessions)
        .filter((s) => s.key !== currentKey && (s.running || s.unseen))
        .sort((a, b) => Number(b.needsYou) - Number(a.needsYou)),
    [sessions, currentKey],
  );

  return {
    messages: current?.messages ?? [],
    isLoading: !!current?.running,
    currentKey,
    currentConversationId: current?.conversationId ?? null,
    agentMode,
    setAgentMode,
    send,
    stop,
    clear,
    addSystemMessage,
    loadMessages,
    appendMessage,
    removeMessage,
    openConversation,
    openSession,
    backgroundSessions,
    runningConversationIds: Object.values(sessions)
      .filter((s) => s.running && s.conversationId)
      .map((s) => s.conversationId as string),
  };
}
