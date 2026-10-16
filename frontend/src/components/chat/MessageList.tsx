import { useEffect, useRef, useState } from "react";
import { ArrowDown, Check, Copy, FileText, MessageSquare } from "lucide-react";
import { BotAvatar } from "../ui/BotAvatar";
import { Markdown } from "./Markdown";
import { AgentActivity } from "./AgentActivity";
import type { UiMessage } from "../../lib/chatTypes";

interface MessageListProps {
  messages: UiMessage[];
}

export function MessageList({ messages }: MessageListProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);
  const [showJump, setShowJump] = useState(false);

  // Follow new output only while the reader is already at the bottom - never
  // yank them away from something they scrolled up to re-read.
  useEffect(() => {
    const el = containerRef.current;
    if (!el || !stickRef.current) return;
    requestAnimationFrame(() => {
      el.scrollTop = el.scrollHeight;
    });
  }, [messages]);

  const jumpToBottom = () => {
    const el = containerRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  };

  return (
    <div className="relative min-h-0 flex-1">
      <div
        ref={containerRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
          stickRef.current = atBottom;
          setShowJump(!atBottom);
        }}
        className="scrollbar-thin h-full overflow-y-auto px-6 pb-6 pt-6"
      >
        <div className="mx-auto flex max-w-[760px] flex-col gap-6">
          {messages.map((m, i) => (
            <MessageItem key={m.id} message={m} latest={i === messages.length - 1} />
          ))}
        </div>
      </div>
      {showJump && (
        <button
          onClick={jumpToBottom}
          title="Jump to latest"
          className="animate-pop absolute bottom-3 left-1/2 flex h-8 w-8 -translate-x-1/2 items-center justify-center rounded-full border border-border-default bg-bg-elevated text-text-secondary shadow-pop transition hover:text-text-primary"
        >
          <ArrowDown size={15} />
        </button>
      )}
    </div>
  );
}

function MessageItem({ message, latest }: { message: UiMessage; latest: boolean }) {
  if (message.kind === "user") {
    const attachments = message.attachments ?? [];
    return (
      <div className="animate-fade-up flex flex-col items-end gap-1.5">
        {attachments.length > 0 && (
          <div className="flex max-w-[85%] flex-wrap justify-end gap-1.5">
            {attachments.map((a) =>
              a.kind === "image" ? (
                <img key={a.id} src={a.dataUrl} alt={a.name} className="h-20 w-20 rounded-xl border border-border-default object-cover" />
              ) : (
                <span
                  key={a.id}
                  className="flex items-center gap-1 rounded-lg border border-border-default bg-bg-tertiary px-2 py-1 text-[12px] text-text-secondary"
                >
                  {a.kind === "file" ? <FileText size={12} /> : <MessageSquare size={12} />}
                  {a.kind === "file" ? a.name : a.kind === "snippet" ? "Terminal output" : a.title}
                </span>
              ),
            )}
          </div>
        )}
        {message.content && (
          <div className="max-w-[80%] whitespace-pre-wrap break-words rounded-[20px] bg-user-bubble px-4 py-2.5 text-[14px] leading-relaxed text-text-primary">
            {message.content}
          </div>
        )}
      </div>
    );
  }

  if (message.kind === "system") {
    return (
      <div className="flex justify-center">
        <div className="rounded-full border border-border-soft px-3 py-1 text-center text-[11.5px] text-text-muted">{message.content}</div>
      </div>
    );
  }

  if (message.kind === "thinking") {
    return (
      <div className="animate-fade-up flex items-center gap-3">
        <BotAvatar kind="kudu" mood="thinking" size={28} title="Kudu is thinking" />
        <span className="shimmer-text text-[13.5px] font-medium">Thinking…</span>
      </div>
    );
  }

  if (message.kind === "agent") {
    return (
      <div className="animate-fade-up flex min-w-0 gap-3">
        <BotAvatar
          kind="kudu"
          mood={message.status === "running" ? "working" : message.status === "failed" ? "error" : "still"}
          size={28}
          className="mt-1.5"
        />
        <div className="min-w-0 flex-1">
          <AgentActivity steps={message.steps} status={message.status} elapsedMs={message.elapsedMs} />
        </div>
      </div>
    );
  }

  return <AssistantMessage content={message.content} latest={latest} />;
}

function AssistantMessage({ content, latest }: { content: string; latest: boolean }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(content).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div className="animate-fade-up group flex min-w-0 gap-3">
      <BotAvatar kind="kudu" mood={latest ? "idle" : "still"} size={28} className="mt-0.5" />
      <div className="min-w-0 flex-1">
      <div className="text-[14px] leading-[1.7] text-text-primary">
        <Markdown>{content}</Markdown>
      </div>
      <div className="mt-1 flex h-6 items-center gap-1 opacity-0 transition group-hover:opacity-100">
        <button
          onClick={copy}
          title="Copy"
          className="flex items-center gap-1 rounded-md px-1.5 py-1 text-[11.5px] text-text-muted transition hover:bg-bg-hover hover:text-text-primary"
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      </div>
    </div>
  );
}
