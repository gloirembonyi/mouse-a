import { useEffect, useState } from "react";
import clsx from "clsx";
import {
  ArrowRight,
  Check,
  ChevronDown,
  Copy,
  Globe,
  KeyRound,
  Monitor,
  RefreshCw,
  ShieldCheck,
  Square,
  Trash2,
  X,
} from "lucide-react";
import { BotAvatar, type BotKind, type BotMood } from "../ui/BotAvatar";
import { AGENT_BOT } from "../../lib/bots";
import { IconButton, Switch } from "../ui/primitives";
import { formatDuration } from "../../lib/format";
import { isOpenTask, type useAgents } from "../../hooks/useAgents";
import type { LinkTask } from "../../lib/tauri";

interface AgentsViewProps {
  agents: ReturnType<typeof useAgents>;
  onClose: () => void;
}

const STATUS_STYLE: Record<LinkTask["status"], { label: string; className: string }> = {
  queued: { label: "Queued", className: "bg-bg-hover text-text-secondary" },
  running: { label: "Working", className: "bg-accent-soft text-accent" },
  waiting: { label: "Needs you", className: "bg-warning-soft text-warning" },
  stalled: { label: "Quiet - checking", className: "bg-warning-soft text-warning" },
  done: { label: "Done", className: "bg-success-soft text-success" },
  failed: { label: "Failed", className: "bg-danger-soft text-danger" },
  cancelled: { label: "Cancelled", className: "bg-bg-hover text-text-muted" },
};

// The "watcher" made visible: the two agents as one team, the link between
// them, and every handoff with its live steps, status and any approval it's
// waiting on.
export function AgentsView({ agents, onClose }: AgentsViewProps) {
  const { info, tasks } = agents;
  const ordered = [...tasks].sort((a, b) => Number(isOpenTask(b)) - Number(isOpenTask(a)) || b.updated_ms - a.updated_ms);
  const anyRunning = tasks.some(isOpenTask);
  const toBrowser = tasks.some((t) => isOpenTask(t) && t.direction === "to_browser");
  const fromBrowser = tasks.some((t) => isOpenTask(t) && t.direction === "from_browser");
  const linked = !!info?.connected;
  const finished = tasks.filter((t) => !isOpenTask(t)).length;

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-12 flex-shrink-0 items-center justify-between border-b border-border-soft px-5">
        <h2 className="text-[14px] font-semibold">Agents</h2>
        <IconButton title="Back to chat" onClick={onClose}>
          <X size={16} />
        </IconButton>
      </div>

      <div className="scrollbar-thin flex-1 overflow-y-auto">
        <div className="brand-wash">
          <div className="mx-auto flex max-w-[720px] flex-col items-center px-6 pb-6 pt-8">
            {/* The team */}
            <div className="flex items-center gap-4">
              <TeamMember
                name="Kudu Desktop"
                role="Desktop agent"
                kind={AGENT_BOT.desktop}
                mood={fromBrowser ? "working" : toBrowser ? "thinking" : "idle"}
                icon={<Monitor size={11} />}
              />
              <LinkLine active={anyRunning} linked={linked} reverse={fromBrowser && !toBrowser} />
              <TeamMember
                name="Kudu Connect"
                role="Browser agent"
                kind={AGENT_BOT.browser}
                mood={!linked ? "sleeping" : toBrowser ? "working" : fromBrowser ? "thinking" : "idle"}
                icon={<Globe size={11} />}
              />
            </div>
            <p className="mt-5 max-w-[460px] text-center text-[12.5px] text-text-muted">
              Each agent hands the other what it does best - Kudu Connect does web work, Kudu Desktop does the
              terminal, files and apps on your PC - and the watcher keeps an eye on every handoff.
            </p>
          </div>
        </div>

        <div className="mx-auto flex max-w-[720px] flex-col gap-6 px-6 pb-10">
          <LinkCard agents={agents} />

          <section className="flex flex-col gap-2.5">
            <div className="flex items-center justify-between">
              <h3 className="text-[13px] font-semibold">Handoffs</h3>
              {finished > 0 && (
                <button
                  onClick={agents.clearFinished}
                  className="flex items-center gap-1 rounded-md px-2 py-1 text-[11.5px] text-text-muted transition hover:bg-bg-hover hover:text-text-primary"
                >
                  <Trash2 size={12} />
                  Clear finished
                </button>
              )}
            </div>
            {ordered.length === 0 ? (
              <div className="flex flex-col items-center gap-2 rounded-2xl border border-dashed border-border-default px-6 py-8 text-center">
                <BotAvatar kind="square" mood="sleeping" size={40} />
                <p className="text-[12.5px] text-text-secondary">No handoffs yet</p>
                <p className="max-w-[380px] text-[12px] text-text-muted">
                  Try “Find the cheapest flight to Kigali next Friday and save the options to a file on my Desktop” -
                  the browser does the search, Kudu writes the file.
                </p>
              </div>
            ) : (
              ordered.map((t) => <TaskCard key={t.id} task={t} agents={agents} />)
            )}
          </section>
        </div>
      </div>
    </div>
  );
}

function TeamMember({
  name,
  role,
  kind,
  mood,
  icon,
}: {
  name: string;
  role: string;
  kind: BotKind;
  mood: BotMood;
  icon: React.ReactNode;
}) {
  return (
    <div className="flex w-[120px] flex-col items-center gap-2">
      <BotAvatar kind={kind} mood={mood} size={kind === "kudu" ? 72 : 64} variant={kind === "kudu" ? "body" : "mascot"} />
      <div className="text-center">
        <div className="text-[13px] font-semibold">{name}</div>
        <div className="flex items-center justify-center gap-1 text-[11px] text-text-muted">
          {icon}
          {role}
        </div>
      </div>
    </div>
  );
}

/** Dashed line between the two bots; dashes flow toward whoever is working. */
function LinkLine({ active, linked, reverse }: { active: boolean; linked: boolean; reverse: boolean }) {
  return (
    <div className="relative -mt-8 flex w-[110px] items-center">
      <svg viewBox="0 0 110 10" className="h-3 w-full" preserveAspectRatio="none">
        <line
          x1="0"
          y1="5"
          x2="110"
          y2="5"
          stroke={linked ? "var(--mx-accent)" : "var(--mx-border-strong)"}
          strokeWidth="2"
          strokeLinecap="round"
          strokeDasharray={linked ? "6 6" : "2 6"}
          style={active ? { animation: `link-flow 0.8s linear infinite${reverse ? " reverse" : ""}` } : undefined}
        />
      </svg>
      <span
        className={clsx(
          "absolute left-1/2 top-1/2 flex h-6 w-6 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full border text-[10px]",
          linked ? "border-accent/40 bg-bg-elevated text-accent" : "border-border-default bg-bg-elevated text-text-muted",
        )}
        title={linked ? "Linked" : "Not connected"}
      >
        {linked ? <Check size={12} /> : <X size={12} />}
      </span>
    </div>
  );
}

export function LinkCard({ agents }: { agents: ReturnType<typeof useAgents> }) {
  const { info } = agents;
  const [copied, setCopied] = useState(false);
  if (!info) return null;

  const copy = async () => {
    await navigator.clipboard.writeText(info.pair_code).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const state = !info.connected ? "offline" : info.paired ? "paired" : "unpaired";

  return (
    <section className="flex flex-col gap-3 rounded-2xl border border-border-default bg-bg-elevated p-4">
      <div className="flex items-center gap-2.5">
        <span
          className={clsx(
            "flex h-8 w-8 items-center justify-center rounded-xl",
            state === "paired" ? "bg-success-soft text-success" : state === "unpaired" ? "bg-warning-soft text-warning" : "bg-bg-hover text-text-muted",
          )}
        >
          {state === "paired" ? <ShieldCheck size={16} /> : <KeyRound size={16} />}
        </span>
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">
            {state === "paired"
              ? info.via === "account"
                ? "Linked to your account"
                : "Linked and paired"
              : state === "unpaired"
                ? "Connected - not paired yet"
                : "Browser not connected"}
            {(info.browsers ?? 0) > 1 && <span className="ml-1.5 font-normal text-text-muted">· {info.browsers} browsers</span>}
          </div>
          <div className="text-[11.5px] text-text-muted">
            {state === "paired"
              ? "Both agents can hand each other tasks."
              : state === "unpaired"
                ? "Sign in to the extension with the same Google account and they link by themselves - or pair with the code below."
                : "Open Chrome/Edge with the Kudu Connect extension signed in to your account - it links automatically."}
          </div>
        </div>
        <IconButton title="Refresh" size="sm" onClick={agents.refreshInfo}>
          <RefreshCw size={13} />
        </IconButton>
      </div>

      <details className="group" open={state !== "paired" || info.via !== "account"}>
      <summary className="cursor-pointer list-none text-[11.5px] text-text-muted hover:text-text-primary group-open:hidden">
        Use a pairing code instead
      </summary>
      <div className="flex flex-wrap items-center gap-3 rounded-xl bg-bg-card px-3 py-2.5">
        <span className="text-[12px] text-text-secondary">Pairing code</span>
        <code className="rounded-md border border-border-default bg-bg-primary px-2 py-1 font-mono text-[13px] font-semibold tracking-wider">
          {info.pair_code}
        </code>
        <button
          onClick={copy}
          className="flex items-center gap-1 rounded-md px-2 py-1 text-[12px] text-text-secondary transition hover:bg-bg-hover hover:text-text-primary"
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
          {copied ? "Copied" : "Copy"}
        </button>
        <button
          onClick={agents.newCode}
          title="Make a new code - the browser has to be paired again"
          className="ml-auto text-[11.5px] text-text-muted underline-offset-2 hover:text-text-primary hover:underline"
        >
          New code
        </button>
      </div>
      <p className="mt-2 text-[11.5px] text-text-muted">
        For a browser signed in to another account: paste it into the extension's popup → Settings → Kudu Desktop link → Desktop pairing code.
      </p>
      </details>

      <label className="flex cursor-pointer items-center justify-between gap-3 border-t border-border-soft pt-3">
        <span>
          <span className="block text-[12.5px] font-medium">Accept tasks from the browser agent</span>
          <span className="block text-[11.5px] text-text-muted">
            Lets it ask Kudu to run commands, save files and use apps. Risky steps still ask you first.
          </span>
        </span>
        <Switch on={info.accept_tasks} onChange={agents.setAccept} label="Accept tasks from the browser agent" />
      </label>
    </section>
  );
}

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

function TaskCard({ task, agents }: { task: LinkTask; agents: ReturnType<typeof useAgents> }) {
  const open = isOpenTask(task);
  const [expanded, setExpanded] = useState(open);
  const now = useNow(open);
  const confirms = agents.confirms[task.id] ?? [];
  const style = STATUS_STYLE[task.status];
  const fromBrowser = task.direction === "from_browser";
  const elapsed = (open ? now : task.updated_ms) - task.started_ms;
  const last = task.steps[task.steps.length - 1];

  return (
    <div
      data-active={task.status === "running"}
      className="border-beam animate-fade-up relative overflow-hidden rounded-2xl border border-border-default bg-bg-elevated"
    >
      <button onClick={() => setExpanded((v) => !v)} className="flex w-full items-start gap-3 px-4 py-3 text-left">
        <span className="flex flex-shrink-0 items-center pt-0.5">
          <BotAvatar kind={fromBrowser ? AGENT_BOT.browser : AGENT_BOT.desktop} mood="idle" size={22} />
          <ArrowRight size={12} className="mx-0.5 text-text-muted" />
          <BotAvatar
            kind={fromBrowser ? AGENT_BOT.desktop : AGENT_BOT.browser}
            mood={task.status === "running" ? "working" : task.status === "failed" ? "error" : task.status === "done" ? "happy" : "idle"}
            size={22}
          />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            <span className="text-[11px] text-text-muted">{fromBrowser ? "Browser asked Kudu" : "Kudu asked the browser"}</span>
            <span className={clsx("rounded-full px-2 py-px text-[10.5px] font-medium", style.className)}>{task.status === "queued" && task.queue_position ? `Queued #${task.queue_position}` : style.label}</span>
            <span className="ml-auto text-[11px] tabular-nums text-text-muted">{formatDuration(elapsed)}</span>
          </span>
          <span className="mt-0.5 line-clamp-2 block text-[13px] text-text-primary">{task.title}</span>
          {!expanded && last && open && <span className="mt-1 block truncate text-[11.5px] text-text-muted">{last.message}</span>}
        </span>
        <ChevronDown size={14} className={clsx("mt-1 flex-shrink-0 text-text-muted transition-transform", expanded && "rotate-180")} />
      </button>

      {confirms.map((c) => (
        <div key={c.requestId} className="mx-4 mb-3 rounded-xl border border-warning/40 bg-warning-soft p-3">
          <p className="mb-2.5 text-[12.5px] text-text-primary">🔒 {c.message}</p>
          <div className="flex gap-2">
            <button
              onClick={() => agents.answer(task.id, c.requestId, true)}
              className="btn-brand flex-1 rounded-lg py-1.5 text-[12.5px] font-medium"
            >
              Allow
            </button>
            <button
              onClick={() => agents.answer(task.id, c.requestId, false)}
              className="flex-1 rounded-lg border border-border-default py-1.5 text-[12.5px] font-medium transition hover:bg-bg-hover"
            >
              Deny
            </button>
          </div>
        </div>
      ))}

      {expanded && (
        <div className="border-t border-border-soft px-4 py-3">
          {task.steps.length === 0 ? (
            <p className="text-[12px] text-text-muted">{open ? "Starting…" : "No steps recorded."}</p>
          ) : (
            <ol className="flex flex-col gap-1.5">
              {task.steps.map((s, i) => (
                <li key={i} className="flex gap-2.5 text-[12px]">
                  <span
                    className={clsx(
                      "mt-[6px] h-1.5 w-1.5 flex-shrink-0 rounded-full",
                      s.kind === "error" ? "bg-danger" : s.kind === "confirm" ? "bg-warning" : s.kind === "tool" ? "bg-accent" : "bg-border-strong",
                    )}
                  />
                  <span className={clsx("min-w-0 flex-1 break-words", s.kind === "tool" ? "text-text-primary" : "text-text-secondary")}>
                    {s.message}
                  </span>
                  <span className="flex-shrink-0 text-[10.5px] tabular-nums text-text-muted">
                    {formatDuration(Math.max(1000, s.at_ms - task.started_ms))}
                  </span>
                </li>
              ))}
            </ol>
          )}
          {task.result && (
            <div className="mt-3 rounded-xl bg-success-soft px-3 py-2 text-[12px] text-text-primary">
              <span className="font-medium text-success">Result · </span>
              {task.result}
            </div>
          )}
          {task.error && task.status === "failed" && (
            <div className="mt-3 rounded-xl bg-danger-soft px-3 py-2 text-[12px] text-danger">{task.error}</div>
          )}
          {open && (
            <button
              onClick={() => agents.cancel(task.id)}
              className="mt-3 flex items-center gap-1.5 rounded-lg border border-border-default px-2.5 py-1 text-[12px] text-text-secondary transition hover:border-danger/40 hover:bg-danger-soft hover:text-danger"
            >
              <Square size={10} fill="currentColor" />
              Stop this handoff
            </button>
          )}
        </div>
      )}
    </div>
  );
}
