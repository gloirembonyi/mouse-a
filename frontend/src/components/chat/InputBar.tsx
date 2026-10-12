import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import {
  ArrowUp,
  AtSign,
  Bot,
  ChevronDown,
  FileText,
  Folder,
  FolderPlus,
  Image as ImageIcon,
  MessageCircle,
  MessageSquare,
  Plus,
  Square,
  X,
} from "lucide-react";
import { newId, type FsEntry } from "../../lib/tauri";
import { MentionPopover } from "./MentionPopover";
import { Menu, type MenuItem } from "../ui/Menu";
import type { Attachment } from "../../lib/chatTypes";
import type { Conversation } from "../../lib/types";
import type { Project } from "../../hooks/useWorkspace";

interface InputBarProps {
  variant: "hero" | "docked";
  agentMode: boolean;
  onSetAgentMode: (agent: boolean) => void;
  onSend: (text: string, attachments: Attachment[]) => void;
  busy: boolean;
  onStop: () => void;
  workspaceRoot: string | null;
  conversations: Conversation[];
  projects: Project[];
  activeProject: Project | null;
  onSelectProject: (path: string | null) => void;
  onAddProject: () => void;
  /** Attachments pushed in from elsewhere (e.g. the editor's "Ask AI"). */
  injected: Attachment[];
  onInjectedConsumed: () => void;
}

// Codex-style composer: textarea on top, a toolbar row below it inside the
// same rounded container (attach, mode, project, send/stop), attachment chips
// above the text, and an "@" picker (files + conversations). While a run is in
// progress a border beam travels around the box and Send becomes Stop.
export function InputBar({
  variant,
  agentMode,
  onSetAgentMode,
  onSend,
  busy,
  onStop,
  workspaceRoot,
  conversations,
  projects,
  activeProject,
  onSelectProject,
  onAddProject,
  injected,
  onInjectedConsumed,
}: InputBarProps) {
  const [value, setValue] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const mentionStartRef = useRef<number | null>(null);

  useEffect(() => {
    textareaRef.current?.focus();
  }, [variant]);

  useEffect(() => {
    if (injected.length === 0) return;
    setAttachments((prev) => [
      ...prev,
      ...injected.filter((a) => !prev.some((p) => p.kind === "file" && a.kind === "file" && p.path === a.path)),
    ]);
    onInjectedConsumed();
    requestAnimationFrame(() => textareaRef.current?.focus());
  }, [injected, onInjectedConsumed]);

  const resize = () => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
  };

  const submit = () => {
    const text = value.trim();
    if ((!text && attachments.length === 0) || busy) return;
    setValue("");
    setAttachments([]);
    setMentionQuery(null);
    requestAnimationFrame(resize);
    onSend(text, attachments);
  };

  const addImage = (dataUrl: string, name: string) => {
    setAttachments((prev) => [...prev.filter((a) => a.kind !== "image"), { id: newId(), kind: "image", dataUrl, name }]);
  };

  const readImage = (file: File, name = file.name) => {
    const reader = new FileReader();
    reader.onload = () => addImage(String(reader.result), name);
    reader.readAsDataURL(file);
  };

  const handleFilePick = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (file) readImage(file);
    e.target.value = "";
  };

  const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const item = Array.from(e.clipboardData.items).find((i) => i.type.startsWith("image/"));
    const file = item?.getAsFile();
    if (!file) return;
    e.preventDefault();
    readImage(file, "pasted-image.png");
  };

  const handleDrop = (e: React.DragEvent) => {
    const file = Array.from(e.dataTransfer.files).find((f) => f.type.startsWith("image/"));
    if (!file) return;
    e.preventDefault();
    readImage(file);
  };

  const handleTextChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const text = e.target.value;
    setValue(text);
    resize();
    const cursor = e.target.selectionStart ?? text.length;
    const match = /(?:^|\s)@([^\s@]*)$/.exec(text.slice(0, cursor));
    if (match) {
      mentionStartRef.current = cursor - match[1].length - 1;
      setMentionQuery(match[1]);
    } else {
      setMentionQuery(null);
    }
  };

  const closeMention = () => {
    setMentionQuery(null);
    mentionStartRef.current = null;
  };

  const openMentionManually = () => {
    const el = textareaRef.current;
    const cursor = el?.selectionStart ?? value.length;
    mentionStartRef.current = cursor;
    setValue(value.slice(0, cursor) + "@" + value.slice(cursor));
    setMentionQuery("");
    requestAnimationFrame(() => el?.focus());
  };

  const removeMentionText = () => {
    const start = mentionStartRef.current;
    const el = textareaRef.current;
    if (start === null || !el) return;
    const cursor = el.selectionStart ?? value.length;
    setValue(value.slice(0, start) + value.slice(cursor));
  };

  const selectFile = (entry: FsEntry) => {
    removeMentionText();
    setAttachments((prev) => [...prev, { id: newId(), kind: "file", path: entry.path, name: entry.name }]);
    closeMention();
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const selectConversation = (conv: Conversation) => {
    removeMentionText();
    setAttachments((prev) => [
      ...prev,
      { id: newId(), kind: "conversation", conversationId: conv.id, title: conv.title || "Untitled conversation" },
    ]);
    closeMention();
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const removeAttachment = (id: string) => setAttachments((prev) => prev.filter((a) => a.id !== id));

  const placement = variant === "hero" ? "bottom" : "top";

  const modeItems: MenuItem[] = [
    {
      key: "agent",
      icon: <Bot size={14} />,
      label: "Agent",
      description: "Runs commands, edits files, browses and uses your PC to finish the task.",
      checked: agentMode,
      onSelect: () => onSetAgentMode(true),
    },
    {
      key: "chat",
      icon: <MessageCircle size={14} />,
      label: "Chat",
      description: "Just answers. Nothing on your computer is touched.",
      checked: !agentMode,
      onSelect: () => onSetAgentMode(false),
    },
  ];

  const projectItems: MenuItem[] = [
    ...projects.map((p) => ({
      key: p.path,
      icon: <Folder size={14} />,
      label: p.name,
      description: p.path,
      checked: activeProject?.path === p.path,
      onSelect: () => onSelectProject(p.path),
    })),
    {
      key: "__none",
      icon: <X size={14} />,
      label: "No project",
      checked: !activeProject,
      onSelect: () => onSelectProject(null),
      divider: projects.length > 0,
    },
    { key: "__add", icon: <FolderPlus size={14} />, label: "Add folder…", onSelect: onAddProject },
  ];

  const canSend = !busy && (value.trim().length > 0 || attachments.length > 0);

  return (
    <div className={clsx("flex-shrink-0", variant === "docked" ? "px-6 pb-4 pt-2" : "w-full")}>
      <div className="relative mx-auto max-w-[760px]">
        {mentionQuery !== null && (
          <MentionPopover
            query={mentionQuery}
            workspaceRoot={workspaceRoot}
            conversations={conversations}
            onSelectFile={selectFile}
            onSelectConversation={selectConversation}
            onClose={closeMention}
          />
        )}

        <div
          data-active={busy}
          onDragOver={(e) => e.preventDefault()}
          onDrop={handleDrop}
          className="border-beam relative rounded-[22px] border border-border-default bg-bg-elevated shadow-[0_1px_2px_rgba(0,0,0,0.06),0_8px_24px_-12px_rgba(0,0,0,0.25)] transition focus-within:border-border-strong"
        >
          {attachments.length > 0 && (
            <div className="flex flex-wrap gap-1.5 px-3.5 pt-3">
              {attachments.map((a) => (
                <AttachmentChip key={a.id} attachment={a} onRemove={() => removeAttachment(a.id)} />
              ))}
            </div>
          )}

          <textarea
            ref={textareaRef}
            rows={variant === "hero" ? 2 : 1}
            value={value}
            onChange={handleTextChange}
            onPaste={handlePaste}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && mentionQuery === null) {
                e.preventDefault();
                submit();
              }
            }}
            placeholder={
              agentMode
                ? activeProject
                  ? `Ask Mouse AI to do anything in ${activeProject.name}…`
                  : "Ask Mouse AI to do anything…"
                : "Ask anything…"
            }
            className="max-h-[220px] w-full resize-none bg-transparent px-4 pb-1 pt-3.5 text-[14px] leading-relaxed text-text-primary outline-none placeholder:text-text-muted"
          />

          <div className="flex items-center justify-between gap-2 px-2.5 pb-2.5 pt-1">
            <div className="flex min-w-0 items-center gap-1">
              <Menu
                placement={placement}
                width={200}
                items={[
                  { key: "img", icon: <ImageIcon size={14} />, label: "Image…", onSelect: () => fileInputRef.current?.click() },
                  { key: "at", icon: <AtSign size={14} />, label: "File or chat…", onSelect: openMentionManually },
                ]}
                trigger={() => (
                  <button
                    title="Attach"
                    className="flex h-8 w-8 items-center justify-center rounded-full text-text-secondary transition hover:bg-bg-hover hover:text-text-primary"
                  >
                    <Plus size={17} />
                  </button>
                )}
              />
              <input ref={fileInputRef} type="file" accept="image/*" hidden onChange={handleFilePick} />

              <Menu
                placement={placement}
                width={290}
                items={modeItems}
                trigger={(open) => (
                  <Chip open={open} title="Agent acts on your PC; Chat only answers">
                    {agentMode ? <Bot size={13} /> : <MessageCircle size={13} />}
                    {agentMode ? "Agent" : "Chat"}
                  </Chip>
                )}
              />

              <Menu
                placement={placement}
                width={280}
                header="Project"
                items={projectItems}
                trigger={(open) => (
                  <Chip open={open} title={activeProject?.path ?? "Pick the folder the agent should work in"}>
                    <Folder size={13} />
                    <span className="max-w-[140px] truncate">{activeProject?.name ?? "No project"}</span>
                  </Chip>
                )}
              />
            </div>

            {busy ? (
              <button
                onClick={onStop}
                title="Stop"
                className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full btn-brand"
              >
                <Square size={11} fill="currentColor" />
              </button>
            ) : (
              <button
                onClick={submit}
                disabled={!canSend}
                title="Send (Enter)"
                className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full btn-brand disabled:cursor-not-allowed disabled:opacity-25"
              >
                <ArrowUp size={16} strokeWidth={2.4} />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function Chip({ children, open, title }: { children: React.ReactNode; open: boolean; title: string }) {
  return (
    <button
      title={title}
      className={clsx(
        "flex h-8 items-center gap-1.5 rounded-full px-2.5 text-[12.5px] font-medium transition",
        open ? "bg-bg-hover text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary",
      )}
    >
      {children}
      <ChevronDown size={12} className="opacity-60" />
    </button>
  );
}

function AttachmentChip({ attachment, onRemove }: { attachment: Attachment; onRemove: () => void }) {
  if (attachment.kind === "image") {
    return (
      <div className="group relative h-14 w-14 flex-shrink-0 overflow-hidden rounded-xl border border-border-default">
        <img src={attachment.dataUrl} alt={attachment.name} className="h-full w-full object-cover" />
        <button
          onClick={onRemove}
          aria-label="Remove image"
          className="absolute right-1 top-1 flex h-4 w-4 items-center justify-center rounded-full bg-black/60 text-white opacity-0 transition group-hover:opacity-100"
        >
          <X size={10} />
        </button>
      </div>
    );
  }
  const icon = attachment.kind === "file" ? <FileText size={12} /> : <MessageSquare size={12} />;
  const label = attachment.kind === "file" ? attachment.name : attachment.title;
  return (
    <div className="flex max-w-[200px] items-center gap-1.5 rounded-lg border border-border-default bg-bg-tertiary py-1 pl-2 pr-1 text-[12px] text-text-secondary">
      {icon}
      <span className="truncate">{label}</span>
      <button onClick={onRemove} aria-label="Remove" className="flex-shrink-0 rounded p-0.5 text-text-muted transition hover:bg-bg-hover hover:text-text-primary">
        <X size={11} />
      </button>
    </div>
  );
}
