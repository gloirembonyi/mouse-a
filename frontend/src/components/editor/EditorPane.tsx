import { useCallback, useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Group, Panel, Separator, type PanelImperativeHandle } from "react-resizable-panels";
import { PanelLeftClose, PanelLeftOpen, X } from "lucide-react";
import { IconButton } from "../ui/primitives";
import { FileTree } from "./FileTree";
import { Editor } from "./Editor";
import { FileIcon } from "./fileIcons";
import { subscribeOpenFile } from "../../lib/openFile";

interface EditorPaneProps {
  root: string;
  onAskAboutFile: (path: string, name: string) => void;
}

interface OpenFile {
  path: string;
  name: string;
}

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

// File tree on the left, tabbed editor on the right. Every open tab keeps its
// Editor mounted (hidden when inactive) so unsaved edits, scroll position and
// undo history survive switching tabs.
export function EditorPane({ root, onAskAboutFile }: EditorPaneProps) {
  const [tabs, setTabs] = useState<OpenFile[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [dirty, setDirty] = useState<Record<string, boolean>>({});
  // The file tree can be hidden so the editor gets the whole dock.
  const treeRef = useRef<PanelImperativeHandle>(null);
  const [treeOpen, setTreeOpen] = useState(() => localStorage.getItem("mouse-ai-tree-open") !== "off");
  useEffect(() => {
    localStorage.setItem("mouse-ai-tree-open", treeOpen ? "on" : "off");
    const panel = treeRef.current;
    if (!panel) return;
    if (treeOpen && panel.isCollapsed()) panel.resize("30%");
    if (!treeOpen && !panel.isCollapsed()) panel.collapse();
  }, [treeOpen]);
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // Ctrl+Shift+E (VS Code's Explorer key) toggles the tree while the dock is visible.
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "e" && rootRef.current?.offsetParent) {
        e.preventDefault();
        setTreeOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // A different project means a different set of files.
  useEffect(() => {
    setTabs([]);
    setActive(null);
    setDirty({});
  }, [root]);

  const open = useCallback((path: string) => {
    setTabs((prev) => (prev.some((t) => t.path === path) ? prev : [...prev, { path, name: fileName(path) }]));
    setActive(path);
  }, []);

  const close = useCallback(
    (path: string) => {
      if (dirty[path] && !window.confirm(`${fileName(path)} has unsaved changes. Close it anyway?`)) return;
      setTabs((prev) => {
        const idx = prev.findIndex((t) => t.path === path);
        const next = prev.filter((t) => t.path !== path);
        setActive((cur) => (cur === path ? (next[Math.min(idx, next.length - 1)]?.path ?? null) : cur));
        return next;
      });
      setDirty((d) => {
        const { [path]: _, ...rest } = d;
        return rest;
      });
    },
    [dirty],
  );

  // Files opened from elsewhere (a path in a chat answer, "Open file…").
  useEffect(() => subscribeOpenFile(open), [open]);

  // The tree renamed or deleted something: retarget or drop matching tabs.
  const onPathChanged = useCallback((from: string, to: string | null) => {
    const hit = (p: string) => p === from || p.startsWith(from + "\\") || p.startsWith(from + "/");
    setTabs((prev) =>
      to === null
        ? prev.filter((t) => !hit(t.path))
        : prev.map((t) => (hit(t.path) ? { path: to + t.path.slice(from.length), name: fileName(to + t.path.slice(from.length)) } : t)),
    );
    setActive((cur) => (cur && hit(cur) ? (to === null ? null : to + cur.slice(from.length)) : cur));
  }, []);

  const setFileDirty = useCallback((path: string, isDirty: boolean) => {
    setDirty((d) => (d[path] === isDirty ? d : { ...d, [path]: isDirty }));
  }, []);

  return (
    <div ref={rootRef} className="flex min-w-0 flex-1">
    <Group orientation="horizontal" className="flex-1">
      <Panel
        panelRef={treeRef}
        id="file-tree"
        defaultSize={treeOpen ? "32%" : "0%"}
        minSize="16%"
        maxSize="55%"
        collapsible
        collapsedSize="0%"
        onResize={(size) => {
          const open = size.asPercentage > 0;
          if (open !== treeOpen) setTreeOpen(open);
        }}
        className="min-w-0 border-r border-border-soft"
      >
        <FileTree root={root} activePath={active} onOpenFile={open} onPathChanged={onPathChanged} onHide={() => setTreeOpen(false)} />
      </Panel>
      <Separator className="resize-handle resize-handle-v" />
      <Panel id="editor" minSize="30%" className="flex min-w-0 flex-col bg-bg-primary">
        {(tabs.length > 0 || !treeOpen) && (
          <div className="scrollbar-thin flex h-9 flex-shrink-0 items-stretch overflow-x-auto border-b border-border-soft bg-bg-secondary">
            <div className="flex flex-shrink-0 items-center border-r border-border-soft px-1">
              <IconButton title={treeOpen ? "Hide files (Ctrl+Shift+E)" : "Show files (Ctrl+Shift+E)"} size="sm" onClick={() => setTreeOpen((v) => !v)}>
                {treeOpen ? <PanelLeftClose size={13} /> : <PanelLeftOpen size={13} />}
              </IconButton>
            </div>
            {tabs.map((t) => (
              <div
                key={t.path}
                onMouseDown={(e) => e.button === 1 && (e.preventDefault(), close(t.path))}
                className={clsx(
                  "group flex max-w-[180px] flex-shrink-0 cursor-pointer items-center gap-1.5 border-r border-border-soft pl-3 pr-1.5 text-[12px] transition",
                  t.path === active
                    ? "bg-bg-primary text-text-primary shadow-[inset_0_1px_0_var(--mx-accent)]"
                    : "text-text-muted hover:text-text-secondary",
                )}
                onClick={() => setActive(t.path)}
                title={t.path}
              >
                <FileIcon name={t.name} size={13} />
                <span className="truncate">{t.name}</span>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    close(t.path);
                  }}
                  className="relative flex h-5 w-5 flex-shrink-0 items-center justify-center rounded hover:bg-bg-hover"
                  aria-label={`Close ${t.name}`}
                >
                  {dirty[t.path] ? (
                    <>
                      <span className="h-2 w-2 rounded-full bg-text-secondary group-hover:hidden" />
                      <X size={12} className="hidden group-hover:block" />
                    </>
                  ) : (
                    <X size={12} className={t.path === active ? "" : "opacity-0 group-hover:opacity-100"} />
                  )}
                </button>
              </div>
            ))}
          </div>
        )}

        {tabs.length === 0 ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-2 text-center text-text-muted">
            <p className="text-[13px] text-text-secondary">No file open</p>
            <p className="max-w-[240px] text-[12px]">Pick a file in the tree, press Ctrl+P to find one by name, or right-click the tree to create files.</p>
          </div>
        ) : (
          tabs.map((t) => (
            <div key={t.path} className={clsx("min-h-0 flex-1 flex-col", t.path === active ? "flex" : "hidden")}>
              <Editor
                path={t.path}
                root={root}
                active={t.path === active}
                onDirtyChange={(d) => setFileDirty(t.path, d)}
                onAskAI={() => onAskAboutFile(t.path, t.name)}
              />
            </div>
          ))
        )}
      </Panel>
    </Group>
    </div>
  );
}
