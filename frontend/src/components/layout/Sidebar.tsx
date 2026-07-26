import { Code2, Eye, MessageSquare, Plus, Settings } from "lucide-react";
import { relativeTime } from "../../lib/format";
import type { Conversation } from "../../lib/types";

interface SidebarProps {
  threads: Conversation[];
  activeId: string | null;
  onNewChat: () => void;
  onOpenThread: (id: string) => void;
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
            <button
              key={t.id}
              onClick={() => onOpenThread(t.id)}
              className={`flex w-full flex-col gap-0.5 rounded-md border-l-2 px-2.5 py-2 text-left transition ${
                t.id === activeId
                  ? "border-accent bg-accent-soft text-text-primary"
                  : "border-transparent text-text-secondary hover:bg-bg-hover hover:text-text-primary"
              }`}
            >
              <div className="flex items-center gap-1.5 overflow-hidden">
                <MessageSquare size={11} className="flex-shrink-0 text-text-muted" />
                <span className="truncate text-xs font-semibold">{t.title || "Untitled conversation"}</span>
              </div>
              <div className="flex items-center gap-1.5 pl-[18px] text-[10px] text-text-muted">
                <span>{relativeTime(t.updated_at)}</span>
                <span className="h-0.5 w-0.5 rounded-full bg-text-muted opacity-70" />
                <span>
                  {t.message_count ?? "?"} msg{t.message_count === 1 ? "" : "s"}
                </span>
              </div>
            </button>
          ))
        )}
      </div>
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
