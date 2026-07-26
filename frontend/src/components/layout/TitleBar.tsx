import { Minus, Square, X } from "lucide-react";
import { appWindow } from "../../lib/tauri";

interface TitleBarProps {
  aiActive: boolean;
  onToggleAi: () => void;
}

export function TitleBar({ aiActive, onToggleAi }: TitleBarProps) {
  return (
    <header
      data-tauri-drag-region
      className="relative z-10 flex flex-shrink-0 cursor-grab select-none items-center justify-between border-b border-border-default bg-gradient-to-b from-white/[0.03] to-transparent px-3.5 py-2.5 shadow-[0_1px_0_rgba(255,255,255,0.02),0_6px_16px_rgba(0,0,0,0.18)]"
    >
      <div className="flex items-center gap-2">
        <button
          onClick={onToggleAi}
          title={aiActive ? "AI Active — Click to disable" : "AI Inactive — Click to enable"}
          className="relative flex h-6 w-6 items-center justify-center"
        >
          <span
            className={`absolute inset-0 rounded-full border-2 ${
              aiActive ? "animate-[pulse-ring_2s_ease-in-out_infinite] border-accent" : "border-transparent"
            }`}
          />
          <span
            className={`h-2 w-2 rounded-full transition ${
              aiActive ? "bg-accent shadow-[0_0_12px_var(--color-accent-glow),0_0_4px_var(--color-accent)]" : "bg-text-muted"
            }`}
          />
        </button>
        <h1 className="bg-gradient-to-br from-accent to-accent-light bg-clip-text text-sm font-semibold text-transparent">
          Mouse AI
        </h1>
        <span
          className={`rounded px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wide ${
            aiActive ? "animate-[badge-pulse_2s_ease-in-out_infinite] bg-accent-soft text-accent" : "bg-bg-tertiary text-text-muted"
          }`}
        >
          {aiActive ? "Active" : "Off"}
        </span>
      </div>

      <div className="flex items-center gap-1">
        <span className="mx-1 h-4 w-px bg-border-default" />
        <TitleBarButton onClick={() => appWindow.minimize().catch(() => {})} title="Minimize">
          <Minus size={14} />
        </TitleBarButton>
        <TitleBarButton onClick={() => appWindow.toggleMaximize().catch(() => {})} title="Maximize / Restore">
          <Square size={12} />
        </TitleBarButton>
        <TitleBarButton
          onClick={() => appWindow.close().catch(() => {})}
          title="Close (minimizes to tray — reopen from the tray icon or a shortcut)"
          danger
        >
          <X size={14} />
        </TitleBarButton>
      </div>
    </header>
  );
}

function TitleBarButton({
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
      className={`flex items-center justify-center rounded-md p-1.5 text-text-secondary transition ${
        danger ? "hover:bg-danger-soft hover:text-danger" : "hover:bg-bg-hover hover:text-text-primary"
      }`}
    >
      {children}
    </button>
  );
}
