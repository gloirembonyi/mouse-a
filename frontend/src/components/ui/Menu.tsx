import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Check } from "lucide-react";

export interface MenuItem {
  key: string;
  label: React.ReactNode;
  description?: string;
  icon?: React.ReactNode;
  checked?: boolean;
  onSelect: () => void;
  divider?: boolean;
}

// Minimal popover menu (click to open, click-outside/Escape to close) for the
// composer's mode and project pickers. `placement` decides whether it opens
// above (composer docked at the bottom) or below (hero composer) its trigger.
export function Menu({
  trigger,
  items,
  placement = "top",
  align = "start",
  width = 240,
  header,
}: {
  trigger: (open: boolean) => React.ReactNode;
  items: MenuItem[];
  placement?: "top" | "bottom";
  align?: "start" | "end";
  width?: number;
  header?: string;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative">
      <div onClick={() => setOpen((v) => !v)}>{trigger(open)}</div>
      {open && (
        <div
          role="menu"
          style={{ width }}
          className={clsx(
            "animate-pop absolute z-50 rounded-xl border border-border-default bg-bg-elevated p-1 shadow-pop",
            placement === "top" ? "bottom-full mb-1.5" : "top-full mt-1.5",
            align === "start" ? "left-0" : "right-0",
          )}
        >
          {header && <div className="px-2.5 pb-1 pt-1.5 text-[11px] font-medium text-text-muted">{header}</div>}
          {items.map((item) => (
            <div key={item.key}>
              {item.divider && <div className="my-1 h-px bg-border-soft" />}
              <button
                role="menuitem"
                onClick={() => {
                  setOpen(false);
                  item.onSelect();
                }}
                className="flex w-full items-start gap-2.5 rounded-lg px-2.5 py-1.5 text-left transition hover:bg-bg-hover"
              >
                {item.icon && <span className="mt-[2px] flex-shrink-0 text-text-muted">{item.icon}</span>}
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] text-text-primary">{item.label}</span>
                  {item.description && <span className="block text-[11.5px] leading-snug text-text-muted">{item.description}</span>}
                </span>
                {item.checked && <Check size={14} className="mt-[2px] flex-shrink-0 text-text-primary" />}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
