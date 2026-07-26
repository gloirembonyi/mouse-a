import { useEffect, useMemo, useState } from "react";
import { FileText, MessageSquare } from "lucide-react";
import { fsSearchFiles, type FsEntry } from "../../lib/tauri";
import type { Conversation } from "../../lib/types";

interface MentionPopoverProps {
  query: string;
  workspaceRoot: string | null;
  conversations: Conversation[];
  onSelectFile: (entry: FsEntry) => void;
  onSelectConversation: (conv: Conversation) => void;
  onClose: () => void;
}

type Item = { kind: "file"; entry: FsEntry } | { kind: "conversation"; conv: Conversation };

// Cursor-style @ picker: type "@" in the composer to open this, live-filtered
// as you keep typing. Selecting an item turns it into a removable chip above
// the input rather than inline text — see InputBar/resolveAttachments.
export function MentionPopover({ query, workspaceRoot, conversations, onSelectFile, onSelectConversation, onClose }: MentionPopoverProps) {
  const [files, setFiles] = useState<FsEntry[]>([]);
  const [selected, setSelected] = useState(0);

  useEffect(() => {
    if (!workspaceRoot || !query.trim()) {
      setFiles([]);
      return;
    }
    let cancelled = false;
    fsSearchFiles(workspaceRoot, query, 12)
      .then((entries) => {
        if (!cancelled) setFiles(entries.filter((e) => !e.is_dir));
      })
      .catch(() => {
        if (!cancelled) setFiles([]);
      });
    return () => {
      cancelled = true;
    };
  }, [workspaceRoot, query]);

  const matchingConversations = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q ? conversations.filter((c) => (c.title || "").toLowerCase().includes(q)) : conversations;
    return list.slice(0, 8);
  }, [conversations, query]);

  const items: Item[] = useMemo(
    () => [
      ...files.map((entry): Item => ({ kind: "file", entry })),
      ...matchingConversations.map((conv): Item => ({ kind: "conversation", conv })),
    ],
    [files, matchingConversations],
  );

  useEffect(() => setSelected(0), [items.length]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelected((i) => Math.min(i + 1, Math.max(items.length - 1, 0)));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter") {
        const item = items[selected];
        if (item) {
          e.preventDefault();
          e.stopPropagation();
          pick(item);
        }
      } else if (e.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items, selected]);

  const pick = (item: Item) => {
    if (item.kind === "file") onSelectFile(item.entry);
    else onSelectConversation(item.conv);
  };

  if (items.length === 0) {
    return (
      <div className="absolute bottom-full left-0 right-0 mb-2 rounded-xl border border-border-default bg-bg-tertiary p-3 text-center text-xs text-text-muted shadow-lg">
        {query.trim() ? "No matching files or conversations." : "Type to search files and conversations…"}
      </div>
    );
  }

  return (
    <div className="absolute bottom-full left-0 right-0 mb-2 max-h-64 overflow-y-auto rounded-xl border border-border-default bg-bg-tertiary py-1.5 shadow-lg">
      {files.length > 0 && <SectionLabel label="Files" />}
      {files.map((entry, i) => (
        <Row
          key={entry.path}
          active={items[selected]?.kind === "file" && (items[selected] as { entry: FsEntry }).entry.path === entry.path}
          icon={<FileText size={13} />}
          label={entry.name}
          sublabel={entry.path}
          onClick={() => pick({ kind: "file", entry })}
          onHover={() => setSelected(i)}
        />
      ))}
      {matchingConversations.length > 0 && <SectionLabel label="Conversations" />}
      {matchingConversations.map((conv, i) => (
        <Row
          key={conv.id}
          active={items[selected]?.kind === "conversation" && (items[selected] as { conv: Conversation }).conv.id === conv.id}
          icon={<MessageSquare size={13} />}
          label={conv.title || "Untitled conversation"}
          sublabel={`${conv.message_count ?? "?"} messages`}
          onClick={() => pick({ kind: "conversation", conv })}
          onHover={() => setSelected(files.length + i)}
        />
      ))}
    </div>
  );
}

function SectionLabel({ label }: { label: string }) {
  return <div className="px-3 py-1 text-[10px] font-bold uppercase tracking-wide text-text-muted">{label}</div>;
}

function Row({
  active,
  icon,
  label,
  sublabel,
  onClick,
  onHover,
}: {
  active: boolean;
  icon: React.ReactNode;
  label: string;
  sublabel: string;
  onClick: () => void;
  onHover: () => void;
}) {
  return (
    <button
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      onMouseEnter={onHover}
      className={`flex w-full items-center gap-2 px-3 py-1.5 text-left transition ${
        active ? "bg-accent-soft text-accent" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
      }`}
    >
      <span className="flex-shrink-0">{icon}</span>
      <span className="min-w-0 flex-1">
        <div className="truncate text-xs font-medium">{label}</div>
        <div className="truncate text-[10px] text-text-muted">{sublabel}</div>
      </span>
    </button>
  );
}
