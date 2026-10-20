// In-app updates (tauri-plugin-updater + tauri-plugin-process).
//
// CI signs every release and uploads a static `latest.json` to the GitHub
// release; `check()` reads it from the endpoint in tauri.conf.json
// (plugins.updater) and verifies the download against the embedded public key.
// One module-level store is shared by the banner (UpdateBanner) and Settings ->
// About, so a check started in one place shows up in the other.
import { useSyncExternalStore } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { installKind, type InstallKind } from "./tauri";

export const DOWNLOAD_PAGE = "https://kuduagent.vercel.app/download";

const AUTO_KEY = "mouse-ai-update-auto";
const DISMISSED_KEY = "mouse-ai-update-dismissed";
/** Set to "on" to exercise the updater from `npm run dev`. */
const DEBUG_KEY = "mouse-ai-update-debug";

const FIRST_CHECK_MS = 10_000;
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

export type UpdatePhase =
  | "idle"
  | "checking"
  | "uptodate"
  | "available"
  | "downloading"
  | "installing"
  | "restart"
  | "error";

export interface UpdaterState {
  phase: UpdatePhase;
  /** The announced version, while one is known. */
  version: string | null;
  /** Release notes (markdown) from latest.json. */
  notes: string | null;
  /** Bytes received / total (total is null when the server sent no length). */
  downloaded: number;
  total: number | null;
  error: string | null;
  /** "check" errors stay in Settings; "install" errors also show on the banner. */
  errorFrom: "check" | "install" | null;
  installKind: InstallKind | null;
  /** Version the user closed the banner for - Settings still shows it. */
  dismissed: string | null;
  autoCheck: boolean;
}

const read = (key: string) => {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
};
const write = (key: string, value: string | null) => {
  try {
    if (value == null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    /* storage unavailable: the preference just isn't remembered */
  }
};

let state: UpdaterState = {
  phase: "idle",
  version: null,
  notes: null,
  downloaded: 0,
  total: null,
  error: null,
  errorFrom: null,
  installKind: null,
  dismissed: read(DISMISSED_KEY),
  autoCheck: read(AUTO_KEY) !== "off",
};
let pending: Update | null = null;
const listeners = new Set<() => void>();

const set = (patch: Partial<UpdaterState>) => {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
};
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export const useUpdater = () => useSyncExternalStore(subscribe, () => state);

const message = (err: unknown) => (err instanceof Error ? err.message : String(err || "Unknown error"));

/** Updates are skipped in `npm run dev` (the dev binary isn't an installed copy)
 *  unless the debug flag is set. */
export const updaterEnabled = () => !import.meta.env.DEV || read(DEBUG_KEY) === "on";

/** True when this install can be replaced in place (everything but a Linux .deb/.rpm). */
export const canInstallInApp = (s: UpdaterState) => s.installKind !== "package";

/** Look for a new release. `manual` checks report failures; background ones stay quiet. */
export async function checkForUpdates({ manual = false } = {}): Promise<void> {
  if (!updaterEnabled() && !manual) return;
  if (state.phase === "checking" || state.phase === "downloading" || state.phase === "installing" || state.phase === "restart") return;
  set({ phase: "checking", error: null, errorFrom: null });
  try {
    if (state.installKind == null) set({ installKind: await installKind().catch(() => null) });
    const update = await check({ timeout: 30_000 });
    if (pending && pending !== update) void pending.close().catch(() => {});
    pending = update;
    if (update) {
      set({ phase: "available", version: update.version, notes: update.body?.trim() || null, downloaded: 0, total: null });
    } else {
      set({ phase: "uptodate", version: null, notes: null });
    }
  } catch (err) {
    // A failed background check (offline, GitHub hiccup) leaves the last known state alone.
    if (manual) set({ phase: "error", error: message(err), errorFrom: "check" });
    else set({ phase: pending ? "available" : "idle" });
  }
}

/** Download + install the pending update, then restart into it. On Windows the
 *  passive NSIS installer closes the app itself and relaunches it when done. */
export async function installUpdate(): Promise<void> {
  if (!pending || !canInstallInApp(state)) return;
  if (state.phase === "downloading" || state.phase === "installing") return;
  set({ phase: "downloading", downloaded: 0, total: null, error: null, errorFrom: null });
  try {
    let downloaded = 0;
    await pending.downloadAndInstall((event) => {
      if (event.event === "Started") set({ total: event.data.contentLength ?? null });
      else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        set({ downloaded });
      } else if (event.event === "Finished") set({ phase: "installing" });
    });
    set({ phase: "restart" });
    await restartApp();
  } catch (err) {
    set({ phase: "error", error: message(err), errorFrom: "install" });
  }
}

/** Relaunch into the installed version. Leaves the "restart" state up if it fails. */
export async function restartApp(): Promise<void> {
  try {
    await relaunch();
  } catch (err) {
    set({ phase: "restart", error: `Restart failed: ${message(err)}. Quit and reopen Kudu Desktop to finish.` });
  }
}

/** Hide the banner for this version; Settings -> About still offers it. */
export function dismissUpdate() {
  if (!state.version) return;
  write(DISMISSED_KEY, state.version);
  set({ dismissed: state.version });
}

export function setAutoCheck(on: boolean) {
  write(AUTO_KEY, on ? "on" : "off");
  set({ autoCheck: on });
  if (on) void checkForUpdates();
}

let started = false;
/** First check ~10 s after launch (never blocking startup), then every 6 h while auto-check is on. */
export function startAutoUpdateChecks(): () => void {
  if (started || !updaterEnabled()) return () => {};
  started = true;
  const tick = () => {
    if (state.autoCheck) void checkForUpdates();
  };
  const first = window.setTimeout(tick, FIRST_CHECK_MS);
  const every = window.setInterval(tick, CHECK_EVERY_MS);
  return () => {
    started = false;
    window.clearTimeout(first);
    window.clearInterval(every);
  };
}
