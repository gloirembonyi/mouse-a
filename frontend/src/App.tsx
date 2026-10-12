import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { TitleBar } from "./components/layout/TitleBar";
import { AppShell } from "./components/layout/AppShell";
import { Sidebar } from "./components/layout/Sidebar";
import { AuthGate } from "./components/auth/AuthGate";
import { SettingsPanel } from "./components/settings/SettingsPanel";
import { ChatView } from "./components/chat/ChatView";
import { useAuth } from "./hooks/useAuth";
import { useConversations } from "./hooks/useConversations";
import { useChat } from "./hooks/useChat";
import { useWorkspace } from "./hooks/useWorkspace";
import { useMeeting } from "./hooks/useMeeting";
import { useAgents } from "./hooks/useAgents";
import {
  getAiActive,
  getConversationMessages,
  newConversation,
  appWindow,
  newId,
  onAiState,
  onMeetingHotkey,
  setConversationId,
  toggleAiActive,
} from "./lib/tauri";
import { threadTitle } from "./lib/format";
import type { Attachment, UiMessage } from "./lib/chatTypes";

const AgentsView = lazy(() => import("./components/agents/AgentsView").then((m) => ({ default: m.AgentsView })));
const MeetingView = lazy(() => import("./components/meeting/MeetingView").then((m) => ({ default: m.MeetingView })));

type View = "chat" | "settings" | "meeting" | "agents";

// The agent prompt is prefixed with the project folder (see useChat); keep
// that plumbing out of the transcript when a thread is reopened.
const stripProjectPrefix = (text: string) => text.replace(/^\[Project folder: [^\]]*\]\s*/, "");

export default function App() {
  const auth = useAuth();
  const conversations = useConversations();
  const workspace = useWorkspace();
  const chat = useChat({ onActivity: conversations.refresh, projectPath: workspace.activeProject?.path ?? null });
  const meeting = useMeeting();
  const agents = useAgents();

  const [view, setView] = useState<View>("chat");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [rightDockOpen, setRightDockOpen] = useState(false);
  const [aiActive, setAiActive] = useState(false);
  const [injected, setInjected] = useState<Attachment[]>([]);

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

  // Ctrl+Shift+Enter (global): during a live session, ask for a suggested
  // reply right away; otherwise just open the copilot.
  const meetingLiveRef = useRef(false);
  meetingLiveRef.current = meeting.status.live;
  const assistRef = useRef(meeting.assist);
  assistRef.current = meeting.assist;
  useEffect(() => {
    if (auth.status !== "signed-in") return;
    const unlisten = onMeetingHotkey(() => {
      setView("meeting");
      if (meetingLiveRef.current) void assistRef.current("suggest");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [auth.status]);

  // A browser-requested task is waiting for the user's Allow/Deny: bring the
  // window forward on the Agents view, or the request would silently time out.
  const prevWaiting = useRef(0);
  useEffect(() => {
    if (agents.waitingCount > prevWaiting.current) {
      setView("agents");
      appWindow.show().catch(() => {});
      appWindow.setFocus().catch(() => {});
    }
    prevWaiting.current = agents.waitingCount;
  }, [agents.waitingCount]);

  // Ctrl+B toggles the sidebar, Ctrl+J the editor/terminal dock.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey) return;
      const k = e.key.toLowerCase();
      if (k === "b") {
        e.preventDefault();
        setSidebarOpen((v) => !v);
      } else if (k === "j") {
        e.preventDefault();
        setRightDockOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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
          content: m.role === "assistant" ? m.content : stripProjectPrefix(m.content),
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

  const handleDeleteThread = useCallback(
    async (id: string) => {
      await conversations.remove(id);
      // The deleted thread was the one open in the chat view — clear it so
      // the next message doesn't try to continue a conversation that no
      // longer exists.
      if (conversations.activeId === id) {
        await handleNewChat();
      }
    },
    [conversations, handleNewChat],
  );

  const handleToggleHoverMode = useCallback(() => {
    toggleAiActive().catch(() => {});
  }, []);

  const handleAddProject = useCallback(() => {
    workspace.addFolder().catch(() => {});
  }, [workspace]);

  const handleAskAboutFile = useCallback((path: string, name: string) => {
    setInjected([{ id: newId(), kind: "file", path, name }]);
    setView("chat");
  }, []);

  const consumeInjected = useCallback(() => setInjected([]), []);

  const frame =
    "flex h-screen flex-col overflow-hidden rounded-[12px] border border-border-default bg-bg-primary text-text-primary";

  if (auth.status !== "signed-in") {
    return (
      <div className={frame}>
        <TitleBar aiActive={false} onToggleAi={() => {}} />
        <AuthGate status={auth.status} error={auth.error} onRetry={auth.retry} onSignedIn={auth.onSignedIn} />
      </div>
    );
  }

  const activeThread = conversations.threads.find((t) => t.id === conversations.activeId);
  const title =
    view === "meeting"
      ? "Meeting copilot"
      : view === "agents"
        ? "Agents"
        : view === "settings"
        ? "Settings"
        : activeThread
          ? threadTitle(activeThread.title)
          : chat.messages.length > 0
            ? "New chat"
            : undefined;

  return (
    <div className={frame}>
      <TitleBar
        aiActive={aiActive}
        onToggleAi={handleToggleHoverMode}
        title={title}
        sidebarOpen={sidebarOpen}
        onOpenSidebar={() => setSidebarOpen(true)}
        dockOpen={rightDockOpen}
        onToggleDock={() => setRightDockOpen((v) => !v)}
      />
      <div className="relative flex min-h-0 flex-1">
        <AppShell
          sidebarOpen={sidebarOpen}
          onCloseSidebar={() => setSidebarOpen(false)}
          rightDockOpen={rightDockOpen}
          onCloseRightDock={() => setRightDockOpen(false)}
          workspaceRoot={workspace.root}
          projectName={workspace.activeProject?.name ?? null}
          onAddProject={handleAddProject}
          onAskAboutFile={handleAskAboutFile}
          sidebar={
            <Sidebar
              user={auth.user}
              threads={conversations.threads}
              activeId={view === "chat" ? conversations.activeId : null}
              onNewChat={handleNewChat}
              onOpenThread={handleOpenThread}
              onDeleteThread={handleDeleteThread}
              onOpenSettings={() => setView(view === "settings" ? "chat" : "settings")}
              settingsActive={view === "settings"}
              onOpenMeeting={() => setView(view === "meeting" ? "chat" : "meeting")}
              meetingActive={view === "meeting"}
              meetingLive={meeting.status.live}
              onOpenAgents={() => setView(view === "agents" ? "chat" : "agents")}
              agentsActive={view === "agents"}
              agentsBusy={agents.openCount}
              agentsWaiting={agents.waitingCount}
              browserLinked={!!agents.info?.connected}
              onToggleHoverMode={handleToggleHoverMode}
              aiActive={aiActive}
              onToggleIdeDock={() => setRightDockOpen((v) => !v)}
              ideDockOpen={rightDockOpen}
              projects={workspace.projects}
              activeProjectPath={workspace.activeProject?.path ?? null}
              onSelectProject={workspace.selectProject}
              onAddProject={handleAddProject}
              onRemoveProject={workspace.removeProject}
              onCollapse={() => setSidebarOpen(false)}
            />
          }
        >
          {view === "agents" ? (
            <Suspense fallback={null}>
              <AgentsView agents={agents} onClose={() => setView("chat")} />
            </Suspense>
          ) : view === "meeting" ? (
            <Suspense fallback={null}>
              <MeetingView meeting={meeting} onClose={() => setView("chat")} />
            </Suspense>
          ) : view === "settings" ? (
            <SettingsPanel user={auth.user} agents={agents} onClose={() => setView("chat")} onSignedOut={auth.retry} />
          ) : (
            <ChatView
              chat={chat}
              aiActive={aiActive}
              workspaceRoot={workspace.root}
              conversations={conversations.threads}
              projects={workspace.projects}
              activeProject={workspace.activeProject}
              onSelectProject={workspace.selectProject}
              onAddProject={handleAddProject}
              onOpenMeeting={() => setView("meeting")}
              browserLinked={!!agents.info?.connected}
              injected={injected}
              onInjectedConsumed={consumeInjected}
            />
          )}
        </AppShell>
      </div>
    </div>
  );
}
