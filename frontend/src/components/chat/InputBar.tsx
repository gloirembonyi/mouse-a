import { useRef, useState } from "react";
import { ArrowUp, Bot, Eye, MousePointerClick } from "lucide-react";
import { askAtCursor } from "../../lib/tauri";

interface InputBarProps {
  agentMode: boolean;
  onToggleAgentMode: () => void;
  onSend: (text: string) => void;
  disabled: boolean;
  onToggleHoverMode: () => void;
}

// Cursor/Codex-style composer: textarea on top, a single toolbar row below it
// inside the SAME bordered container (mode toggle on the left, send button on
// the right) instead of a separate pill row floating above the input.
export function InputBar({ agentMode, onToggleAgentMode, onSend, disabled, onToggleHoverMode }: InputBarProps) {
  const [value, setValue] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const resize = () => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  };

  const submit = () => {
    const text = value.trim();
    if (!text || disabled) return;
    setValue("");
    requestAnimationFrame(resize);
    onSend(text);
  };

  return (
    <div className="flex-shrink-0 border-t border-border-default bg-bg-secondary px-6 pb-4 pt-3.5">
      <div className="mx-auto max-w-[760px] rounded-2xl border border-border-default bg-bg-tertiary transition focus-within:border-border-focus">
        <textarea
          ref={textareaRef}
          rows={1}
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            resize();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder={agentMode ? "Ask Mouse AI to do anything…" : "Ask Mouse AI anything…"}
          className="max-h-[160px] w-full resize-none bg-transparent px-3.5 pb-1 pt-3 text-[13px] text-text-primary outline-none placeholder:text-text-muted"
        />

        <div className="flex items-center justify-between gap-2 px-2.5 pb-2 pt-1">
          <button
            onClick={onToggleAgentMode}
            title="Agent mode: Mouse AI can run commands, research, edit files, and control your PC to complete a task"
            className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] font-semibold transition ${
              agentMode
                ? "border-border-focus bg-accent-soft text-accent"
                : "border-border-default bg-transparent text-text-secondary hover:bg-bg-hover"
            }`}
          >
            <Bot size={12} />
            Agent {agentMode ? "ON" : "OFF"}
          </button>

          <button
            onClick={submit}
            disabled={disabled || !value.trim()}
            title="Send (Enter)"
            className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-full bg-accent text-white transition hover:scale-105 hover:bg-accent-light disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:scale-100"
          >
            <ArrowUp size={15} />
          </button>
        </div>
      </div>

      <div className="mx-auto flex max-w-[760px] items-center gap-3.5 px-1 pt-2">
        <HintButton onClick={() => askAtCursor().catch(() => {})} label="Ask at cursor" shortcut="Ctrl+Shift+Space" />
        <HintButton onClick={onToggleHoverMode} label="Hover mode" shortcut="Ctrl+Shift+Q" icon={<Eye size={10} />} />
        <span className="flex items-center gap-1 text-[10px] text-text-muted">
          <MousePointerClick size={10} />
          Ctrl+Shift+S
        </span>
      </div>
    </div>
  );
}

function HintButton({
  onClick,
  label,
  shortcut,
  icon,
}: {
  onClick: () => void;
  label: string;
  shortcut: string;
  icon?: React.ReactNode;
}) {
  return (
    <button onClick={onClick} className="flex items-center gap-1 text-[10px] text-text-muted transition hover:text-text-secondary">
      {icon}
      {label}: {shortcut}
    </button>
  );
}
