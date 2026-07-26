import { useState } from "react";
import { Code2, FolderOpen, GitCompare, PanelRightClose, TerminalSquare } from "lucide-react";
import { EditorPane } from "../editor/EditorPane";
import { TerminalPane } from "../terminal/TerminalPane";
import { ChangesPane } from "../diff/ChangesPane";

type DockTab = "editor" | "terminal" | "changes";

interface RightDockProps {
  root: string | null;
  onChangeRoot: (path: string) => void;
  onClose: () => void;
}

// All three panes are mounted at all times (just hidden via CSS when not the
// active tab) instead of conditionally rendered — switching tabs shouldn't
// kill the terminal session or lose the editor's open file.
export function RightDock({ root, onChangeRoot, onClose }: RightDockProps) {
  const [tab, setTab] = useState<DockTab>("editor");
  const [rootInput, setRootInput] = useState(root ?? "");

  return (
    <div className="flex h-full flex-col bg-bg-secondary">
      <div className="flex flex-shrink-0 items-center justify-between border-b border-border-default px-2 py-1.5">
        <div className="flex gap-1">
          <DockTabButton icon={<Code2 size={13} />} label="Editor" active={tab === "editor"} onClick={() => setTab("editor")} />
          <DockTabButton icon={<TerminalSquare size={13} />} label="Terminal" active={tab === "terminal"} onClick={() => setTab("terminal")} />
          <DockTabButton icon={<GitCompare size={13} />} label="Changes" active={tab === "changes"} onClick={() => setTab("changes")} />
        </div>
        <button onClick={onClose} title="Close panel" className="rounded p-1 text-text-muted transition hover:bg-bg-hover hover:text-text-primary">
          <PanelRightClose size={14} />
        </button>
      </div>

      <div className="flex flex-shrink-0 items-center gap-1.5 border-b border-border-default px-2 py-1.5">
        <FolderOpen size={13} className="flex-shrink-0 text-text-muted" />
        <input
          value={rootInput}
          onChange={(e) => setRootInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && rootInput.trim()) onChangeRoot(rootInput.trim());
          }}
          onBlur={() => {
            if (rootInput.trim() && rootInput.trim() !== root) onChangeRoot(rootInput.trim());
          }}
          placeholder="Folder path…"
          className="min-w-0 flex-1 rounded-md border border-border-default bg-bg-tertiary px-2 py-1 font-mono text-[11px] text-text-primary outline-none focus:border-border-focus"
        />
      </div>

      {!root ? (
        <div className="flex flex-1 items-center justify-center text-xs text-text-muted">Loading workspace…</div>
      ) : (
        <>
          <div className={`flex min-h-0 flex-1 ${tab === "editor" ? "" : "hidden"}`}>
            <EditorPane root={root} />
          </div>
          <div className={`min-h-0 flex-1 ${tab === "terminal" ? "flex" : "hidden"}`}>
            <TerminalPane cwd={root} />
          </div>
          <div className={`flex min-h-0 flex-1 ${tab === "changes" ? "" : "hidden"}`}>
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
      className={`flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-[11.5px] font-medium transition ${
        active ? "bg-accent-soft text-accent" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}
