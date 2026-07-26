import { useEffect, useRef, useState } from "react";
import { Check, Code2, Eye, MessageSquare, Plus, Settings, Trash2 } from "lucide-react";
import { relativeTime } from "../../lib/format";
import type { Conversation } from "../../lib/types";

interface SidebarProps {
  threads: Conversation[];
  activeId: string | null;
  onNewChat: () => void;
  onOpenThread: (id: string) => void;
  onDeleteThread: (id: string) => void;
  onOpenSettings: () => void;
  onToggleHoverMode: () => void;
  onToggleIdeDock: () => void;
  ideDockOpen: boolean;
  aiActive: boolean;
}

export function Sidebar({
  threads,
  activeId,
  onNewChat,
  onOpenThread,
  onDeleteThread,
  onOpenSettings,
  onToggleHoverMode,
  onToggleIdeDock,
  ideDockOpen,
  aiActive,
}: SidebarProps) {
  return (
    <div className="flex h-full flex-col gap-1 overflow-hidden bg-bg-secondary px-2.5 py-3">
      <button
        onClick={onNewChat}
        className="mb-2 flex w-full flex-shrink-0 items-center gap-2 rounded-lg border border-border-focus bg-accent-soft px-3 py-2.5 text-[12.5px] font-semibold text-text-primary transition hover:bg-accent/25"
      >
        <Plus size={15} />
        New chat
      </button>

      <nav className="mb-2 flex flex-shrink-0 flex-col gap-0.5 border-b border-border-soft pb-2.5">
        <NavItem icon={<Eye size={15} />} label="Hover mode" active={aiActive} onClick={onToggleHoverMode} />
        <NavItem icon={<Code2 size={15} />} label="Editor / Terminal / Changes" active={ideDockOpen} onClick={onToggleIdeDock} />
        <NavItem icon={<Settings size={15} />} label="Settings" onClick={onOpenSettings} />
      </nav>

      <div className="flex-shrink-0 px-2.5 py-1 text-[10.5px] font-bold uppercase tracking-wide text-text-muted">
        Threads
      </div>
      <div className="scrollbar-thin flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-0.5">
        {threads.length === 0 ? (
          <p className="p-2 text-center text-xs text-text-muted">No conversations yet.</p>
        ) : (
          threads.map((t) => (
            <ThreadRow
              key={t.id}
              thread={t}
              active={t.id === activeId}
              onOpen={() => onOpenThread(t.id)}
              onDelete={() => onDeleteThread(t.id)}
            />
          ))
        )}
      </div>
    </div>
  );
}

function ThreadRow({
  thread,
  active,
  onOpen,
  onDelete,
}: {
  thread: Conversation;
  active: boolean;
  onOpen: () => void;
  onDelete: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (resetTimer.current) clearTimeout(resetTimer.current);
  }, []);

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
      className={`group relative flex w-full items-center rounded-md border-l-2 transition ${
        active ? "border-accent bg-accent-soft text-text-primary" : "border-transparent text-text-secondary hover:bg-bg-hover hover:text-text-primary"
      }`}
    >
      <button onClick={onOpen} className="flex min-w-0 flex-1 flex-col gap-0.5 px-2.5 py-2 text-left">
        <div className="flex items-center gap-1.5 overflow-hidden">
          <MessageSquare size={11} className="flex-shrink-0 text-text-muted" />
          <span className="truncate text-xs font-semibold">{thread.title || "Untitled conversation"}</span>
        </div>
        <div className="flex items-center gap-1.5 pl-[18px] text-[10px] text-text-muted">
          <span>{relativeTime(thread.updated_at)}</span>
          <span className="h-0.5 w-0.5 rounded-full bg-text-muted opacity-70" />
          <span>
            {thread.message_count ?? "?"} msg{thread.message_count === 1 ? "" : "s"}
          </span>
        </div>
      </button>

      {confirming ? (
        <button
          onClick={confirmDelete}
          title="Click again to permanently delete"
          className="mr-1.5 flex flex-shrink-0 items-center gap-1 rounded px-1.5 py-1 text-[10px] font-semibold text-danger transition hover:bg-danger-soft"
        >
          <Check size={12} />
          Delete?
        </button>
      ) : (
        <button
          onClick={armConfirm}
          title="Delete conversation"
          className="mr-1.5 flex-shrink-0 rounded p-1 text-text-muted opacity-0 transition hover:bg-danger-soft hover:text-danger group-hover:opacity-100"
        >
          <Trash2 size={13} />
        </button>
      )}
    </div>
  );
}

function NavItem({
  icon,
  label,
  active,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-[12.5px] font-medium transition ${
        active ? "bg-accent-soft text-accent" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}
