import { useEffect, useMemo, useRef, useState } from "react";
import clsx from "clsx";
import {
  Check,
  Code2,
  Folder,
  FolderOpen,
  FolderPlus,
  Headphones,
  Network,
  Moon,
  MousePointer2,
  PanelLeftClose,
  Search,
  Settings,
  SquarePen,
  Sun,
  Trash2,
  X,
} from "lucide-react";
import { dateBucket, threadTitle, type DateBucket } from "../../lib/format";
import { useTheme } from "../../lib/theme";
import type { Conversation, User } from "../../lib/types";
import type { Project } from "../../hooks/useWorkspace";
import type { ChatSession } from "../../hooks/useChat";
import { BotAvatar } from "../ui/BotAvatar";
import { IconButton, Kbd, SectionLabel, Switch } from "../ui/primitives";

interface SidebarProps {
  user?: User;
  threads: Conversation[];
  activeId: string | null;
  onNewChat: () => void;
  onOpenThread: (id: string) => void;
  onDeleteThread: (id: string) => void;
  onOpenSettings: () => void;
  settingsActive: boolean;
  onOpenMeeting: () => void;
  meetingActive: boolean;
  onOpenAgents: () => void;
  agentsActive: boolean;
  agentsBusy: number;
  agentsWaiting: number;
  browserLinked: boolean;
  meetingLive: boolean;
  onToggleHoverMode: () => void;
  aiActive: boolean;
  onToggleIdeDock: () => void;
  ideDockOpen: boolean;
  projects: Project[];
  activeProjectPath: string | null;
  onSelectProject: (path: string | null) => void;
  onAddProject: () => void;
  onRemoveProject: (path: string) => void;
  onCollapse: () => void;
  backgroundSessions: ChatSession[];
  runningConversationIds: string[];
  onOpenSession: (key: string) => void;
}

const BUCKET_ORDER: DateBucket[] = ["Today", "Yesterday", "Previous 7 days", "Previous 30 days", "Older"];

export function Sidebar(props: SidebarProps) {
  const { threads, activeId, projects, activeProjectPath } = props;
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const { theme, toggleTheme } = useTheme();

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q ? threads.filter((t) => threadTitle(t.title).toLowerCase().includes(q)) : threads;
    const map = new Map<DateBucket, Conversation[]>();
    for (const t of filtered) {
      const b = dateBucket(t.updated_at);
      map.set(b, [...(map.get(b) ?? []), t]);
    }
    return BUCKET_ORDER.filter((b) => map.has(b)).map((b) => ({ bucket: b, items: map.get(b)! }));
  }, [threads, query]);

  // Ctrl+K focuses thread search, Ctrl+N starts a new chat - the two
  // keyboard paths people expect from Codex/ChatGPT-style apps.
  const newChatRef = useRef(props.onNewChat);
  newChatRef.current = props.onNewChat;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSearching(true);
        requestAnimationFrame(() => searchRef.current?.focus());
      } else if (e.key.toLowerCase() === "n" && !e.shiftKey) {
        e.preventDefault();
        newChatRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <aside className="flex h-full flex-col overflow-hidden bg-bg-secondary">
      {/* Primary actions */}
      <div className="flex flex-shrink-0 flex-col gap-px px-2 pt-2">
        <div className="mb-1 flex items-center justify-between pl-1">
          <span className="text-[11px] font-medium text-text-muted">Workspace</span>
          <IconButton title="Hide sidebar" size="sm" onClick={props.onCollapse}>
            <PanelLeftClose size={14} />
          </IconButton>
        </div>
        <NavRow icon={<SquarePen size={15} />} label="New chat" hint={<Kbd>Ctrl N</Kbd>} onClick={props.onNewChat} />
        {searching ? (
          <div className="flex h-8 items-center gap-2 rounded-lg bg-bg-hover px-2.5">
            <Search size={14} className="flex-shrink-0 text-text-muted" />
            <input
              ref={searchRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  setQuery("");
                  setSearching(false);
                }
              }}
              onBlur={() => !query && setSearching(false)}
              placeholder="Search chats"
              className="min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-text-muted"
            />
            <button
              onClick={() => {
                setQuery("");
                setSearching(false);
              }}
              className="text-text-muted hover:text-text-primary"
              aria-label="Clear search"
            >
              <X size={13} />
            </button>
          </div>
        ) : (
          <NavRow
            icon={<Search size={15} />}
            label="Search"
            hint={<Kbd>Ctrl K</Kbd>}
            onClick={() => {
              setSearching(true);
              requestAnimationFrame(() => searchRef.current?.focus());
            }}
          />
        )}
        <NavRow
          icon={<Headphones size={15} />}
          label="Meeting copilot"
          active={props.meetingActive}
          onClick={props.onOpenMeeting}
          hint={
            props.meetingLive ? (
              <span className="flex items-center gap-1 text-[10.5px] font-medium text-danger">
                <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-danger" />
                Live
              </span>
            ) : undefined
          }
        />
        <NavRow
          icon={<Network size={15} />}
          label="Agents"
          active={props.agentsActive}
          onClick={props.onOpenAgents}
          hint={
            props.agentsWaiting > 0 ? (
              <span className="rounded-full bg-warning px-1.5 text-[10.5px] font-semibold text-black">{props.agentsWaiting}</span>
            ) : props.agentsBusy > 0 ? (
              <span className="flex items-center gap-1 text-[10.5px] font-medium text-accent">
                <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-accent" />
                {props.agentsBusy}
              </span>
            ) : (
              <span
                className={clsx("h-1.5 w-1.5 rounded-full", props.browserLinked ? "bg-success" : "bg-border-strong")}
                title={props.browserLinked ? "Browser agent linked" : "Browser agent not connected"}
              />
            )
          }
        />
        <NavRow icon={<Code2 size={15} />} label="Editor" active={props.ideDockOpen} onClick={props.onToggleIdeDock} />
        <NavRow
          icon={<MousePointer2 size={15} />}
          label="Hover mode"
          onClick={props.onToggleHoverMode}
          hint={<Switch on={props.aiActive} onChange={props.onToggleHoverMode} label="Toggle hover mode (Ctrl+Shift+Q)" />}
        />
      </div>

      <div className="scrollbar-thin mt-3 flex min-h-0 flex-1 flex-col overflow-y-auto px-2 pb-2">
        {/* Chats running (or just finished) in the background */}
        {props.backgroundSessions.length > 0 && (
          <div className="mb-2">
            <SectionLabel>In progress</SectionLabel>
            {props.backgroundSessions.map((s) => (
              <button
                key={s.key}
                onClick={() => props.onOpenSession(s.key)}
                title={s.running ? "Still working - click to watch" : "Finished - click to see the result"}
                className="group flex h-9 w-full items-center gap-2 rounded-lg px-2 text-left transition hover:bg-bg-hover"
              >
                <BotAvatar kind="ghost" mood={s.running ? "working" : "happy"} size={20} />
                <span className="min-w-0 flex-1 truncate text-[13px] text-text-primary">{s.title || "New chat"}</span>
                {s.needsYou ? (
                  <span className="flex-shrink-0 rounded-full bg-warning px-1.5 text-[10px] font-semibold text-black">Needs you</span>
                ) : s.running ? (
                  <span className="h-1.5 w-1.5 flex-shrink-0 animate-pulse rounded-full bg-accent" />
                ) : (
                  <Check size={13} className="flex-shrink-0 text-success" />
                )}
              </button>
            ))}
          </div>
        )}

        {/* Projects */}
        <SectionLabel
          action={
            <IconButton title="Add a project folder" size="sm" onClick={props.onAddProject}>
              <FolderPlus size={13} />
            </IconButton>
          }
        >
          Projects
        </SectionLabel>
        {projects.length === 0 ? (
          <button
            onClick={props.onAddProject}
            className="mx-0.5 mb-1 flex items-center gap-2 rounded-lg border border-dashed border-border-default px-2.5 py-2 text-left text-[12px] text-text-muted transition hover:border-border-strong hover:text-text-secondary"
          >
            <FolderPlus size={14} />
            Add a folder you're working on
          </button>
        ) : (
          projects.map((p) => (
            <ProjectRow
              key={p.path}
              project={p}
              active={p.path === activeProjectPath}
              onSelect={() => props.onSelectProject(p.path === activeProjectPath ? null : p.path)}
              onRemove={() => props.onRemoveProject(p.path)}
            />
          ))
        )}

        {/* Threads */}
        {threads.length === 0 ? (
          <>
            <SectionLabel>Chats</SectionLabel>
            <p className="px-2.5 py-1 text-[12px] text-text-muted">No chats yet.</p>
          </>
        ) : groups.length === 0 ? (
          <p className="px-2.5 py-3 text-[12px] text-text-muted">No chats match “{query}”.</p>
        ) : (
          groups.map((g) => (
            <div key={g.bucket} className="mt-2">
              <SectionLabel>{g.bucket}</SectionLabel>
              {g.items.map((t) => (
                <ThreadRow
                  key={t.id}
                  running={props.runningConversationIds.includes(t.id)}
                  thread={t}
                  active={t.id === activeId}
                  onOpen={() => props.onOpenThread(t.id)}
                  onDelete={() => props.onDeleteThread(t.id)}
                />
              ))}
            </div>
          ))
        )}
      </div>

      {/* Footer: account + appearance + settings */}
      <div className="flex flex-shrink-0 items-center gap-2 border-t border-border-soft px-2.5 py-2">
        <button
          onClick={props.onOpenSettings}
          className={clsx(
            "flex min-w-0 flex-1 items-center gap-2 rounded-lg px-1.5 py-1 text-left transition hover:bg-bg-hover",
            props.settingsActive && "bg-bg-hover",
          )}
        >
          {props.user?.picture ? (
            <img src={props.user.picture} alt="" className="h-6 w-6 flex-shrink-0 rounded-full object-cover" />
          ) : (
            <span className="flex h-6 w-6 flex-shrink-0 items-center justify-center rounded-full bg-accent-soft text-[11px] font-semibold text-accent">
              {(props.user?.name || props.user?.email || "G").slice(0, 1).toUpperCase()}
            </span>
          )}
          <span className="truncate text-[12.5px] font-medium">{props.user?.name || props.user?.email || "Guest"}</span>
        </button>
        <IconButton title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"} onClick={toggleTheme}>
          {theme === "dark" ? <Sun size={15} /> : <Moon size={15} />}
        </IconButton>
        <IconButton title="Settings" onClick={props.onOpenSettings} active={props.settingsActive}>
          <Settings size={15} />
        </IconButton>
      </div>
    </aside>
  );
}

function NavRow({
  icon,
  label,
  hint,
  active,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  hint?: React.ReactNode;
  active?: boolean;
  onClick: () => void;
}) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onClick())}
      className={clsx(
        "group flex h-8 cursor-pointer select-none items-center gap-2.5 rounded-lg px-2.5 text-[13px] transition",
        active ? "bg-bg-hover text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary",
      )}
    >
      <span className={clsx("flex-shrink-0", active ? "text-text-primary" : "text-text-muted group-hover:text-text-secondary")}>{icon}</span>
      <span className="flex-1 truncate">{label}</span>
      {hint && <span className="flex flex-shrink-0 items-center">{hint}</span>}
    </div>
  );
}

function ProjectRow({
  project,
  active,
  onSelect,
  onRemove,
}: {
  project: Project;
  active: boolean;
  onSelect: () => void;
  onRemove: () => void;
}) {
  return (
    <div
      className={clsx(
        "group flex h-8 items-center rounded-lg transition",
        active ? "bg-accent-soft text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary",
      )}
    >
      <button onClick={onSelect} title={project.path} className="flex min-w-0 flex-1 items-center gap-2.5 px-2.5 text-left text-[13px]">
        {active ? (
          <FolderOpen size={15} className="flex-shrink-0 text-accent" />
        ) : (
          <Folder size={15} className="flex-shrink-0 text-text-muted" />
        )}
        <span className="truncate">{project.name}</span>
      </button>
      <button
        onClick={onRemove}
        title="Remove from list (doesn't delete anything)"
        className="mr-1.5 rounded p-1 text-text-muted opacity-0 transition hover:bg-bg-tertiary hover:text-text-primary group-hover:opacity-100"
      >
        <X size={12} />
      </button>
    </div>
  );
}

function ThreadRow({
  thread,
  running,
  active,
  onOpen,
  onDelete,
}: {
  thread: Conversation;
  running?: boolean;
  active: boolean;
  onOpen: () => void;
  onDelete: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (resetTimer.current) clearTimeout(resetTimer.current);
    },
    [],
  );

  const armConfirm = (e: React.MouseEvent) => {
    e.stopPropagation();
    setConfirming(true);
    resetTimer.current = setTimeout(() => setConfirming(false), 3000);
  };

  const confirmDelete = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (resetTimer.current) clearTimeout(resetTimer.current);
    onDelete();
  };

  return (
    <div
      className={clsx(
        "group relative flex h-8 w-full items-center rounded-lg transition",
        active ? "bg-bg-hover text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary",
      )}
    >
      <button onClick={onOpen} title={thread.title} className="min-w-0 flex-1 truncate px-2.5 text-left text-[13px]">
        {threadTitle(thread.title)}
      </button>
      {running && <span className="mr-2 h-1.5 w-1.5 flex-shrink-0 animate-pulse rounded-full bg-accent" title="Working" />}

      {confirming ? (
        <button
          onClick={confirmDelete}
          title="Click again to permanently delete"
          className="mr-1 flex flex-shrink-0 items-center gap-1 rounded-md bg-danger-soft px-1.5 py-0.5 text-[11px] font-medium text-danger"
        >
          <Check size={11} />
          Delete
        </button>
      ) : (
        <button
          onClick={armConfirm}
          title="Delete chat"
          className="mr-1.5 flex-shrink-0 rounded p-1 text-text-muted opacity-0 transition hover:text-danger group-hover:opacity-100"
        >
          <Trash2 size={12} />
        </button>
      )}
    </div>
  );
}
