import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { MessageList } from "./MessageList";
import { InputBar } from "./InputBar";
import { captureAnalyze, getCursorContext, newId } from "../../lib/tauri";
import type { useChat } from "../../hooks/useChat";
import type { Conversation, CursorContext } from "../../lib/types";

interface ChatViewProps {
  chat: ReturnType<typeof useChat>;
  aiActive: boolean;
  onToggleHoverMode: () => void;
  workspaceRoot: string | null;
  conversations: Conversation[];
}

export function ChatView({ chat, aiActive, onToggleHoverMode, workspaceRoot, conversations }: ChatViewProps) {
  const [context, setContext] = useState<CursorContext | null>(null);
  const lastKeyRef = useRef("");

  useEffect(() => {
    if (!aiActive) {
      setContext(null);
      return;
    }
    // Each tick crosses into Win32 UI Automation through Tauri, so it must not keep
    // running while the window is hidden — there is no cursor context worth reading
    // when nobody is looking at the app.
    const poll = async () => {
      try {
        const ctx = await getCursorContext();
        const key = `${ctx.process_name}|${ctx.window_title}`;
        if (key !== lastKeyRef.current && ctx.process_name) {
          lastKeyRef.current = key;
          setContext(ctx);
        }
      } catch {
        /* ignore */
      }
    };
    let interval: ReturnType<typeof setInterval> | null = null;
    const start = () => { if (!interval) { void poll(); interval = setInterval(poll, 1000); } };
    const stop = () => { if (interval) { clearInterval(interval); interval = null; } };
    const onVisibility = () => (document.hidden ? stop() : start());
    if (!document.hidden) start();
    document.addEventListener("visibilitychange", onVisibility);
    return () => { stop(); document.removeEventListener("visibilitychange", onVisibility); };
  }, [aiActive]);

  const handleQuickAction = async (action: "capture" | "ask" | "search") => {
    if (action === "ask") return; // just focuses the input in the vanilla version; textarea auto-focus isn't critical here
    if (action === "search") {
      chat.send("Search the web for: ");
      return;
    }
    if (action === "capture") {
      chat.addSystemMessage("📸 Capturing screen around cursor...");
      try {
        const result = await captureAnalyze();
        chat.loadMessages([
          ...chat.messages,
          { id: newId(), kind: "assistant", content: result.text },
        ]);
      } catch (err) {
        chat.loadMessages([
          ...chat.messages,
          { id: newId(), kind: "assistant", content: `❌ Capture error: ${err}` },
        ]);
      }
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      {context && (
        <div className="flex flex-shrink-0 items-center justify-between border-b border-accent/20 bg-accent-soft px-3.5 py-1.5">
          <div className="flex items-center gap-2 overflow-hidden text-[11px]">
            <span className="font-semibold text-accent">{context.process_name}</span>
            <span className="truncate text-text-muted">{context.window_title}</span>
          </div>
          <button onClick={() => setContext(null)} className="text-text-muted hover:text-text-primary">
            <X size={12} />
          </button>
        </div>
      )}

      <MessageList messages={chat.messages} onQuickAction={handleQuickAction} />

      <InputBar
        agentMode={chat.agentMode}
        onToggleAgentMode={() => chat.setAgentMode((v) => !v)}
        onSend={chat.send}
        disabled={chat.isLoading}
        onToggleHoverMode={onToggleHoverMode}
        workspaceRoot={workspaceRoot}
        conversations={conversations}
      />
    </div>
  );
}
