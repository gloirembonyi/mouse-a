import { useEffect, useMemo, useRef, useState } from "react";
import { ThinkingOrb } from "thinking-orbs";
import {
  Check,
  ChevronDown,
  FileText,
  Globe,
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
// confirm prompt) and collapsed to a one-line recap once finished - the raw
// step log is still there, just tucked away instead of permanently taking up
// transcript space.
export function AgentActivity({ steps, status, elapsedMs }: AgentActivityProps) {
  const hasPendingConfirm = steps.some((s) => s.kind === "confirm");
  const [userToggled, setUserToggled] = useState<boolean | null>(null);
  const expanded = userToggled ?? (status === "running" || hasPendingConfirm);

  // The elapsed counter used to re-render this whole activity card - every step row
  // in it - once a second, purely to change one number, and it kept ticking while the
  // window was hidden. Write the text straight into the node instead, and stop the
  // timer whenever the window is not visible.
  const elapsedRef = useRef<HTMLSpanElement | null>(null);
  useEffect(() => {
    if (status !== "running") return;
    const startedAt = Date.now();
    const paint = () => {
      if (elapsedRef.current) elapsedRef.current.textContent = ` for ${formatDuration(Date.now() - startedAt)}`;
    };
    let interval: ReturnType<typeof setInterval> | null = null;
    const start = () => { if (!interval) { paint(); interval = setInterval(paint, 1000); } };
    const stop = () => { if (interval) { clearInterval(interval); interval = null; } };
    const onVisibility = () => (document.hidden ? stop() : start());
    if (!document.hidden) start();
    document.addEventListener("visibilitychange", onVisibility);
    return () => { stop(); document.removeEventListener("visibilitychange", onVisibility); };
  }, [status]);

  const durationLabel = useMemo(
    () => (status === "running" ? null : typeof elapsedMs === "number" ? formatDuration(elapsedMs) : null),
    [status, elapsedMs],
  );

  return (
    <div className="max-w-full overflow-hidden rounded-2xl border border-border-default bg-bg-card">
      <button
        onClick={() => setUserToggled(!expanded)}
        className={`flex w-full items-center gap-2 px-3 py-2.5 text-left text-[12.5px] font-semibold transition hover:bg-bg-hover/60 ${
          status === "failed" ? "text-danger" : "text-text-primary"
        }`}
      >
        {status === "running" ? (
          // "solving" is the multi-step reasoning state, which is exactly what an
          // agent run is - a generic spinner said only that something was happening.
          <ThinkingOrb state="solving" size={20} theme="auto" aria-label="Agent working" className="flex-shrink-0" />
        ) : status === "failed" ? (
          <X size={13} className="flex-shrink-0" />
        ) : (
          <Check size={13} className="flex-shrink-0 text-success" />
        )}
        <span className="flex-1">
          {status === "running" ? <span className="shimmer-text">Working</span> : status === "failed" ? "Failed" : "Worked"}
          {status === "running"
            ? <span ref={elapsedRef} className="font-normal text-text-muted" />
            : durationLabel && <span className="font-normal text-text-muted"> for {durationLabel}</span>}
        </span>
        <ChevronDown size={14} className={`flex-shrink-0 text-text-muted transition-transform ${expanded ? "rotate-180" : ""}`} />
      </button>

      {expanded && (
        <div className="flex flex-col gap-1 border-t border-border-soft px-3 py-3">
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
// row is a single compact line, not a full document - so this renders just
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
        <code key={key++} className="rounded bg-bg-tertiary px-1 py-0.5 font-mono text-[10px]">
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
  if (step.kind === "plan" || step.kind === "thinking" || step.kind === "phase-summary") return null;
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
  // A rollup ("Explored 12 files, 3 searches +367 -14") isn't a timeline
  // entry the way a tool call is - no icon/rail, just a small muted line
  // under whatever heading it summarizes, same as Cursor's grey recap lines.
  if (step.kind === "phase-summary") {
    return <div className="pb-3 pl-1 text-[10.5px] leading-[1.5] text-text-muted">{step.message}</div>;
  }

  const Icon = iconForStep(step);
  const textClass =
    step.kind === "tool"
      ? "text-text-primary font-mono text-[10.5px]"
      : step.kind === "tool-result"
        ? "text-text-muted text-[10.5px]"
        : step.kind === "error"
          ? "text-danger"
          : step.kind === "thinking"
            ? "font-semibold text-text-primary"
            : "text-text-secondary"; // "plan"

  const iconClass =
    step.kind === "tool"
      ? "bg-accent-soft border-accent/35 text-accent"
      : step.kind === "error"
        ? "bg-danger-soft border-danger/40 text-danger"
        : "bg-bg-tertiary border-border-default text-text-secondary";

  // The model's OPENING read of the task gets a distinct card - same idea as
  // Cursor's separate "Thought for Xs" block - so it visually reads as the
  // model's reasoning. Later narration ("thinking", between tool bursts)
  // renders as a plain bold heading instead, matching Cursor's un-boxed
  // phase-transition lines.
  const bodyWrapClass = step.kind === "plan" ? "rounded-lg border border-accent/20 bg-accent-soft px-2.5 py-2" : "";

  return (
    <div className="flex gap-2">
      <div className="flex w-4 flex-shrink-0 flex-col items-center">
        <span className={`flex h-[15px] w-[15px] flex-shrink-0 items-center justify-center rounded-full border ${iconClass}`}>
          {Icon ? <Icon size={8.5} /> : <span className="h-1 w-1 rounded-full bg-current" />}
        </span>
        <span className={`mt-0.5 w-px flex-1 ${isLast ? "bg-transparent" : "bg-border-default"}`} style={{ minHeight: 6 }} />
      </div>
      <div className={`flex-1 pb-3 text-[11px] leading-[1.5] ${textClass}`}>
        <div className={bodyWrapClass}>
          {renderInline(step.kind === "error" ? friendlyError(step.message) : step.message)}
        </div>
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
    <div className="my-1.5 rounded-xl border border-warning/45 bg-warning-soft px-3 py-3 shadow-[0_2px_12px_rgba(255,183,77,0.12)]">
      <div className="mb-2.5 text-[12px] leading-[1.45] text-text-primary">
        {decided ? (decided === "allow" ? `✅ Allowed: ${message}` : `🚫 Denied: ${message}`) : `🔒 ${message}`}
      </div>
      {!decided && (
        <div className="flex gap-2">
          <button
            onClick={() => decide(true)}
            className="flex-1 rounded-lg border border-success/35 bg-success-soft px-3 py-2 text-[12px] font-bold text-success transition hover:-translate-y-px hover:bg-success/30"
          >
            Allow
          </button>
          <button
            onClick={() => decide(false)}
            className="flex-1 rounded-lg border border-danger/35 bg-danger-soft px-3 py-2 text-[12px] font-bold text-danger transition hover:-translate-y-px hover:bg-danger/30"
          >
            Deny
          </button>
        </div>
      )}
    </div>
  );
}
