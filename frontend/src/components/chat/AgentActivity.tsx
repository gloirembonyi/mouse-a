import { useEffect, useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  FileText,
  Globe,
  Loader2,
  MousePointerClick,
  Search,
  Terminal,
  Wrench,
  X,
} from "lucide-react";
import { agentConfirm } from "../../lib/tauri";
import { formatDuration, friendlyError } from "../../lib/format";
import type { AgentProgressEvent } from "../../lib/types";

export interface AgentStep {
  kind: AgentProgressEvent["kind"];
  message: string;
  requestId?: string;
}

interface AgentActivityProps {
  steps: AgentStep[];
  status: "running" | "done" | "failed";
  elapsedMs?: number;
}

// Cursor/Codex-style: a single collapsible "Thought for Xs" summary line,
// expanded automatically while the task runs (or if it's waiting on a
// confirm prompt) and collapsed to a one-line recap once finished — the raw
// step log is still there, just tucked away instead of permanently taking up
// transcript space.
export function AgentActivity({ steps, status, elapsedMs }: AgentActivityProps) {
  const hasPendingConfirm = steps.some((s) => s.kind === "confirm");
  const [userToggled, setUserToggled] = useState<boolean | null>(null);
  const expanded = userToggled ?? (status === "running" || hasPendingConfirm);

  const [liveElapsed, setLiveElapsed] = useState(0);
  useEffect(() => {
    if (status !== "running") return;
    const startedAt = Date.now();
    const interval = setInterval(() => setLiveElapsed(Date.now() - startedAt), 1000);
    return () => clearInterval(interval);
  }, [status]);

  const durationLabel = useMemo(() => {
    const ms = status === "running" ? liveElapsed : elapsedMs;
    return typeof ms === "number" ? formatDuration(ms) : null;
  }, [status, liveElapsed, elapsedMs]);

  return (
    <div className="max-w-full overflow-hidden rounded-xl border border-border-soft bg-bg-card">
      <button
        onClick={() => setUserToggled(!expanded)}
        className={`flex w-full items-center gap-2 px-3 py-2.5 text-left text-[12.5px] font-semibold transition hover:bg-white/[0.02] ${
          status === "failed" ? "text-danger" : "text-text-primary"
        }`}
      >
        {status === "running" ? (
          <Loader2 size={13} className="flex-shrink-0 animate-spin text-accent" />
        ) : status === "failed" ? (
          <X size={13} className="flex-shrink-0" />
        ) : (
          <Check size={13} className="flex-shrink-0 text-success" />
        )}
        <span className="flex-1">
          {status === "running" ? "Working" : status === "failed" ? "Failed" : "Thought"}
          {durationLabel && <span className="font-normal text-text-muted"> for {durationLabel}</span>}
        </span>
        <ChevronDown size={14} className={`flex-shrink-0 text-text-muted transition-transform ${expanded ? "rotate-180" : ""}`} />
      </button>

      {expanded && (
        <div className="flex flex-col border-t border-border-soft px-3 py-2">
          {steps.map((step, i) =>
            step.kind === "confirm" ? (
              <ConfirmPrompt key={i} message={step.message} requestId={step.requestId!} />
            ) : (
              <Step key={i} step={step} isLast={i === steps.length - 1} />
            ),
          )}
        </div>
      )}
    </div>
  );
}

// The model's own step narration ("thinking"/"tool"/"tool-result" text) can
// contain `code` spans or **bold** the same way a final answer does, but this
// row is a single compact line, not a full document — so this renders just
// those two inline forms instead of pulling in ReactMarkdown's block-level
// layout (headings, lists, paragraphs) for what's meant to read as one line.
function renderInline(text: string): React.ReactNode[] {
  const parts: React.ReactNode[] = [];
  const regex = /(`[^`]+`|\*\*[^*]+\*\*)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  let key = 0;
  while ((match = regex.exec(text))) {
    if (match.index > lastIndex) parts.push(text.slice(lastIndex, match.index));
    const token = match[0];
    if (token.startsWith("`")) {
      parts.push(
        <code key={key++} className="rounded bg-white/10 px-1 py-0.5 font-mono text-[10px]">
          {token.slice(1, -1)}
        </code>,
      );
    } else {
      parts.push(<strong key={key++}>{token.slice(2, -2)}</strong>);
    }
    lastIndex = regex.lastIndex;
  }
  if (lastIndex < text.length) parts.push(text.slice(lastIndex));
  return parts;
}

function iconForStep(step: AgentStep) {
  if (step.kind === "thinking") return null;
  if (step.kind === "error") return X;
  const text = step.message.toLowerCase();
  if (/\b(read|write|file|directory|folder)\b/.test(text)) return FileText;
  if (/\b(search|web|google|bing)\b/.test(text)) return Search;
  if (/\b(command|terminal|shell|run|powershell)\b/.test(text)) return Terminal;
  if (/\b(click|scroll|type|press|move)\b/.test(text)) return MousePointerClick;
  if (/\b(url|open|browser|navigate)\b/.test(text)) return Globe;
  return Wrench;
}

function Step({ step, isLast }: { step: AgentStep; isLast: boolean }) {
  const Icon = iconForStep(step);
  const textClass =
    step.kind === "tool"
      ? "text-text-primary font-mono text-[10.5px]"
      : step.kind === "tool-result"
        ? "text-text-muted text-[10.5px]"
        : step.kind === "error"
          ? "text-danger"
          : step.kind === "thinking"
            ? "italic text-text-secondary"
            : "text-text-secondary";

  const iconClass =
    step.kind === "tool"
      ? "bg-accent-soft border-accent/35 text-accent"
      : step.kind === "error"
        ? "bg-danger-soft border-danger/40 text-danger"
        : "bg-bg-tertiary border-border-default text-text-secondary";

  return (
    <div className="flex gap-2">
      <div className="flex w-4 flex-shrink-0 flex-col items-center">
        <span className={`flex h-[15px] w-[15px] flex-shrink-0 items-center justify-center rounded-full border ${iconClass}`}>
          {Icon ? <Icon size={8.5} /> : <span className="h-1 w-1 rounded-full bg-current" />}
        </span>
        <span className={`mt-0.5 w-px flex-1 ${isLast ? "bg-transparent" : "bg-border-default"}`} style={{ minHeight: 6 }} />
      </div>
      <div className={`flex-1 pb-2.5 text-[11px] leading-[1.5] ${textClass}`}>
        {renderInline(step.kind === "error" ? friendlyError(step.message) : step.message)}
      </div>
    </div>
  );
}

function ConfirmPrompt({ message, requestId }: { message: string; requestId: string }) {
  const [decided, setDecided] = useState<"allow" | "deny" | null>(null);

  const decide = async (approved: boolean) => {
    setDecided(approved ? "allow" : "deny");
    try {
      await agentConfirm(requestId, approved);
    } catch {
      /* ignore */
    }
  };

  return (
    <div className="my-1 rounded-lg border border-warning/40 bg-warning-soft px-2.5 py-2">
      <div className="mb-2 text-[11.5px] leading-[1.4] text-text-primary">
        {decided ? (decided === "allow" ? `✅ Allowed: ${message}` : `🚫 Denied: ${message}`) : `🔒 ${message}`}
      </div>
      {!decided && (
        <div className="flex gap-2">
          <button
            onClick={() => decide(true)}
            className="flex-1 rounded-md border border-success/35 bg-success-soft px-2.5 py-1.5 text-[11.5px] font-semibold text-success transition hover:bg-success/30"
          >
            Allow
          </button>
          <button
            onClick={() => decide(false)}
            className="flex-1 rounded-md border border-danger/35 bg-danger-soft px-2.5 py-1.5 text-[11.5px] font-semibold text-danger transition hover:bg-danger/30"
          >
            Deny
          </button>
        </div>
      )}
    </div>
  );
}
