import { useCallback, useEffect, useRef, useState } from "react";
import clsx from "clsx";
import {
  ChevronRight,
  Copy,
  ExternalLink,
  FilePlus2,
  FileUp,
  Folder,
  FolderOpen,
  FolderPlus,
  FolderSearch,
  PanelLeftClose,
  Pencil,
  RefreshCw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  fsCreateDir,
  fsCreateFile,
  fsDelete,
  fsListDir,
  fsOpenDefault,
  fsRename,
  fsReveal,
  fsSearchFiles,
  type FsEntry,
} from "../../lib/tauri";
import { FileIcon } from "./fileIcons";
import { IconButton } from "../ui/primitives";

interface FileTreeProps {
  root: string;
  activePath: string | null;
  onOpenFile: (path: string) => void;
  /** A file was renamed or deleted, so open tabs can follow. */
  onPathChanged: (from: string, to: string | null) => void;
  onHide?: () => void;
}

const baseName = (p: string) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;
const parentOf = (p: string) => p.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]*$/, "");
const join = (dir: string, name: string) => `${dir.replace(/[\\/]+$/, "")}\\${name}`;
const relative = (root: string, p: string) =>
  p.toLowerCase().startsWith(root.toLowerCase()) ? p.slice(root.length).replace(/^[\\/]/, "") : p;

type Pending = { kind: "file" | "folder" | "rename"; dir: string; entry?: FsEntry };
type MenuState = { x: number; y: number; entry: FsEntry | null };

export function FileTree({ root, activePath, onOpenFile, onPathChanged, onHide }: FileTreeProps) {
  const [children, setChildren] = useState<Record<string, FsEntry[] | undefined>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<FsEntry[] | null>(null);
  const [cursor, setCursor] = useState(0);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;

  const load = useCallback(async (dir: string) => {
    try {
      const list = await fsListDir(dir);
      setChildren((c) => ({ ...c, [dir]: list }));
      return true;
    } catch (err) {
      if (dir === root) setError(err instanceof Error ? err.message : String(err));
      setChildren((c) => ({ ...c, [dir]: [] }));
      return false;
    }
  }, [root]);

  /** Re-list the root and every open folder, keeping what's expanded. */
  const refresh = useCallback(async () => {
    setError(null);
    await Promise.all([root, ...expandedRef.current].map(load));
  }, [root, load]);

  useEffect(() => {
    setChildren({});
    setExpanded(new Set());
    setError(null);
    void load(root);
  }, [root, load]);

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

  // Ctrl+P while the dock is visible = quick open.
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

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", close);
    };
  }, [menu]);

  const toggle = (entry: FsEntry) => {
    if (!entry.is_dir) {
      onOpenFile(entry.path);
      return;
    }
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(entry.path)) next.delete(entry.path);
      else {
        next.add(entry.path);
        if (!children[entry.path]) void load(entry.path);
      }
      return next;
    });
  };

  const openResult = (e: FsEntry) => {
    onOpenFile(e.path);
    setQuery("");
  };

  const openAnyFile = async () => {
    const picked = await openDialog({ multiple: false, directory: false, title: "Open a file" });
    if (typeof picked === "string") onOpenFile(picked);
  };

  const submitPending = async (name: string) => {
    const p = pending;
    setPending(null);
    const clean = name.trim();
    if (!p || !clean || /[<>:"/\\|?*]/.test(clean)) return;
    try {
      if (p.kind === "rename" && p.entry) {
        if (clean === p.entry.name) return;
        const to = join(parentOf(p.entry.path), clean);
        await fsRename(p.entry.path, to);
        onPathChanged(p.entry.path, to);
      } else {
        const target = join(p.dir, clean);
        if (p.kind === "file") {
          await fsCreateFile(target);
          onOpenFile(target);
        } else {
          await fsCreateDir(target);
        }
        if (p.dir !== root) setExpanded((prev) => new Set(prev).add(p.dir));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
    await refresh();
  };

  const remove = async (entry: FsEntry) => {
    if (!window.confirm(`Move "${entry.name}" to the Recycle Bin?`)) return;
    try {
      await fsDelete(entry.path);
      onPathChanged(entry.path, null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
    await refresh();
  };

  const startCreate = (kind: "file" | "folder", entry: FsEntry | null) => {
    const dir = entry ? (entry.is_dir ? entry.path : parentOf(entry.path)) : root;
    if (dir !== root) setExpanded((prev) => new Set(prev).add(dir));
    if (dir !== root && !children[dir]) void load(dir);
    setPending({ kind, dir });
  };

  const renderList = (dir: string, depth: number): React.ReactNode => {
    const list = children[dir];
    const creating = pending && pending.kind !== "rename" && pending.dir === dir;
    if (!list) {
      return depth === 0 ? <TreeSkeleton /> : null;
    }
    return (
      <>
        {creating && (
          <NameInput
            depth={depth}
            icon={pending!.kind === "folder" ? <Folder size={14} className="text-text-muted" /> : <FilePlus2 size={14} className="text-text-muted" />}
            placeholder={pending!.kind === "folder" ? "Folder name" : "File name, e.g. notes.md"}
            onDone={submitPending}
          />
        )}
        {list.length === 0 && !creating ? (
          <p className="py-1 text-[11px] text-text-muted" style={{ paddingLeft: 28 + depth * 12 }}>
            {depth === 0 ? "Empty folder." : "Empty"}
          </p>
        ) : (
          list.map((entry) => {
            const isOpen = expanded.has(entry.path);
            if (pending?.kind === "rename" && pending.entry?.path === entry.path) {
              return (
                <NameInput
                  key={entry.path}
                  depth={depth}
                  icon={entry.is_dir ? <Folder size={14} className="text-text-muted" /> : <FileIcon name={entry.name} />}
                  initial={entry.name}
                  placeholder="New name"
                  onDone={submitPending}
                />
              );
            }
            return (
              <div key={entry.path}>
                <button
                  onClick={() => toggle(entry)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({ x: e.clientX, y: e.clientY, entry });
                  }}
                  style={{ paddingLeft: 8 + depth * 12 }}
                  title={entry.path}
                  className={clsx(
                    "flex h-[26px] w-full items-center gap-1.5 pr-2 text-left text-[12.5px] transition",
                    activePath === entry.path
                      ? "bg-accent-soft text-text-primary"
                      : "text-text-secondary hover:bg-bg-hover hover:text-text-primary",
                  )}
                >
                  {entry.is_dir ? (
                    <>
                      <ChevronRight size={12} className={clsx("flex-shrink-0 text-text-muted transition-transform", isOpen && "rotate-90")} />
                      {isOpen ? (
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
                {entry.is_dir && isOpen && (
                  <div className="relative">
                    <span className="absolute bottom-0 top-0 w-px bg-border-soft" style={{ left: 14 + depth * 12 }} />
                    {renderList(entry.path, depth + 1)}
                  </div>
                )}
              </div>
            );
          })
        )}
      </>
    );
  };

  const menuEntry = menu?.entry ?? null;

  return (
    <div ref={containerRef} className="flex h-full flex-col">
      <div className="flex h-9 flex-shrink-0 items-center justify-between gap-1 pl-3 pr-1.5">
        <span className="truncate text-[11px] font-semibold uppercase tracking-wide text-text-muted" title={root}>
          {baseName(root)}
        </span>
        <div className="flex items-center">
          <IconButton title="New file" size="sm" onClick={() => startCreate("file", null)}>
            <FilePlus2 size={13} />
          </IconButton>
          <IconButton title="New folder" size="sm" onClick={() => startCreate("folder", null)}>
            <FolderPlus size={13} />
          </IconButton>
          <IconButton title="Open any file…" size="sm" onClick={() => void openAnyFile()}>
            <FileUp size={13} />
          </IconButton>
          <IconButton title="Refresh" size="sm" onClick={() => void refresh()}>
            <RefreshCw size={12} />
          </IconButton>
          {onHide && (
            <IconButton title="Hide files (Ctrl+Shift+E)" size="sm" onClick={onHide}>
              <PanelLeftClose size={13} />
            </IconButton>
          )}
        </div>
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

      {error && (
        <div className="mx-2 mb-1.5 flex items-start gap-2 rounded-md bg-danger-soft px-2 py-1.5 text-[11px] text-danger">
          <span className="min-w-0 flex-1">{error}</span>
          <button onClick={() => setError(null)} aria-label="Dismiss">
            <X size={11} />
          </button>
        </div>
      )}

      <div
        className="scrollbar-thin min-h-0 flex-1 overflow-y-auto pb-2"
        onContextMenu={(e) => {
          if (e.target === e.currentTarget) {
            e.preventDefault();
            setMenu({ x: e.clientX, y: e.clientY, entry: null });
          }
        }}
      >
        {results ? (
          results.length === 0 ? (
            <p className="px-3 py-2 text-[12px] text-text-muted">No files match.</p>
          ) : (
            results.map((r, i) => (
              <button
                key={r.path}
                onClick={() => openResult(r)}
                onMouseEnter={() => setCursor(i)}
                className={clsx("flex w-full flex-col px-3 py-1 text-left transition", i === cursor ? "bg-bg-hover" : "hover:bg-bg-hover")}
              >
                <span className="flex items-center gap-1.5 text-[12px] text-text-primary">
                  <FileIcon name={r.name} size={13} />
                  <span className="truncate">{r.name}</span>
                </span>
                <span className="truncate pl-[19px] text-[10.5px] text-text-muted">{relative(root, r.path)}</span>
              </button>
            ))
          )
        ) : (
          renderList(root, 0)
        )}
      </div>

      {menu && (
        <div
          onMouseDown={(e) => e.stopPropagation()}
          style={{ left: Math.min(menu.x, window.innerWidth - 210), top: Math.min(menu.y, window.innerHeight - 290) }}
          className="animate-pop fixed z-50 w-[200px] rounded-xl border border-border-default bg-bg-elevated p-1 shadow-pop"
        >
          <MenuItem icon={<FilePlus2 size={13} />} label="New file" onClick={() => (setMenu(null), startCreate("file", menuEntry))} />
          <MenuItem icon={<FolderPlus size={13} />} label="New folder" onClick={() => (setMenu(null), startCreate("folder", menuEntry))} />
          {menuEntry && (
            <>
              <div className="my-1 h-px bg-border-soft" />
              <MenuItem
                icon={<Pencil size={13} />}
                label="Rename"
                onClick={() => (setMenu(null), setPending({ kind: "rename", dir: parentOf(menuEntry.path), entry: menuEntry }))}
              />
              <MenuItem
                icon={<Copy size={13} />}
                label="Copy path"
                onClick={() => (setMenu(null), void navigator.clipboard.writeText(menuEntry.path).catch(() => {}))}
              />
              <MenuItem icon={<FolderSearch size={13} />} label="Show in Explorer" onClick={() => (setMenu(null), void fsReveal(menuEntry.path).catch(() => {}))} />
              {!menuEntry.is_dir && (
                <MenuItem
                  icon={<ExternalLink size={13} />}
                  label="Open with default app"
                  onClick={() => (setMenu(null), void fsOpenDefault(menuEntry.path).catch(() => {}))}
                />
              )}
              <div className="my-1 h-px bg-border-soft" />
              <MenuItem icon={<Trash2 size={13} />} label="Delete" danger onClick={() => (setMenu(null), void remove(menuEntry))} />
            </>
          )}
        </div>
      )}
    </div>
  );
}

function MenuItem({ icon, label, onClick, danger }: { icon: React.ReactNode; label: string; onClick: () => void; danger?: boolean }) {
  return (
    <button
      onClick={onClick}
      className={clsx(
        "flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-[12.5px] transition",
        danger ? "text-danger hover:bg-danger-soft" : "text-text-primary hover:bg-bg-hover",
      )}
    >
      <span className={danger ? "" : "text-text-muted"}>{icon}</span>
      {label}
    </button>
  );
}

function NameInput({
  depth,
  icon,
  initial = "",
  placeholder,
  onDone,
}: {
  depth: number;
  icon: React.ReactNode;
  initial?: string;
  placeholder: string;
  onDone: (name: string) => void;
}) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    // Select the name without its extension, like Explorer/VS Code.
    const dot = initial.lastIndexOf(".");
    el.setSelectionRange(0, dot > 0 ? dot : initial.length);
  }, [initial]);
  const finish = (v: string) => {
    if (done.current) return;
    done.current = true;
    onDone(v);
  };
  return (
    <div className="flex h-[26px] items-center gap-1.5 pr-2" style={{ paddingLeft: 20 + depth * 12 }}>
      {icon}
      <input
        ref={ref}
        value={value}
        placeholder={placeholder}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") finish(value);
          if (e.key === "Escape") finish("");
        }}
        onBlur={() => finish(value)}
        className="min-w-0 flex-1 rounded border border-border-focus bg-bg-primary px-1.5 py-0.5 text-[12px] outline-none"
      />
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
