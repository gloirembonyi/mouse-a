import { useCallback, useEffect, useState } from "react";
import { PanelLeftOpen } from "lucide-react";
import { TitleBar } from "./components/layout/TitleBar";
import { AppShell } from "./components/layout/AppShell";
import { AuthGate } from "./components/auth/AuthGate";
import { SettingsPanel } from "./components/settings/SettingsPanel";
import { ChatView } from "./components/chat/ChatView";
import { useAuth } from "./hooks/useAuth";
import { useConversations } from "./hooks/useConversations";
import { useChat } from "./hooks/useChat";
import { useWorkspace } from "./hooks/useWorkspace";
import {
  getAiActive,
  getConversationMessages,
  newConversation,
  newId,
  onAiState,
  setConversationId,
  toggleAiActive,
} from "./lib/tauri";
import type { UiMessage } from "./lib/chatTypes";

type View = "chat" | "settings";

export default function App() {
  const auth = useAuth();
  const conversations = useConversations();
  const chat = useChat({ onActivity: conversations.refresh });
  const workspace = useWorkspace();

  const [view, setView] = useState<View>("chat");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [rightDockOpen, setRightDockOpen] = useState(false);
  const [aiActive, setAiActive] = useState(false);

  useEffect(() => {
    if (auth.status !== "signed-in") return;
    conversations.refresh();
    getAiActive().then(setAiActive).catch(() => {});
    const unlisten = onAiState((active) => setAiActive(active));
    return () => {
      unlisten.then((fn) => fn());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auth.status]);

  const handleNewChat = useCallback(async () => {
    try {
      await newConversation();
      chat.clear();
      conversations.setActiveId(null);
      setView("chat");
    } catch {
      /* ignore */
    }
  }, [chat, conversations]);

  const handleOpenThread = useCallback(
    async (id: string) => {
      try {
        const msgs = await getConversationMessages(id, 200);
        await setConversationId(id);
        const uiMsgs: UiMessage[] = msgs.map((m) => ({
          id: newId(),
          kind: m.role === "assistant" ? "assistant" : "user",
          content: m.content,
        }));
        chat.loadMessages(uiMsgs);
        conversations.setActiveId(id);
        setView("chat");
      } catch (err) {
        chat.addSystemMessage(`Couldn't open that conversation: ${err instanceof Error ? err.message : String(err)}`);
      }
    },
    [chat, conversations],
  );

  const handleToggleHoverMode = useCallback(() => {
    toggleAiActive().catch(() => {});
  }, []);

  if (auth.status !== "signed-in") {
    return (
      <div className="flex h-screen flex-col overflow-hidden rounded-[14px] border border-border-default bg-bg-primary shadow-[inset_0_1px_0_rgba(255,255,255,0.06)]">
        <TitleBar aiActive={false} onToggleAi={() => {}} />
        <AuthGate status={auth.status} error={auth.error} onRetry={auth.retry} onSignedIn={auth.onSignedIn} />
      </div>
    );
  }

  return (
    <div className="flex h-screen flex-col overflow-hidden rounded-[14px] border border-border-default bg-bg-primary shadow-[inset_0_1px_0_rgba(255,255,255,0.06)]">
      <TitleBar aiActive={aiActive} onToggleAi={handleToggleHoverMode} />
      <div className="relative flex min-h-0 flex-1">
        {!sidebarOpen && (
          <button
            onClick={() => setSidebarOpen(true)}
            title="Open sidebar"
            className="absolute left-2 top-2 z-10 rounded-md border border-border-default bg-bg-secondary p-1.5 text-text-secondary shadow-md transition hover:text-text-primary"
          >
            <PanelLeftOpen size={15} />
          </button>
        )}
        <AppShell
          sidebarOpen={sidebarOpen}
          onCloseSidebar={() => setSidebarOpen(false)}
          rightDockOpen={rightDockOpen}
          onCloseRightDock={() => setRightDockOpen(false)}
          onToggleRightDock={() => setRightDockOpen((v) => !v)}
          workspaceRoot={workspace.root}
          onChangeWorkspaceRoot={workspace.setRoot}
          threads={conversations.threads}
          activeThreadId={conversations.activeId}
          onNewChat={handleNewChat}
          onOpenThread={handleOpenThread}
          onOpenSettings={() => setView("settings")}
          onToggleHoverMode={handleToggleHoverMode}
          aiActive={aiActive}
        >
          {view === "settings" ? (
            <SettingsPanel user={auth.user} onClose={() => setView("chat")} onSignedOut={auth.retry} />
          ) : (
            <ChatView chat={chat} aiActive={aiActive} onToggleHoverMode={handleToggleHoverMode} />
          )}
        </AppShell>
      </div>
    </div>
  );
}
