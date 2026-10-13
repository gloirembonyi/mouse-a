import clsx from "clsx";
import { BotAvatar, type BotMood } from "./BotAvatar";

// Small shared building blocks so every view uses the same button sizes,
// key hints and switches instead of re-styling them inline.

/** The Thothai logo - the ghost mascot (same art as the app icon,
 *  src-tauri/icons/source/thothai-ghost.svg), still unless told otherwise. */
export function BrandMark({ size = 22, className, mood = "still" }: { size?: number; className?: string; mood?: BotMood }) {
  return <BotAvatar kind="ghost" mood={mood} size={size} className={className} title="Thothai" />;
}

export function Kbd({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <kbd
      className={clsx(
        "inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-[5px] border border-border-default bg-bg-tertiary px-1 font-sans text-[10px] font-medium text-text-muted",
        className,
      )}
    >
      {children}
    </kbd>
  );
}

export function IconButton({
  children,
  title,
  onClick,
  active,
  size = "md",
  className,
  disabled,
}: {
  children: React.ReactNode;
  title: string;
  onClick?: (e: React.MouseEvent) => void;
  active?: boolean;
  size?: "sm" | "md";
  className?: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      aria-label={title}
      disabled={disabled}
      className={clsx(
        "inline-flex flex-shrink-0 items-center justify-center rounded-md transition disabled:cursor-not-allowed disabled:opacity-40",
        size === "sm" ? "h-6 w-6" : "h-7 w-7",
        active ? "bg-bg-hover text-text-primary" : "text-text-muted hover:bg-bg-hover hover:text-text-primary",
        className,
      )}
    >
      {children}
    </button>
  );
}

export function Switch({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      title={label}
      onClick={(e) => {
        e.stopPropagation();
        onChange(!on);
      }}
      className={clsx(
        "relative inline-flex h-[16px] w-[28px] flex-shrink-0 items-center rounded-full transition-colors",
        on ? "bg-accent" : "bg-border-strong",
      )}
    >
      <span
        className={clsx(
          "absolute h-3 w-3 rounded-full bg-white shadow transition-transform",
          on ? "translate-x-[14px]" : "translate-x-[2px]",
        )}
      />
    </button>
  );
}

export function SectionLabel({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="group/section flex h-7 flex-shrink-0 items-center justify-between px-2.5 text-[11px] font-medium text-text-muted">
      <span>{children}</span>
      {action}
    </div>
  );
}
