import { useEffect, useRef, useState } from "react";
import { AppWindow, Camera, FolderSearch, Globe, Headphones, X } from "lucide-react";
import { MessageList } from "./MessageList";
import { InputBar } from "./InputBar";
import { captureAnalyze, getCursorContext, newId } from "../../lib/tauri";
import { friendlyError } from "../../lib/format";
import { Kbd } from "../ui/primitives";
import { BotAvatar } from "../ui/BotAvatar";
import type { useChat } from "../../hooks/useChat";
import type { Project } from "../../hooks/useWorkspace";
import type { Attachment } from "../../lib/chatTypes";
import type { Conversation, CursorContext } from "../../lib/types";

interface ChatViewProps {
  chat: ReturnType<typeof useChat>;
  aiActive: boolean;
  workspaceRoot: string | null;
  conversations: Conversation[];
  projects: Project[];
  activeProject: Project | null;
  onSelectProject: (path: string | null) => void;
  onAddProject: () => void;
  onOpenMeeting: () => void;
  browserLinked: boolean;
  injected: Attachment[];
  onInjectedConsumed: () => void;
}

export function ChatView(props: ChatViewProps) {
  const { chat, aiActive, activeProject } = props;
  const [context, setContext] = useState<CursorContext | null>(null);
  const lastKeyRef = useRef("");

  useEffect(() => {
    if (!aiActive) {
      setContext(null);
      return;
    }
    // Each tick crosses into Win32 UI Automation through Tauri, so it must not keep
    // running while the window is hidden - there is no cursor context worth reading
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
    const start = () => {
      if (!interval) {
        void poll();
        interval = setInterval(poll, 1000);
      }
    };
    const stop = () => {
      if (interval) {
        clearInterval(interval);
        interval = null;
      }
    };
    const onVisibility = () => (document.hidden ? stop() : start());
    if (!document.hidden) start();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [aiActive]);

  const captureScreen = async () => {
    chat.appendMessage({ id: newId(), kind: "user", content: "What's on my screen around the cursor?" });
    const thinkingId = newId();
    chat.appendMessage({ id: thinkingId, kind: "thinking" });
    try {
      const result = await captureAnalyze();
      chat.removeMessage(thinkingId);
      chat.appendMessage({ id: newId(), kind: "assistant", content: result.text });
    } catch (err) {
      chat.removeMessage(thinkingId);
      chat.appendMessage({ id: newId(), kind: "assistant", content: `⚠️ ${friendlyError(err)}` });
    }
  };

  const composer = (variant: "hero" | "docked") => (
    <InputBar
      variant={variant}
      agentMode={chat.agentMode}
      onSetAgentMode={chat.setAgentMode}
      onSend={chat.send}
      busy={chat.isLoading}
      onStop={chat.stop}
      workspaceRoot={props.workspaceRoot}
      conversations={props.conversations}
      projects={props.projects}
      activeProject={activeProject}
      onSelectProject={props.onSelectProject}
      onAddProject={props.onAddProject}
      injected={props.injected}
      onInjectedConsumed={props.onInjectedConsumed}
    />
  );

  const contextBar = context && (
    <div className="mx-auto mt-3 flex w-fit max-w-[90%] items-center gap-2 rounded-full border border-border-default bg-bg-elevated py-1 pl-3 pr-1.5 text-[11.5px]">
      <AppWindow size={12} className="flex-shrink-0 text-accent" />
      <span className="font-medium text-text-secondary">{(context.process_name ?? "").replace(/\.exe$/i, "")}</span>
      <span className="truncate text-text-muted">{context.window_title}</span>
      <button onClick={() => setContext(null)} aria-label="Dismiss" className="rounded-full p-0.5 text-text-muted hover:text-text-primary">
        <X size={12} />
      </button>
    </div>
  );

  if (chat.messages.length === 0) {
    const suggestions = [
      {
        icon: <Camera size={15} />,
        title: "Explain my screen",
        body: "Look at what's around the cursor",
        run: captureScreen,
      },
      activeProject
        ? {
            icon: <FolderSearch size={15} />,
            title: `Tour ${activeProject.name}`,
            body: "Summarize the structure and how to run it",
            run: () => chat.send(`Give me a quick tour of this project: what it is, its structure, and how to run it.`),
          }
        : {
            icon: <FolderSearch size={15} />,
            title: "Add a project",
            body: "Point Kudu at a folder you work in",
            run: props.onAddProject,
          },
      {
        icon: <Globe size={15} />,
        title: "Scout the web",
        body: "Latest news on a topic, with sources",
        run: () => chat.send("Search the web for today's most important AI news and summarize it with sources."),
      },
      {
        icon: <Headphones size={15} />,
        title: "Kudu Pilot",
        body: "Live transcript and answers",
        run: props.onOpenMeeting,
      },
    ];

    return (
      <div className="flex h-full min-h-0 flex-col">
        {contextBar}
        <div className="brand-wash scrollbar-thin flex min-h-0 flex-1 flex-col items-center justify-center overflow-y-auto px-6 py-8">
          <div className="animate-fade-up flex w-full max-w-[760px] flex-col items-center">
            <div className="mb-5 flex items-end gap-1">
              <BotAvatar kind="kudu" mood="idle" size={72} variant="body" title="Kudu" />
              <BotAvatar kind="connect" mood={props.browserLinked ? "idle" : "sleeping"} size={38} title={props.browserLinked ? "Kudu Connect linked" : "Kudu Connect not connected"} />
            </div>
            <h1 className="mb-1.5 text-center text-[26px] font-semibold tracking-[-0.02em]">
              {activeProject ? (
                <>
                  What should we do in <span className="text-accent">{activeProject.name}</span>?
                </>
              ) : (
                "What should we work on?"
              )}
            </h1>
            <p className="mb-7 text-center text-[13px] text-text-muted">
              {chat.agentMode
                ? "Agent mode can run commands, edit files and use your PC - it asks before anything risky."
                : "Chat mode only answers; nothing on your computer is touched."}
            </p>
            {composer("hero")}
            <div className="mt-5 grid w-full grid-cols-2 gap-2 lg:grid-cols-4">
              {suggestions.map((s) => (
                <button
                  key={s.title}
                  onClick={() => void s.run()}
                  className="group flex flex-col gap-1 rounded-2xl border border-border-soft bg-bg-card p-3 text-left transition hover:-translate-y-px hover:border-border-default hover:bg-bg-hover/60"
                >
                  <span className="text-text-muted transition group-hover:text-accent">{s.icon}</span>
                  <span className="truncate text-[12.5px] font-medium text-text-primary">{s.title}</span>
                  <span className="line-clamp-2 text-[11.5px] leading-snug text-text-muted">{s.body}</span>
                </button>
              ))}
            </div>
            <div className="mt-8 flex flex-wrap items-center justify-center gap-x-5 gap-y-2 text-[11.5px] text-text-muted">
              <span className="flex items-center gap-1.5">
                <Kbd>Ctrl</Kbd>
                <Kbd>Shift</Kbd>
                <Kbd>Space</Kbd>
                ask at cursor, anywhere
              </span>
              <span className="flex items-center gap-1.5">
                <Kbd>Ctrl</Kbd>
                <Kbd>Shift</Kbd>
                <Kbd>Q</Kbd>
                Kudu Lens
              </span>
              <span className="flex items-center gap-1.5">
                <Kbd>Ctrl</Kbd>
                <Kbd>Shift</Kbd>
                <Kbd>Enter</Kbd>
                meeting answer
              </span>
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      {contextBar}
      <MessageList messages={chat.messages} />
      {composer("docked")}
    </div>
  );
}
