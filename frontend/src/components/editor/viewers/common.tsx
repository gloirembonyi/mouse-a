import { ExternalLink, FileWarning, FolderSearch, Info } from "lucide-react";
import { fsOpenDefault, fsReveal } from "../../../lib/tauri";

/** Shimmering placeholder lines while a file loads. */
export function ViewerSkeleton() {
  return (
    <div className="flex flex-col gap-2 p-4">
      {[60, 80, 40, 70, 55].map((w, i) => (
        <div key={i} className="h-3 animate-pulse rounded bg-bg-hover" style={{ width: `${w}%` }} />
      ))}
    </div>
  );
}

/** Centered "can't show this here" state with the two ways out. */
export function EmptyNotice({ title, detail, path, icon }: { title: string; detail: string; path: string; icon?: React.ReactNode }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
      {icon ?? <FileWarning size={28} className="text-text-muted" />}
      <div>
        <p className="text-[13px] font-medium text-text-primary">{title}</p>
        <p className="mt-1 max-w-[340px] text-[12px] text-text-muted">{detail}</p>
      </div>
      <div className="flex gap-2">
        <button
          onClick={() => fsOpenDefault(path).catch(() => {})}
          className="btn-brand flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[12px] font-medium"
        >
          <ExternalLink size={12} />
          Open with default app
        </button>
        <button
          onClick={() => fsReveal(path).catch(() => {})}
          className="flex items-center gap-1.5 rounded-lg border border-border-default px-3 py-1.5 text-[12px] text-text-secondary transition hover:bg-bg-hover"
        >
          <FolderSearch size={12} />
          Show in Explorer
        </button>
      </div>
    </div>
  );
}

/** Thin info strip above a viewer (e.g. "showing the first 5,000 rows"). */
export function ViewerNote({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex flex-shrink-0 items-center gap-1.5 border-b border-border-soft bg-bg-secondary px-3 py-1 text-[11px] text-text-muted">
      <Info size={11} className="flex-shrink-0" />
      <span className="truncate">{children}</span>
    </div>
  );
}
