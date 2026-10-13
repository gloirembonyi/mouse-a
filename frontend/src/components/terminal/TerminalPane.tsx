import { useCallback, useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@xterm/xterm/css/xterm.css";
import { Eraser, Plus, RotateCcw, Sparkles, SquareTerminal, X } from "lucide-react";
import { newId, onPtyExit, onPtyOutput, ptyKill, ptyResize, ptySpawn, ptyWrite } from "../../lib/tauri";
import { subscribeRunInTerminal } from "../../lib/openFile";
import { useTheme, type Theme } from "../../lib/theme";
import { IconButton } from "../ui/primitives";

interface TerminalPaneProps {
  cwd: string | null;
  visible: boolean;
  /** Attach recent terminal output to the chat. */
  onAskAI: (text: string, title: string) => void;
}

const THEMES: Record<Theme, ITheme> = {
  dark: {
    background: "#121827",
    foreground: "#e9f0f6",
    cursor: "#8fb3dc",
    selectionBackground: "rgba(118, 159, 205, 0.35)",
  },
  light: {
    background: "#f7fbfc",
    foreground: "#12223a",
    cursor: "#5578a8",
    selectionBackground: "rgba(85, 120, 168, 0.25)",
    black: "#12223a",
    white: "#6a87a4",
    brightBlack: "#4a6886",
    brightWhite: "#3a5572",
    yellow: "#8a5a00",
    brightYellow: "#8a5a00",
    green: "#0f7a4f",
    brightGreen: "#0f7a4f",
  },
};

interface SessionHandle {
  write: (data: string) => void;
  clear: () => void;
  restart: () => void;
  recentText: (lines: number) => string;
  focus: () => void;
}

interface Session {
  id: string;
  title: string;
  exited: boolean;
}

let counter = 0;
const makeSession = (): Session => ({ id: newId(), title: `PowerShell ${++counter}`, exited: false });

// Tabs of independent PTY sessions (terminal.rs keys them by id). All of them
// stay mounted (hidden when inactive) so switching tabs never kills a shell.
export function TerminalPane({ cwd, visible, onAskAI }: TerminalPaneProps) {
  const [sessions, setSessions] = useState<Session[]>(() => [makeSession()]);
  const [active, setActive] = useState(() => sessions[0].id);
  const handles = useRef(new Map<string, SessionHandle>());

  const add = () => {
    const s = makeSession();
    setSessions((prev) => [...prev, s]);
    setActive(s.id);
  };

  const close = (id: string) => {
    ptyKill(id).catch(() => {});
    handles.current.delete(id);
    setSessions((prev) => {
      const next = prev.filter((s) => s.id !== id);
      if (next.length === 0) {
        const s = makeSession();
        setActive(s.id);
        return [s];
      }
      if (id === active) setActive(next[next.length - 1].id);
      return next;
    });
  };

  const markExited = useCallback((id: string, exited: boolean) => {
    setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, exited } : s)));
  }, []);

  // "Run in terminal" from a chat code block: type it into the active shell.
  const activeRef = useRef(active);
  activeRef.current = active;
  useEffect(
    () =>
      subscribeRunInTerminal((command) => {
        const h = handles.current.get(activeRef.current);
        if (!h) return;
        h.write(command.replace(/\r?\n/g, "\r") + "\r");
        h.focus();
      }),
    [],
  );

  const current = handles.current;
  const activeSession = sessions.find((s) => s.id === active);

  return (
    <div className="flex h-full w-full flex-col bg-bg-primary">
      <div className="flex h-9 flex-shrink-0 items-center gap-1 border-b border-border-soft bg-bg-secondary px-1.5">
        <div className="scrollbar-thin flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto">
          {sessions.map((s) => (
            <div
              key={s.id}
              onClick={() => setActive(s.id)}
              className={clsx(
                "group flex h-7 flex-shrink-0 cursor-pointer items-center gap-1.5 rounded-md pl-2 pr-1 text-[12px] transition",
                s.id === active ? "bg-bg-primary text-text-primary shadow-sm" : "text-text-muted hover:text-text-secondary",
              )}
            >
              <SquareTerminal size={12} className={s.exited ? "text-danger" : ""} />
              <span className="max-w-[120px] truncate">{s.title}</span>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  close(s.id);
                }}
                aria-label={`Close ${s.title}`}
                className="flex h-5 w-5 items-center justify-center rounded opacity-0 transition hover:bg-bg-hover group-hover:opacity-100"
              >
                <X size={11} />
              </button>
            </div>
          ))}
          <IconButton title="New terminal" size="sm" onClick={add}>
            <Plus size={14} />
          </IconButton>
        </div>
        <button
          onClick={() => {
            const text = current.get(active)?.recentText(150) ?? "";
            if (text.trim()) onAskAI(text, `Terminal output (${activeSession?.title ?? "terminal"}, cwd ${cwd ?? "?"})`);
          }}
          title="Attach the recent output to the chat (great for errors)"
          className="flex h-7 items-center gap-1 rounded-md px-2 text-[11.5px] text-text-secondary transition hover:bg-accent-soft hover:text-accent"
        >
          <Sparkles size={12} />
          Ask AI
        </button>
        <IconButton title="Clear" size="sm" onClick={() => current.get(active)?.clear()}>
          <Eraser size={13} />
        </IconButton>
        <IconButton title="Restart this terminal" size="sm" onClick={() => current.get(active)?.restart()}>
          <RotateCcw size={13} />
        </IconButton>
      </div>

      <div className="relative min-h-0 flex-1">
        {sessions.map((s) => (
          <XtermSession
            key={s.id}
            id={s.id}
            cwd={cwd}
            shown={visible && s.id === active}
            register={(h) => handles.current.set(s.id, h)}
            onExitChange={(exited) => markExited(s.id, exited)}
          />
        ))}
      </div>
    </div>
  );
}

function XtermSession({
  id,
  cwd,
  shown,
  register,
  onExitChange,
}: {
  id: string;
  cwd: string | null;
  shown: boolean;
  register: (h: SessionHandle) => void;
  onExitChange: (exited: boolean) => void;
}) {
  const { theme } = useTheme();
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const spawnedRef = useRef(false);
  const cwdRef = useRef(cwd);
  const lastCwd = useRef(cwd);
  cwdRef.current = cwd;
  const exitRef = useRef(onExitChange);
  exitRef.current = onExitChange;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new Terminal({
      fontFamily: "'JetBrains Mono', 'Cascadia Code', Consolas, monospace",
      fontSize: 12.5,
      lineHeight: 1.25,
      theme: THEMES[document.documentElement.dataset.theme === "light" ? "light" : "dark"],
      cursorBlink: true,
      scrollback: 10000,
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon((_e, uri) => void openUrl(uri).catch(() => {})));
    term.open(container);
    termRef.current = term;
    fitRef.current = fit;

    const spawn = () => {
      // Only with real dimensions: spawning while hidden gave the shell a
      // bogus width, and every line it drew afterwards wrapped/overwrote.
      if (!container.offsetParent || container.clientWidth < 50) return;
      fit.fit();
      spawnedRef.current = true;
      exitRef.current(false);
      ptySpawn(id, cwdRef.current, term.cols, term.rows).catch((err) => {
        spawnedRef.current = false;
        term.writeln(`\r\n\x1b[31mCould not start a terminal: ${err instanceof Error ? err.message : String(err)}\x1b[0m`);
      });
    };

    // Windows-style clipboard: Ctrl+C copies when text is selected (else it's
    // ^C to the shell), Ctrl+V / Ctrl+Shift+V paste, Ctrl+Shift+C copies.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      const key = e.key.toLowerCase();
      if (e.ctrlKey && key === "c" && (e.shiftKey || term.hasSelection())) {
        void navigator.clipboard.writeText(term.getSelection());
        term.clearSelection();
        return false;
      }
      if (e.ctrlKey && key === "v") {
        e.preventDefault();
        navigator.clipboard
          .readText()
          .then((t) => {
            if (t) void ptyWrite(id, t);
          })
          .catch(() => {});
        return false;
      }
      return true;
    });
    const onContextMenu = (e: MouseEvent) => {
      e.preventDefault();
      if (term.hasSelection()) {
        void navigator.clipboard.writeText(term.getSelection());
        term.clearSelection();
      } else {
        navigator.clipboard
          .readText()
          .then((t) => {
            if (t) void ptyWrite(id, t);
          })
          .catch(() => {});
      }
    };
    container.addEventListener("contextmenu", onContextMenu);

    const dataDisposable = term.onData((data) => {
      if (!spawnedRef.current) return;
      ptyWrite(id, data).catch(() => {});
    });

    const unlistenOutput = onPtyOutput((e) => {
      if (e.id === id) term.write(e.data);
    });
    const unlistenExit = onPtyExit((e) => {
      if (e.id !== id) return;
      spawnedRef.current = false;
      exitRef.current(true);
      term.writeln("\r\n\x1b[90m[process exited - press Restart to start a new shell]\x1b[0m");
    });

    const resizeObserver = new ResizeObserver(() => {
      if (!container.offsetParent || container.clientWidth < 50) return; // hidden
      if (!spawnedRef.current) {
        spawn();
        return;
      }
      fit.fit();
      ptyResize(id, term.cols, term.rows).catch(() => {});
    });
    resizeObserver.observe(container);

    register({
      write: (data) => {
        if (spawnedRef.current) ptyWrite(id, data).catch(() => {});
      },
      clear: () => term.clear(),
      restart: () => {
        term.reset();
        spawnedRef.current = false;
        spawn();
      },
      recentText: (lines) => {
        const buf = term.buffer.active;
        const out: string[] = [];
        const end = buf.baseY + buf.cursorY;
        for (let i = Math.max(0, end - lines); i <= end; i++) {
          out.push(buf.getLine(i)?.translateToString(true) ?? "");
        }
        return out.join("\n").replace(/\n{3,}/g, "\n\n").trim();
      },
      focus: () => term.focus(),
    });

    return () => {
      dataDisposable.dispose();
      unlistenOutput.then((fn) => fn());
      unlistenExit.then((fn) => fn());
      resizeObserver.disconnect();
      container.removeEventListener("contextmenu", onContextMenu);
      term.dispose();
      termRef.current = null;
    };
    // Mount-once: the session lives as long as its tab.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (termRef.current) termRef.current.options.theme = THEMES[theme];
  }, [theme]);

  // Switching projects moves the running shell into the new folder rather
  // than killing it (the shell is PowerShell, see terminal.rs).
  useEffect(() => {
    if (!cwd || cwd === lastCwd.current) return;
    lastCwd.current = cwd;
    if (spawnedRef.current) ptyWrite(id, `Set-Location -LiteralPath '${cwd.replace(/'/g, "''")}'\r`).catch(() => {});
  }, [cwd, id]);

  useEffect(() => {
    if (!shown) return;
    requestAnimationFrame(() => {
      const term = termRef.current;
      const fit = fitRef.current;
      if (!term || !fit || !spawnedRef.current) return;
      fit.fit();
      ptyResize(id, term.cols, term.rows).catch(() => {});
      term.focus();
    });
  }, [shown, id]);

  return (
    <div
      ref={containerRef}
      className={clsx("absolute inset-0 overflow-hidden px-2 py-1.5", shown ? "block" : "hidden")}
      style={{ background: THEMES[theme].background }}
    />
  );
}
