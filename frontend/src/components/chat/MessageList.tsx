import { useEffect, useRef } from "react";
import { Camera, FileText, MessageCircle, MessageSquare, Search } from "lucide-react";
import { Markdown } from "./Markdown";
import { AgentActivity } from "./AgentActivity";
import type { UiMessage } from "../../lib/chatTypes";

interface MessageListProps {
  messages: UiMessage[];
  onQuickAction: (action: "capture" | "ask" | "search") => void;
}

export function MessageList({ messages, onQuickAction }: MessageListProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    requestAnimationFrame(() => {
      el.scrollTop = el.scrollHeight;
    });
  }, [messages]);

  return (
    <div ref={containerRef} className="scrollbar-thin flex-1 overflow-y-auto px-6 py-5">
      <div className="mx-auto max-w-[760px]">
        {messages.length === 0 ? (
          <Welcome onQuickAction={onQuickAction} />
        ) : (
          messages.map((m) => <MessageItem key={m.id} message={m} />)
        )}
      </div>
    </div>
  );
}

function Welcome({ onQuickAction }: { onQuickAction: MessageListProps["onQuickAction"] }) {
  return (
    <div className="flex flex-col items-center gap-3 px-4 py-8 text-center">
      <div className="animate-[float_3s_ease-in-out_infinite] text-[40px]">🖱️✨</div>
      <h3 className="text-lg font-semibold">Mouse AI Ready</h3>
      <p className="max-w-[280px] text-xs text-text-secondary">
        Press <kbd className="rounded border border-border-default bg-bg-tertiary px-1.5 py-0.5 font-sans text-[11px]">Ctrl+Shift+Space</kbd> anywhere
        — an ask box opens right at your cursor about whatever you're pointing at. Or press{" "}
        <kbd className="rounded border border-border-default bg-bg-tertiary px-1.5 py-0.5 font-sans text-[11px]">Ctrl+Shift+Q</kbd> for hover mode:
        pause the mouse and a <strong>✦ Ask AI</strong> button appears.
      </p>
      <div className="mt-2 flex flex-wrap justify-center gap-1.5">
        <ActionChip icon={<Camera size={13} />} label="Capture & Analyze" onClick={() => onQuickAction("capture")} />
        <ActionChip icon={<MessageCircle size={13} />} label="Quick Ask" onClick={() => onQuickAction("ask")} />
        <ActionChip icon={<Search size={13} />} label="Web Search" onClick={() => onQuickAction("search")} />
      </div>
    </div>
  );
}

function ActionChip({ icon, label, onClick }: { icon: React.ReactNode; label: string; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className="flex items-center gap-1.5 rounded-full border border-border-default bg-bg-tertiary px-3.5 py-1.5 text-xs text-text-secondary transition hover:-translate-y-px hover:border-accent hover:bg-accent-soft hover:text-accent"
    >
      {icon}
      {label}
    </button>
  );
}

function MessageItem({ message }: { message: UiMessage }) {
  if (message.kind === "user") {
    const attachments = message.attachments ?? [];
    return (
      <div className="mb-3.5 flex flex-col items-end gap-1.5 border-b border-border-soft pb-3.5 last:border-none last:pb-0">
        {attachments.length > 0 && (
          <div className="flex max-w-[85%] flex-wrap justify-end gap-1.5">
            {attachments.map((a) =>
              a.kind === "image" ? (
                <img key={a.id} src={a.dataUrl} alt={a.name} className="h-14 w-14 rounded-lg border border-border-default object-cover" />
              ) : (
                <span
                  key={a.id}
                  className="flex items-center gap-1 rounded-full border border-border-default bg-bg-tertiary px-2.5 py-1 text-[11px] text-text-secondary"
                >
                  {a.kind === "file" ? <FileText size={11} /> : <MessageSquare size={11} />}
                  {a.kind === "file" ? a.name : a.title}
                </span>
              ),
            )}
          </div>
        )}
        {message.content && (
          <div className="max-w-[85%] rounded-2xl rounded-br-md bg-gradient-to-br from-accent to-accent-light px-3.5 py-2.5 text-[13px] text-white shadow-[0_3px_12px_rgba(118,159,205,0.25)]">
            {message.content}
          </div>
        )}
      </div>
    );
  }

  if (message.kind === "system") {
    return (
      <div className="mb-3.5 flex justify-center">
        <div className="rounded-md border border-border-default bg-bg-tertiary px-3 py-1.5 text-center text-[11px] text-text-muted">
          {message.content}
        </div>
      </div>
    );
  }

  if (message.kind === "thinking") {
    return (
      <div className="mb-3.5 flex items-start gap-2 border-b border-border-soft pb-3.5">
        <Avatar />
        <div className="flex gap-1 px-2 py-3">
          {[0, 1, 2].map((i) => (
            <span
              key={i}
              className="h-1.5 w-1.5 animate-bounce rounded-full bg-text-muted"
              style={{ animationDelay: `${i * 0.15}s` }}
            />
          ))}
        </div>
      </div>
    );
  }

  if (message.kind === "agent") {
    return (
      <div className="mb-3.5 flex items-start gap-2 border-b border-border-soft pb-3.5">
        <Avatar />
        <div className="min-w-0 flex-1">
          <AgentActivity steps={message.steps} status={message.status} elapsedMs={message.elapsedMs} />
        </div>
      </div>
    );
  }

  // assistant
  return (
    <div className="mb-3.5 flex items-start gap-2 border-b border-border-soft pb-3.5 last:border-none last:pb-0">
      <Avatar />
      <div className="min-w-0 flex-1 text-[13px] leading-[1.65] text-text-primary">
        <Markdown>{message.content}</Markdown>
      </div>
    </div>
  );
}

function Avatar() {
  return (
    <div className="mt-0.5 flex h-6 w-6 flex-shrink-0 items-center justify-center rounded-full border border-border-default bg-gradient-to-br from-accent-soft to-transparent text-xs">
      🖱️
    </div>
  );
}
