import { useRef, useState } from "react";
import { AtSign, ArrowUp, Bot, Eye, FileText, Image as ImageIcon, MessageSquare, MousePointerClick, X } from "lucide-react";
import { askAtCursor, newId, type FsEntry } from "../../lib/tauri";
import { MentionPopover } from "./MentionPopover";
import type { Attachment } from "../../lib/chatTypes";
import type { Conversation } from "../../lib/types";

interface InputBarProps {
  agentMode: boolean;
  onToggleAgentMode: () => void;
  onSend: (text: string, attachments: Attachment[]) => void;
  disabled: boolean;
  onToggleHoverMode: () => void;
  workspaceRoot: string | null;
  conversations: Conversation[];
}

// Cursor/Codex-style composer: textarea on top, a toolbar row below it inside
// the same bordered container, an attachment-chips row above it when there's
// anything tagged, and an "@" picker (files + conversations) that opens by
// typing "@" or clicking the @ button.
export function InputBar({
  agentMode,
  onToggleAgentMode,
  onSend,
  disabled,
  onToggleHoverMode,
  workspaceRoot,
  conversations,
}: InputBarProps) {
  const [value, setValue] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const mentionStartRef = useRef<number | null>(null);

  const resize = () => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  };

  const submit = () => {
    const text = value.trim();
    if ((!text && attachments.length === 0) || disabled) return;
    setValue("");
    setAttachments([]);
    setMentionQuery(null);
    requestAnimationFrame(resize);
    onSend(text, attachments);
  };

  const addImage = (dataUrl: string, name: string) => {
    setAttachments((prev) => [...prev.filter((a) => a.kind !== "image"), { id: newId(), kind: "image", dataUrl, name }]);
  };

  const handleFilePick = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => addImage(String(reader.result), file.name);
    reader.readAsDataURL(file);
    e.target.value = "";
  };

  const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const item = Array.from(e.clipboardData.items).find((i) => i.type.startsWith("image/"));
    if (!item) return;
    const file = item.getAsFile();
    if (!file) return;
    e.preventDefault();
    const reader = new FileReader();
    reader.onload = () => addImage(String(reader.result), "pasted-image.png");
    reader.readAsDataURL(file);
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
    const insertion = value.slice(0, cursor) + "@" + value.slice(cursor);
    mentionStartRef.current = cursor;
    setValue(insertion);
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

  return (
    <div className="flex-shrink-0 border-t border-border-default bg-bg-secondary px-6 pb-4 pt-3.5">
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

        <div className="rounded-2xl border border-border-default bg-bg-tertiary transition focus-within:border-border-focus">
          {attachments.length > 0 && (
            <div className="flex flex-wrap gap-1.5 px-3 pt-2.5">
              {attachments.map((a) => (
                <AttachmentChip key={a.id} attachment={a} onRemove={() => removeAttachment(a.id)} />
              ))}
            </div>
          )}

          <textarea
            ref={textareaRef}
            rows={1}
            value={value}
            onChange={handleTextChange}
            onPaste={handlePaste}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && mentionQuery === null) {
                e.preventDefault();
                submit();
              }
            }}
            placeholder={agentMode ? "Ask Mouse AI to do anything… (@ to tag a file or conversation)" : "Ask Mouse AI anything…"}
            className="max-h-[160px] w-full resize-none bg-transparent px-3.5 pb-1 pt-3 text-[13px] text-text-primary outline-none placeholder:text-text-muted"
          />

          <div className="flex items-center justify-between gap-2 px-2.5 pb-2 pt-1">
            <div className="flex items-center gap-1.5">
              <button
                onClick={onToggleAgentMode}
                title="Agent mode: Mouse AI can run commands, research, edit files, and control your PC to complete a task"
                className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] font-semibold transition ${
                  agentMode
                    ? "border-border-focus bg-accent-soft text-accent"
                    : "border-border-default bg-transparent text-text-secondary hover:bg-bg-hover"
                }`}
              >
                <Bot size={12} />
                Agent {agentMode ? "ON" : "OFF"}
              </button>
              <IconButton title="Attach an image" onClick={() => fileInputRef.current?.click()}>
                <ImageIcon size={14} />
              </IconButton>
              <IconButton title="Tag a file or conversation" onClick={openMentionManually}>
                <AtSign size={14} />
              </IconButton>
              <input ref={fileInputRef} type="file" accept="image/*" hidden onChange={handleFilePick} />
            </div>

            <button
              onClick={submit}
              disabled={disabled || (!value.trim() && attachments.length === 0)}
              title="Send (Enter)"
              className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-full bg-accent text-white transition hover:scale-105 hover:bg-accent-light disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:scale-100"
            >
              <ArrowUp size={15} />
            </button>
          </div>
        </div>
      </div>

      <div className="mx-auto flex max-w-[760px] items-center gap-3.5 px-1 pt-2">
        <HintButton onClick={() => askAtCursor().catch(() => {})} label="Ask at cursor" shortcut="Ctrl+Shift+Space" />
        <HintButton onClick={onToggleHoverMode} label="Hover mode" shortcut="Ctrl+Shift+Q" icon={<Eye size={10} />} />
        <span className="flex items-center gap-1 text-[10px] text-text-muted">
          <MousePointerClick size={10} />
          Ctrl+Shift+S
        </span>
      </div>
    </div>
  );
}

function IconButton({ children, title, onClick }: { children: React.ReactNode; title: string; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="flex h-6 w-6 items-center justify-center rounded-md text-text-muted transition hover:bg-bg-hover hover:text-text-secondary"
    >
      {children}
    </button>
  );
}

function AttachmentChip({ attachment, onRemove }: { attachment: Attachment; onRemove: () => void }) {
  if (attachment.kind === "image") {
    return (
      <div className="group relative h-12 w-12 flex-shrink-0 overflow-hidden rounded-lg border border-border-default">
        <img src={attachment.dataUrl} alt={attachment.name} className="h-full w-full object-cover" />
        <button
          onClick={onRemove}
          className="absolute right-0.5 top-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-black/60 text-white opacity-0 transition group-hover:opacity-100"
        >
          <X size={10} />
        </button>
      </div>
    );
  }
  const icon = attachment.kind === "file" ? <FileText size={11} /> : <MessageSquare size={11} />;
  const label = attachment.kind === "file" ? attachment.name : attachment.title;
  return (
    <div className="flex max-w-[180px] items-center gap-1.5 rounded-full border border-border-default bg-bg-hover px-2.5 py-1 text-[11px] text-text-secondary">
      {icon}
      <span className="truncate">{label}</span>
      <button onClick={onRemove} className="flex-shrink-0 text-text-muted transition hover:text-danger">
        <X size={11} />
      </button>
    </div>
  );
}

function HintButton({
  onClick,
  label,
  shortcut,
  icon,
}: {
  onClick: () => void;
  label: string;
  shortcut: string;
  icon?: React.ReactNode;
}) {
  return (
    <button onClick={onClick} className="flex items-center gap-1 text-[10px] text-text-muted transition hover:text-text-secondary">
      {icon}
      {label}: {shortcut}
    </button>
  );
}
