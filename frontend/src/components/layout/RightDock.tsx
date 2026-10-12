import { useState } from "react";
import clsx from "clsx";
import { Code2, FolderOpen, GitCompare, PanelRightClose, TerminalSquare } from "lucide-react";
import { EditorPane } from "../editor/EditorPane";
import { TerminalPane } from "../terminal/TerminalPane";
import { ChangesPane } from "../diff/ChangesPane";
import { IconButton } from "../ui/primitives";

type DockTab = "editor" | "terminal" | "changes";

interface RightDockProps {
  root: string | null;
  projectName: string | null;
  onAddProject: () => void;
  onClose: () => void;
  onAskAboutFile: (path: string, name: string) => void;
}

// All three panes are mounted at all times (just hidden via CSS when not the
// active tab) instead of conditionally rendered — switching tabs shouldn't
// kill the terminal session or lose the editor's open files.
export function RightDock({ root, projectName, onAddProject, onClose, onAskAboutFile }: RightDockProps) {
  const [tab, setTab] = useState<DockTab>("editor");

  return (
    <div className="flex h-full flex-col bg-bg-secondary">
      <div className="flex h-10 flex-shrink-0 items-center gap-1 border-b border-border-soft px-2">
        <div className="flex rounded-lg bg-bg-tertiary p-0.5">
          <DockTabButton icon={<Code2 size={13} />} label="Files" active={tab === "editor"} onClick={() => setTab("editor")} />
          <DockTabButton icon={<TerminalSquare size={13} />} label="Terminal" active={tab === "terminal"} onClick={() => setTab("terminal")} />
          <DockTabButton icon={<GitCompare size={13} />} label="Changes" active={tab === "changes"} onClick={() => setTab("changes")} />
        </div>
        <button
          onClick={onAddProject}
          title={root ? `${root}\nClick to open another folder` : "Open a folder"}
          className="ml-auto flex min-w-0 items-center gap-1.5 rounded-md px-2 py-1 text-[12px] text-text-secondary transition hover:bg-bg-hover hover:text-text-primary"
        >
          <FolderOpen size={13} className="flex-shrink-0" />
          <span className="truncate">{projectName ?? "Home folder"}</span>
        </button>
        <IconButton title="Close panel" size="sm" onClick={onClose}>
          <PanelRightClose size={14} />
        </IconButton>
      </div>

      {!root ? (
        <div className="flex flex-1 items-center justify-center text-xs text-text-muted">Loading workspace…</div>
      ) : (
        <>
          <div className={clsx("min-h-0 flex-1", tab === "editor" ? "flex" : "hidden")}>
            <EditorPane root={root} onAskAboutFile={onAskAboutFile} />
          </div>
          <div className={clsx("min-h-0 flex-1", tab === "terminal" ? "flex" : "hidden")}>
            <TerminalPane cwd={root} visible={tab === "terminal"} />
          </div>
          <div className={clsx("min-h-0 flex-1", tab === "changes" ? "flex" : "hidden")}>
            <ChangesPane root={root} />
          </div>
        </>
      )}
    </div>
  );
}

function DockTabButton({
  icon,
  label,
  active,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={clsx(
        "flex items-center gap-1.5 rounded-md px-2.5 py-1 text-[12px] font-medium transition",
        active ? "bg-bg-primary text-text-primary shadow-sm" : "text-text-muted hover:text-text-primary",
      )}
    >
      {icon}
      {label}
    </button>
  );
}
