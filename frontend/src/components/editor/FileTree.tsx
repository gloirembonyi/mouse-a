import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { ChevronRight, Folder, FolderOpen, RefreshCw, Search, X } from "lucide-react";
import { fsListDir, fsSearchFiles, type FsEntry } from "../../lib/tauri";
import { FileIcon } from "./fileIcons";
import { IconButton } from "../ui/primitives";

interface FileTreeProps {
  root: string;
  activePath: string | null;
  onOpenFile: (path: string) => void;
}

const baseName = (p: string) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;
const relative = (root: string, p: string) => (p.startsWith(root) ? p.slice(root.length).replace(/^[\\/]/, "") : p);

export function FileTree({ root, activePath, onOpenFile }: FileTreeProps) {
  const [entries, setEntries] = useState<FsEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<FsEntry[] | null>(null);
  const [cursor, setCursor] = useState(0);
  const searchRef = useRef<HTMLInputElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setEntries(null);
    setError(null);
    fsListDir(root)
      .then(setEntries)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  }, [root, reloadKey]);

  // Debounced name search across the whole project (bounded server-side).
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults(null);
      return;
    }
    const t = setTimeout(() => {
      fsSearchFiles(root, q, 50)
        .then((r) => {
          setResults(r.filter((e) => !e.is_dir));
          setCursor(0);
        })
        .catch(() => setResults([]));
    }, 120);
    return () => clearTimeout(t);
  }, [query, root]);

  // Ctrl+P while the dock has focus = quick open.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p" && containerRef.current?.offsetParent) {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const openResult = (e: FsEntry) => {
    onOpenFile(e.path);
    setQuery("");
  };

  return (
    <div ref={containerRef} className="flex h-full flex-col">
      <div className="flex h-9 flex-shrink-0 items-center justify-between gap-1 pl-3 pr-1.5">
        <span className="truncate text-[11px] font-semibold uppercase tracking-wide text-text-muted" title={root}>
          {baseName(root)}
        </span>
        <IconButton title="Refresh" size="sm" onClick={() => setReloadKey((k) => k + 1)}>
          <RefreshCw size={12} />
        </IconButton>
      </div>

      <div className="px-2 pb-1.5">
        <div className="flex h-7 items-center gap-1.5 rounded-md border border-border-soft bg-bg-primary px-2 focus-within:border-border-focus">
          <Search size={12} className="flex-shrink-0 text-text-muted" />
          <input
            ref={searchRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (!results?.length) {
                if (e.key === "Escape") setQuery("");
                return;
              }
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setCursor((c) => Math.min(c + 1, results.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setCursor((c) => Math.max(c - 1, 0));
              } else if (e.key === "Enter") {
                e.preventDefault();
                openResult(results[cursor]);
              } else if (e.key === "Escape") {
                setQuery("");
              }
            }}
            placeholder="Go to file  (Ctrl+P)"
            className="min-w-0 flex-1 bg-transparent text-[12px] outline-none placeholder:text-text-muted"
          />
          {query && (
            <button onClick={() => setQuery("")} className="text-text-muted hover:text-text-primary" aria-label="Clear">
              <X size={12} />
            </button>
          )}
        </div>
      </div>

      <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto pb-2">
        {results ? (
          results.length === 0 ? (
            <p className="px-3 py-2 text-[12px] text-text-muted">No files match.</p>
          ) : (
            results.map((r, i) => (
              <button
                key={r.path}
                onClick={() => openResult(r)}
                onMouseEnter={() => setCursor(i)}
                className={clsx(
                  "flex w-full flex-col px-3 py-1 text-left transition",
                  i === cursor ? "bg-bg-hover" : "hover:bg-bg-hover",
                )}
              >
                <span className="flex items-center gap-1.5 text-[12px] text-text-primary">
                  <FileIcon name={r.name} size={13} />
                  <span className="truncate">{r.name}</span>
                </span>
                <span className="truncate pl-[19px] text-[10.5px] text-text-muted">{relative(root, r.path)}</span>
              </button>
            ))
          )
        ) : error ? (
          <p className="p-3 text-xs text-danger">{error}</p>
        ) : !entries ? (
          <TreeSkeleton />
        ) : entries.length === 0 ? (
          <p className="p-3 text-xs text-text-muted">Empty folder.</p>
        ) : (
          entries.map((entry) => (
            <TreeNode key={`${reloadKey}:${entry.path}`} entry={entry} depth={0} activePath={activePath} onOpenFile={onOpenFile} />
          ))
        )}
      </div>
    </div>
  );
}

function TreeSkeleton() {
  return (
    <div className="flex flex-col gap-2 px-3 py-2">
      {[70, 55, 80, 45, 65, 50].map((w, i) => (
        <div key={i} className="h-3 animate-pulse rounded bg-bg-hover" style={{ width: `${w}%` }} />
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
        style={{ paddingLeft: 8 + depth * 12 }}
        className={clsx(
          "flex h-[26px] w-full items-center gap-1.5 pr-2 text-left text-[12.5px] transition",
          isActive ? "bg-accent-soft text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary",
        )}
      >
        {entry.is_dir ? (
          <>
            <ChevronRight size={12} className={clsx("flex-shrink-0 text-text-muted transition-transform", expanded && "rotate-90")} />
            {expanded ? (
              <FolderOpen size={14} className="flex-shrink-0 text-accent" />
            ) : (
              <Folder size={14} className="flex-shrink-0 text-text-muted" />
            )}
          </>
        ) : (
          <>
            <span className="w-3 flex-shrink-0" />
            <FileIcon name={entry.name} />
          </>
        )}
        <span className="truncate">{entry.name}</span>
      </button>
      {entry.is_dir && expanded && children && (
        <div className="relative">
          <span className="absolute bottom-0 top-0 w-px bg-border-soft" style={{ left: 14 + depth * 12 }} />
          {children.length === 0 ? (
            <p className="py-1 text-[11px] text-text-muted" style={{ paddingLeft: 28 + depth * 12 }}>
              Empty
            </p>
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
