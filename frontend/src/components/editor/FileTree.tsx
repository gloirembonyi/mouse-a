import { useEffect, useState } from "react";
import { ChevronRight, File, Folder, FolderOpen } from "lucide-react";
import { fsListDir, type FsEntry } from "../../lib/tauri";

interface FileTreeProps {
  root: string;
  activePath: string | null;
  onOpenFile: (path: string) => void;
}

export function FileTree({ root, activePath, onOpenFile }: FileTreeProps) {
  const [entries, setEntries] = useState<FsEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setEntries(null);
    setError(null);
    fsListDir(root)
      .then(setEntries)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  }, [root]);

  if (error) return <p className="p-3 text-xs text-danger">{error}</p>;
  if (!entries) return <p className="p-3 text-xs text-text-muted">Loading…</p>;
  if (entries.length === 0) return <p className="p-3 text-xs text-text-muted">Empty folder.</p>;

  return (
    <div className="scrollbar-thin flex h-full flex-col overflow-y-auto py-1">
      {entries.map((entry) => (
        <TreeNode key={entry.path} entry={entry} depth={0} activePath={activePath} onOpenFile={onOpenFile} />
      ))}
    </div>
  );
}

function TreeNode({
  entry,
  depth,
  activePath,
  onOpenFile,
}: {
  entry: FsEntry;
  depth: number;
  activePath: string | null;
  onOpenFile: (path: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [children, setChildren] = useState<FsEntry[] | null>(null);

  const toggle = async () => {
    if (!entry.is_dir) {
      onOpenFile(entry.path);
      return;
    }
    setExpanded(!expanded);
    if (!expanded && children === null) {
      try {
        setChildren(await fsListDir(entry.path));
      } catch {
        setChildren([]);
      }
    }
  };

  const isActive = activePath === entry.path;

  return (
    <div>
      <button
        onClick={toggle}
        style={{ paddingLeft: 10 + depth * 14 }}
        className={`flex w-full items-center gap-1 py-1 pr-2 text-left text-[12px] transition ${
          isActive ? "bg-accent-soft text-accent" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
        }`}
      >
        {entry.is_dir ? (
          <>
            <ChevronRight size={12} className={`flex-shrink-0 text-text-muted transition-transform ${expanded ? "rotate-90" : ""}`} />
            {expanded ? <FolderOpen size={13} className="flex-shrink-0 text-accent" /> : <Folder size={13} className="flex-shrink-0" />}
          </>
        ) : (
          <>
            <span className="w-3 flex-shrink-0" />
            <File size={13} className="flex-shrink-0" />
          </>
        )}
        <span className="truncate">{entry.name}</span>
      </button>
      {entry.is_dir && expanded && children && (
        <div>
          {children.length === 0 ? (
            <p className="py-1 pl-8 text-[11px] text-text-muted">Empty</p>
          ) : (
            children.map((child) => (
              <TreeNode key={child.path} entry={child} depth={depth + 1} activePath={activePath} onOpenFile={onOpenFile} />
            ))
          )}
        </div>
      )}
    </div>
  );
}
