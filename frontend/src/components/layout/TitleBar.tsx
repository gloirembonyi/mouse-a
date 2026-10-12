import clsx from "clsx";
import { Minus, PanelLeftOpen, PanelRight, Square, X } from "lucide-react";
import { appWindow } from "../../lib/tauri";
import { BrandMark, IconButton } from "../ui/primitives";

interface TitleBarProps {
  aiActive: boolean;
  onToggleAi: () => void;
  /** Shown centered, e.g. the open chat or "Meeting copilot". */
  title?: string;
  sidebarOpen?: boolean;
  onOpenSidebar?: () => void;
  dockOpen?: boolean;
  onToggleDock?: () => void;
}

// Frameless window chrome: one slim, draggable row. Everything that isn't a
// button is a drag region, so the window moves from anywhere along the top.
export function TitleBar({ aiActive, onToggleAi, title, sidebarOpen = true, onOpenSidebar, dockOpen, onToggleDock }: TitleBarProps) {
  return (
    <header
      data-tauri-drag-region
      className="relative z-10 flex h-10 flex-shrink-0 select-none items-center justify-between gap-2 border-b border-border-soft bg-bg-secondary pl-3 pr-1.5"
    >
      <div data-tauri-drag-region className="flex min-w-0 items-center gap-2">
        {!sidebarOpen && onOpenSidebar && (
          <IconButton title="Show sidebar" size="sm" onClick={onOpenSidebar}>
            <PanelLeftOpen size={14} />
          </IconButton>
        )}
        <BrandMark size={18} />
        <span data-tauri-drag-region className="text-[13px] font-semibold tracking-[-0.01em]">
          Mouse AI
        </span>
        <button
          onClick={onToggleAi}
          title={aiActive ? "Hover mode is on: pause your mouse on anything to ask. Click to turn off (Ctrl+Shift+Q)" : "Hover mode is off. Click to turn on (Ctrl+Shift+Q)"}
          className={clsx(
            "ml-1 flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[10.5px] font-medium transition",
            aiActive
              ? "border-accent/30 bg-accent-soft text-accent"
              : "border-border-default text-text-muted hover:text-text-secondary",
          )}
        >
          <span className="relative flex h-1.5 w-1.5">
            {aiActive && <span className="absolute inset-0 animate-ping rounded-full bg-accent opacity-60" />}
            <span className={clsx("relative h-1.5 w-1.5 rounded-full", aiActive ? "bg-accent" : "bg-text-muted")} />
          </span>
          {aiActive ? "Hover on" : "Hover off"}
        </button>
      </div>

      {title && (
        <div
          data-tauri-drag-region
          className="pointer-events-none absolute left-1/2 max-w-[40%] -translate-x-1/2 truncate text-[12px] text-text-muted"
        >
          {title}
        </div>
      )}

      <div className="flex items-center gap-0.5">
        {onToggleDock && (
          <IconButton title={dockOpen ? "Hide editor panel" : "Show editor, terminal & changes"} size="sm" active={dockOpen} onClick={onToggleDock}>
            <PanelRight size={14} />
          </IconButton>
        )}
        <span className="mx-1.5 h-4 w-px bg-border-default" />
        <WindowButton onClick={() => appWindow.minimize().catch(() => {})} title="Minimize">
          <Minus size={14} />
        </WindowButton>
        <WindowButton onClick={() => appWindow.toggleMaximize().catch(() => {})} title="Maximize / Restore">
          <Square size={11} />
        </WindowButton>
        <WindowButton
          onClick={() => appWindow.close().catch(() => {})}
          title="Close (minimizes to tray — reopen from the tray icon or a shortcut)"
          danger
        >
          <X size={14} />
        </WindowButton>
      </div>
    </header>
  );
}

function WindowButton({
  children,
  onClick,
  title,
  danger,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title: string;
  danger?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      aria-label={title}
      className={clsx(
        "flex h-7 w-8 items-center justify-center rounded-md text-text-muted transition",
        danger ? "hover:bg-danger hover:text-white" : "hover:bg-bg-hover hover:text-text-primary",
      )}
    >
      {children}
    </button>
  );
}
