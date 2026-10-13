import { useEffect, useState } from "react";
import {
  agentCancel,
  agentConfirm,
  COPILOT_AGENT_SOURCE,
  copilotAgentRun,
  copilotInsert,
  copilotInsertCancel,
  onAgentProgress,
} from "../lib/tauri";

// State for the copilot bar's hands-on tools (copilot.rs) - kept apart from
// CopilotTools.tsx so that file only exports components.

// ── Fix it: the desktop agent, from the bar ──────────────────────

export interface AgentStep {
  kind: string;
  message: string;
}

export interface AgentRunState {
  task: string;
  steps: AgentStep[];
  confirm: { id: string; message: string } | null;
  text: string | null;
  error: string | null;
  running: boolean;
}

const ACTIVE_PROJECT_KEY = "mouse-ai-active-project";

/** Owns the copilot's single agent run and its live progress. */
export function useCopilotAgent() {
  const [run, setRun] = useState<AgentRunState | null>(null);

  useEffect(() => {
    const un = onAgentProgress((e) => {
      if (e.source !== COPILOT_AGENT_SOURCE) return;
      setRun((prev) => {
        if (!prev) return prev;
        if (e.kind === "confirm" && e.request_id) {
          return { ...prev, confirm: { id: e.request_id, message: e.message ?? "Allow this action?" } };
        }
        const message = (e.message ?? "").trim();
        if (!message || e.kind === "done" || e.kind === "error") return prev;
        return { ...prev, steps: [...prev.steps, { kind: e.kind, message }].slice(-40) };
      });
    });
    return () => void un.then((fn) => fn());
  }, []);

  const start = async (task: string) => {
    if (run?.running) return;
    setRun({ task: task.trim() || "Fix what just broke in my demo", steps: [], confirm: null, text: null, error: null, running: true });
    try {
      const { text } = await copilotAgentRun(task, localStorage.getItem(ACTIVE_PROJECT_KEY));
      setRun((prev) => prev && { ...prev, text, running: false, confirm: null });
    } catch (err) {
      setRun((prev) => prev && { ...prev, error: err instanceof Error ? err.message : String(err), running: false, confirm: null });
    }
  };

  const stop = () => void agentCancel(COPILOT_AGENT_SOURCE);
  const answer = (approved: boolean) => {
    if (!run?.confirm) return;
    void agentConfirm(run.confirm.id, approved);
    setRun((prev) => prev && { ...prev, confirm: null });
  };
  const dismiss = () => setRun((prev) => (prev?.running ? prev : null));

  return { run, start, stop, answer, dismiss };
}

// ── Private insert ───────────────────────────────────────────────

export type InsertFn = (text: string, mode: "paste" | "type", label: string) => void;

export interface InsertState {
  label: string;
  mode: "paste" | "type";
}

/** Owns the one-at-a-time insert into the user's app (and its Stop). */
export function useInsert() {
  const [busy, setBusy] = useState<InsertState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const insert: InsertFn = async (text, mode, label) => {
    if (busy) return;
    setError(null);
    setBusy({ label, mode });
    try {
      const { completed } = await copilotInsert(text, mode);
      if (!completed) setError("Stopped - you clicked away or pressed Stop.");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  };
  return { busy, error, clearError: () => setError(null), insert, cancel: () => void copilotInsertCancel() };
}

const FENCE = /```([\w+#.-]*)[^\n]*\n([\s\S]*?)```/g;

export function codeBlocks(text: string): { lang: string; code: string }[] {
  return [...text.matchAll(FENCE)].map((m) => ({ lang: m[1] || "code", code: m[2].replace(/\n$/, "") }));
}

/** The line to actually say: the **Say:** line if there is one, else the
 *  answer with markdown stripped. */
export function speakable(text: string) {
  const say = /\*\*Say:\*\*\s*(.+)/i.exec(text);
  const body = say ? say[1] : text.replace(FENCE, "");
  return body
    .replace(/\*\*|__|`/g, "")
    .replace(/^\s*[-*]\s+/gm, "")
    .replace(/^#+\s*/gm, "")
    .trim();
}

