import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ArrowUpCircle, ExternalLink, X } from "lucide-react";
import { Markdown } from "../chat/Markdown";
import {
  DOWNLOAD_PAGE,
  canInstallInApp,
  dismissUpdate,
  installUpdate,
  restartApp,
  useUpdater,
  type UpdaterState,
} from "../../lib/updater";

/** Calm bottom-right card announcing a new release (lib/updater.ts runs the checks,
 *  started from App).
 *  Closing it remembers the version, so it doesn't come back every launch;
 *  Settings -> About keeps offering the update. */
export function UpdateBanner() {
  const s = useUpdater();
  const [notesOpen, setNotesOpen] = useState(false);

  const busy = s.phase === "downloading" || s.phase === "installing" || s.phase === "restart";
  const installFailed = s.phase === "error" && s.errorFrom === "install";
  const show = s.version != null && (busy || installFailed || (s.phase === "available" && s.dismissed !== s.version));
  if (!show) return null;

  return (
    <>
      <div
        role="status"
        className="absolute bottom-4 right-4 z-[900] flex w-[340px] flex-col gap-2.5 rounded-xl border border-border-default bg-bg-elevated p-3.5 shadow-pop"
      >
        <div className="flex items-start gap-2.5">
          <ArrowUpCircle size={16} className="mt-0.5 flex-shrink-0 text-accent" />
          <div className="min-w-0 flex-1 text-[12.5px] leading-snug">
            <span className="font-medium text-text-primary">Kudu Desktop v{s.version} is available</span>
            {s.notes && (
              <>
                <span className="text-text-muted"> · </span>
                <button onClick={() => setNotesOpen(true)} className="text-accent hover:underline">
                  What's new
                </button>
              </>
            )}
          </div>
          {!busy && (
            <button
              onClick={dismissUpdate}
              title="Not now"
              aria-label="Not now"
              className="-m-1 rounded-md p-1 text-text-muted transition hover:bg-bg-hover hover:text-text-primary"
            >
              <X size={14} />
            </button>
          )}
        </div>
        <UpdateActions s={s} />
      </div>
      {notesOpen && <ReleaseNotesDialog version={s.version!} notes={s.notes ?? ""} onClose={() => setNotesOpen(false)} />}
    </>
  );
}

/** The action row shared by the banner and Settings: install button, progress,
 *  restart, package-manager notice, or the error with the website fallback. */
export function UpdateActions({ s }: { s: UpdaterState }) {
  if (!canInstallInApp(s)) {
    return (
      <p className="text-[12px] text-text-muted">
        This copy was installed from a .deb/.rpm package - update it with your package manager or{" "}
        <WebsiteLink>download it from the website</WebsiteLink>.
      </p>
    );
  }
  if (s.phase === "downloading" || s.phase === "installing") return <UpdateProgress s={s} />;
  if (s.phase === "restart") {
    return (
      <div className="flex flex-col gap-1.5">
        <button onClick={() => void restartApp()} className="btn-brand self-start rounded-lg px-3 py-1.5 text-[12.5px] font-medium">
          Restart to finish
        </button>
        {s.error && <p className="text-[12px] text-danger">{s.error}</p>}
      </div>
    );
  }
  if (s.phase === "error" && s.errorFrom === "install") {
    return (
      <div className="flex flex-col gap-1.5">
        <p className="text-[12px] text-danger">The update couldn't be installed: {s.error}</p>
        <div className="flex items-center gap-3">
          <button onClick={() => void installUpdate()} className="btn-brand rounded-lg px-3 py-1.5 text-[12.5px] font-medium">
            Try again
          </button>
          <WebsiteLink>Download from the website</WebsiteLink>
        </div>
      </div>
    );
  }
  return (
    <button onClick={() => void installUpdate()} className="btn-brand self-start rounded-lg px-3 py-1.5 text-[12.5px] font-medium">
      Update &amp; restart
    </button>
  );
}

export function UpdateProgress({ s }: { s: UpdaterState }) {
  const pct = s.total ? Math.min(100, Math.round((s.downloaded / s.total) * 100)) : null;
  const label =
    s.phase === "installing"
      ? "Installing… Kudu Desktop restarts when it's done."
      : pct != null
        ? `Downloading… ${pct}%`
        : `Downloading… ${(s.downloaded / 1048576).toFixed(1)} MB`;
  return (
    <div className="flex flex-col gap-1.5">
      <div
        className="h-1.5 overflow-hidden rounded-full bg-bg-tertiary"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct ?? undefined}
      >
        <div
          className={pct == null || s.phase === "installing" ? "h-full w-1/3 animate-pulse rounded-full bg-accent" : "h-full rounded-full bg-accent transition-[width] duration-200"}
          style={pct != null && s.phase !== "installing" ? { width: `${pct}%` } : undefined}
        />
      </div>
      <span className="text-[11.5px] text-text-muted">{label}</span>
    </div>
  );
}

export function WebsiteLink({ children }: { children: React.ReactNode }) {
  return (
    <button
      onClick={() => void openUrl(DOWNLOAD_PAGE).catch(() => {})}
      className="inline-flex items-center gap-1 text-[12px] text-accent hover:underline"
    >
      {children}
      <ExternalLink size={11} />
    </button>
  );
}

export function ReleaseNotesDialog({ version, notes, onClose }: { version: string; notes: string; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return createPortal(
    <div className="fixed inset-0 z-[1000] flex items-center justify-center bg-black/50 p-4" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-label={`What's new in v${version}`}
        className="flex max-h-[70vh] w-full max-w-[520px] flex-col overflow-hidden rounded-2xl border border-border-default bg-bg-primary shadow-pop"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex flex-shrink-0 items-center gap-2 border-b border-border-soft px-4 py-2.5">
          <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-text-primary">What's new in v{version}</span>
          <button onClick={onClose} title="Close (Esc)" className="rounded-md p-1 text-text-muted transition hover:bg-bg-hover hover:text-text-primary">
            <X size={15} />
          </button>
        </div>
        <div className="scrollbar-thin min-h-0 flex-1 overflow-auto px-4 py-3 text-[13px]">
          <Markdown>{notes || "No release notes for this version."}</Markdown>
        </div>
      </div>
    </div>,
    document.body,
  );
}
