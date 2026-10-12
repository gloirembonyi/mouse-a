import { useEffect, useState } from "react";
import { FileDiff, RefreshCw } from "lucide-react";
import { gitDiffFile, gitStatus, type ChangedFile } from "../../lib/tauri";

interface ChangesPaneProps {
  root: string;
}

const statusLabel: Record<string, string> = {
  M: "Modified",
  A: "Added",
  D: "Deleted",
  R: "Renamed",
  "??": "Untracked",
};

export function ChangesPane({ root }: ChangesPaneProps) {
  const [files, setFiles] = useState<ChangedFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [diff, setDiff] = useState<string>("");
  const [loadingDiff, setLoadingDiff] = useState(false);

  const refresh = () => {
    setError(null);
    gitStatus(root)
      .then(setFiles)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  };

  useEffect(() => {
    setFiles(null);
    setSelected(null);
    setDiff("");
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root]);

  useEffect(() => {
    if (!selected) return;
    setLoadingDiff(true);
    gitDiffFile(root, selected)
      .then(setDiff)
      .catch((err) => setDiff(`Could not load diff: ${err instanceof Error ? err.message : String(err)}`))
      .finally(() => setLoadingDiff(false));
  }, [root, selected]);

  return (
    <div className="flex h-full min-w-0 flex-1">
      <div className="flex w-[240px] flex-shrink-0 flex-col border-r border-border-soft">
        <div className="flex h-9 items-center justify-between px-3">
          <span className="text-[11px] font-semibold uppercase tracking-wide text-text-muted">
            {files ? `${files.length} changed file${files.length === 1 ? "" : "s"}` : "Changes"}
          </span>
          <button onClick={refresh} title="Refresh" className="rounded p-1 text-text-muted transition hover:bg-bg-hover hover:text-text-primary">
            <RefreshCw size={12} />
          </button>
        </div>
        <div className="scrollbar-thin flex-1 overflow-y-auto">
          {error ? (
            <p className="p-3 text-xs text-danger">{error}</p>
          ) : !files ? (
            <p className="p-3 text-xs text-text-muted">Loading…</p>
          ) : files.length === 0 ? (
            <p className="p-3 text-xs text-text-muted">No changes — working tree is clean.</p>
          ) : (
            files.map((f) => (
              <button
                key={f.path}
                onClick={() => setSelected(f.path)}
                className={`flex w-full items-center gap-2 px-3 py-1.5 text-left transition ${
                  selected === f.path ? "bg-bg-hover text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
                }`}
              >
                <FileDiff size={12} className="flex-shrink-0" />
                <span className="min-w-0 flex-1 truncate text-[11.5px]" title={f.path}>
                  {f.path}
                </span>
                <span className="flex-shrink-0 text-[9.5px] font-semibold text-text-muted">
                  {statusLabel[f.status] ?? f.status}
                </span>
                {(f.additions > 0 || f.deletions > 0) && (
                  <span className="flex-shrink-0 text-[10px]">
                    {f.additions > 0 && <span className="text-success">+{f.additions}</span>}{" "}
                    {f.deletions > 0 && <span className="text-danger">-{f.deletions}</span>}
                  </span>
                )}
              </button>
            ))
          )}
        </div>
      </div>

      <div className="scrollbar-thin min-w-0 flex-1 overflow-auto bg-bg-primary">
        {!selected ? (
          <div className="flex h-full items-center justify-center text-xs text-text-muted">Select a file to view its diff.</div>
        ) : loadingDiff ? (
          <div className="flex h-full items-center justify-center text-xs text-text-muted">Loading diff…</div>
        ) : (
          <DiffView text={diff} />
        )}
      </div>
    </div>
  );
}

function DiffView({ text }: { text: string }) {
  if (!text.trim()) {
    return <div className="flex h-full items-center justify-center text-xs text-text-muted">No diff to show.</div>;
  }
  const lines = text.split("\n");
  return (
    <pre className="whitespace-pre-wrap break-all p-3 font-mono text-[11.5px] leading-[1.6]">
      {lines.map((line, i) => {
        let cls = "text-text-secondary";
        if (line.startsWith("+++") || line.startsWith("---")) cls = "text-text-muted font-semibold";
        else if (line.startsWith("+")) cls = "bg-success-soft text-success";
        else if (line.startsWith("-")) cls = "bg-danger-soft text-danger";
        else if (line.startsWith("@@")) cls = "text-accent bg-accent-soft my-1";
        return (
          <div key={i} className={cls}>
            {line || " "}
          </div>
        );
      })}
    </pre>
  );
}
