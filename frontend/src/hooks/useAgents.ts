import { useCallback, useEffect, useState } from "react";
import {
  agentActiveRuns,
  agentCancelAll,
  agentConfirm,
  linkCancelTask,
  linkClearFinished,
  linkInfo,
  linkNewPairCode,
  linkSetAcceptTasks,
  linkTasks,
  onAgentProgress,
  onBridgeStatus,
  onLinkTask,
  type LinkInfo,
  type LinkTask,
} from "../lib/tauri";

export interface PendingConfirm {
  requestId: string;
  message: string;
}

const OPEN = new Set(["queued", "running", "waiting", "stalled"]);
export const isOpenTask = (t: LinkTask) => OPEN.has(t.status);

/** Live view of the desktop ⇄ browser agent team: link/pairing state, every
 *  cross-agent task the watcher tracks (src-tauri/src/link.rs), and the
 *  Allow/Deny prompts of browser-requested runs - those runs aren't tied to a
 *  chat, so their confirmations surface here instead. */
export function useAgents() {
  const [info, setInfo] = useState<LinkInfo | null>(null);
  const [tasks, setTasks] = useState<LinkTask[]>([]);
  const [confirms, setConfirms] = useState<Record<string, PendingConfirm[]>>({});
  // Every run in flight on this PC (not only browser handoffs), straight from the agent loop.
  const [runs, setRuns] = useState<string[]>([]);

  const refreshInfo = useCallback(() => {
    linkInfo().then(setInfo).catch(() => {});
  }, []);

  useEffect(() => {
    refreshInfo();
    linkTasks().then(setTasks).catch(() => {});
    const unTask = onLinkTask((t) => {
      setTasks((prev) => {
        const i = prev.findIndex((p) => p.id === t.id);
        if (i === -1) return [...prev, t];
        const next = prev.slice();
        next[i] = t;
        return next;
      });
      if (!OPEN.has(t.status)) {
        setConfirms((c) => {
          if (!c[t.id]) return c;
          const { [t.id]: _, ...rest } = c;
          return rest;
        });
      }
    });
    const unStatus = onBridgeStatus(() => refreshInfo());
    // Progress events are the heartbeat of a run, so refresh the list on them (coalesced), and once
    // more shortly after the last one so a run that just ended drops off.
    let timer = 0;
    const refreshRuns = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => agentActiveRuns().then(setRuns).catch(() => {}), 350);
    };
    agentActiveRuns().then(setRuns).catch(() => {});
    const unProgress = onAgentProgress((p) => {
      refreshRuns();
      if (p.kind !== "confirm" || !p.request_id || !p.source?.startsWith("browser:")) return;
      const id = p.source.slice("browser:".length);
      setConfirms((c) => ({ ...c, [id]: [...(c[id] ?? []), { requestId: p.request_id!, message: p.message ?? "" }] }));
    });
    return () => {
      window.clearTimeout(timer);
      unTask.then((fn) => fn());
      unStatus.then((fn) => fn());
      unProgress.then((fn) => fn());
    };
  }, [refreshInfo]);

  const answer = useCallback(async (taskId: string, requestId: string, approved: boolean) => {
    setConfirms((c) => ({ ...c, [taskId]: (c[taskId] ?? []).filter((x) => x.requestId !== requestId) }));
    await agentConfirm(requestId, approved).catch(() => {});
  }, []);

  const cancel = useCallback((id: string) => linkCancelTask(id).catch(() => {}), []);
  const clearFinished = useCallback(() => linkClearFinished().then(setTasks).catch(() => {}), []);
  const setAccept = useCallback((v: boolean) => linkSetAcceptTasks(v).then(setInfo).catch(() => {}), []);
  const newCode = useCallback(() => linkNewPairCode().then(setInfo).catch(() => {}), []);
  const stopAll = useCallback(() => agentCancelAll().then(() => setRuns([])).catch(() => {}), []);

  const openCount = tasks.filter(isOpenTask).length;
  const waitingCount = Object.values(confirms).reduce((n, list) => n + list.length, 0);

  return { info, tasks, confirms, runs, openCount, waitingCount, answer, cancel, clearFinished, setAccept, newCode, refreshInfo, stopAll };
}
