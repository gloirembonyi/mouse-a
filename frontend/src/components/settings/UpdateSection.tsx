import { useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import { Switch } from "../ui/primitives";
import { ReleaseNotesDialog, UpdateActions, WebsiteLink } from "../layout/UpdateBanner";
import { checkForUpdates, setAutoCheck, updaterEnabled, useUpdater } from "../../lib/updater";

/** Settings -> About: current version, a manual check, and the auto-check switch.
 *  Shares its state with the banner (lib/updater.ts), and keeps offering an
 *  update the banner was dismissed for. */
export function UpdateSection({ appVersion }: { appVersion: string }) {
  const s = useUpdater();
  const [notesOpen, setNotesOpen] = useState(false);
  const busy = s.phase === "downloading" || s.phase === "installing" || s.phase === "restart";
  const hasUpdate = s.version != null && (s.phase === "available" || busy || (s.phase === "error" && s.errorFrom === "install"));

  return (
    <div className="flex flex-col gap-2.5 rounded-xl border border-border-soft bg-bg-card px-3.5 py-3">
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 flex-col">
          <span className="text-[13px] text-text-primary">Version {appVersion}</span>
          <span className="text-[11.5px] text-text-muted">
            {s.phase === "checking"
              ? "Checking for updates…"
              : s.phase === "uptodate"
                ? "You're up to date."
                : hasUpdate
                  ? `Version ${s.version} is available.`
                  : !updaterEnabled()
                    ? "Development build - updates are off."
                    : "Updates come from the signed Kudu Desktop releases."}
            {hasUpdate && s.notes && (
              <>
                {" "}
                <button onClick={() => setNotesOpen(true)} className="text-accent hover:underline">
                  What's new
                </button>
              </>
            )}
          </span>
        </div>
        {!busy && (
          <button
            onClick={() => void checkForUpdates({ manual: true })}
            disabled={s.phase === "checking"}
            className="inline-flex flex-shrink-0 items-center gap-1.5 rounded-lg border border-border-default px-3 py-1.5 text-[12.5px] font-medium transition hover:bg-bg-hover disabled:opacity-60"
          >
            {s.phase === "checking" ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
            Check for updates
          </button>
        )}
      </div>

      {hasUpdate && <UpdateActions s={s} />}
      {s.phase === "error" && s.errorFrom === "check" && (
        <p className="text-[12px] text-danger">
          Couldn't check for updates: {s.error} <WebsiteLink>Download from the website</WebsiteLink>
        </p>
      )}

      <label className="flex cursor-pointer items-center justify-between gap-4 border-t border-border-soft pt-2.5">
        <span className="flex flex-col">
          <span className="text-[13px] text-text-primary">Automatically check for updates</span>
          <span className="text-[11.5px] text-text-muted">Shortly after launch and every 6 hours. Nothing installs without your click.</span>
        </span>
        <Switch on={s.autoCheck} onChange={setAutoCheck} label="Automatically check for updates" />
      </label>

      {notesOpen && s.version && <ReleaseNotesDialog version={s.version} notes={s.notes ?? ""} onClose={() => setNotesOpen(false)} />}
    </div>
  );
}
