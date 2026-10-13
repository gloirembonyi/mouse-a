// A tiny "open this file in the editor" channel. Anything (a file path in a
// chat answer, the composer, a menu) can request a file; the editor dock
// subscribes when mounted. Requests made before it mounts (the dock loads
// lazily) are queued and replayed, and App opens the dock on every request.
type Listener = (path: string) => void;

const listeners = new Set<Listener>();
const pending: string[] = [];

export const OPEN_DOCK_EVENT = "mouse-ai:open-dock";

export type DockTab = "editor" | "terminal" | "changes";

/** Show the dock (and optionally switch it to a tab). */
export function showDock(tab?: DockTab) {
  window.dispatchEvent(new CustomEvent<DockTab | undefined>(OPEN_DOCK_EVENT, { detail: tab }));
}

export function requestOpenFile(path: string) {
  if (listeners.size) listeners.forEach((l) => l(path));
  else pending.push(path);
  showDock("editor");
}

// ── "Run this in the terminal" (from chat code blocks) ──
const runListeners = new Set<(command: string) => void>();
const pendingRuns: string[] = [];

export function requestRunInTerminal(command: string) {
  if (runListeners.size) runListeners.forEach((l) => l(command));
  else pendingRuns.push(command);
  showDock("terminal");
}

export function subscribeRunInTerminal(listener: (command: string) => void) {
  runListeners.add(listener);
  pendingRuns.splice(0).forEach(listener);
  return () => {
    runListeners.delete(listener);
  };
}

export function subscribeOpenFile(listener: Listener) {
  listeners.add(listener);
  pending.splice(0).forEach(listener);
  return () => {
    listeners.delete(listener);
  };
}

// Absolute Windows path to a file with an extension: "C:\dir\file.md",
// "C:/dir/file.ts" or a UNC "\\server\share\file.txt".
const FILE_PATH = /^(?:[A-Za-z]:[/\\]|\\\\)[^<>"|?*\n]*[/\\][^<>"|?*\n/\\]+\.[A-Za-z0-9]{1,10}$/;

export function looksLikeFilePath(text: string) {
  return FILE_PATH.test(text.trim());
}
