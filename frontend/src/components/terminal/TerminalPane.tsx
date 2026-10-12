import { useEffect, useRef } from "react";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { onPtyExit, onPtyOutput, ptyResize, ptySpawn, ptyWrite } from "../../lib/tauri";
import { useTheme, type Theme } from "../../lib/theme";

interface TerminalPaneProps {
  cwd: string | null;
  visible: boolean;
}

const THEMES: Record<Theme, ITheme> = {
  dark: {
    background: "#161616",
    foreground: "#e6e6e6",
    cursor: "#7aa2d6",
    selectionBackground: "rgba(122, 162, 214, 0.3)",
  },
  light: {
    background: "#ffffff",
    foreground: "#1f1f1f",
    cursor: "#3668aa",
    selectionBackground: "rgba(54, 104, 170, 0.2)",
    black: "#1f1f1f",
    white: "#8a8a8a",
    brightWhite: "#5a5a5a",
    yellow: "#9a6b00",
    brightYellow: "#9a6b00",
  },
};

// A single global PTY session backs this pane (see terminal.rs — one
// writer/master slot, not a pool) — spawn is guarded at module scope so
// switching away from the Terminal tab and back doesn't start a second shell
// process on top of the first.
let spawned = false;

export function TerminalPane({ cwd, visible }: TerminalPaneProps) {
  const { theme } = useTheme();
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const lastCwd = useRef(cwd);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new Terminal({
      fontFamily: "'JetBrains Mono', 'Cascadia Code', Consolas, monospace",
      fontSize: 12.5,
      lineHeight: 1.2,
      theme: THEMES[document.documentElement.dataset.theme === "light" ? "light" : "dark"],
      cursorBlink: true,
      scrollback: 5000,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);
    fitAddon.fit();
    termRef.current = term;
    fitRef.current = fitAddon;

    if (!spawned) {
      spawned = true;
      ptySpawn(cwd, term.cols, term.rows).catch((err) => {
        term.writeln(`\r\n\x1b[31mCould not start a terminal: ${err instanceof Error ? err.message : String(err)}\x1b[0m`);
      });
    } else {
      ptyResize(term.cols, term.rows).catch(() => {});
    }

    const dataDisposable = term.onData((data) => {
      ptyWrite(data).catch(() => {});
    });

    const unlistenOutput = onPtyOutput((data) => term.write(data));
    const unlistenExit = onPtyExit(() => {
      term.writeln("\r\n\x1b[90m[process exited]\x1b[0m");
      spawned = false;
    });

    const resizeObserver = new ResizeObserver(() => {
      if (!container.offsetParent) return; // hidden tab: 0×0 would garble the PTY
      fitAddon.fit();
      ptyResize(term.cols, term.rows).catch(() => {});
    });
    resizeObserver.observe(container);

    return () => {
      dataDisposable.dispose();
      unlistenOutput.then((fn) => fn());
      unlistenExit.then((fn) => fn());
      resizeObserver.disconnect();
      term.dispose();
      termRef.current = null;
    };
    // Intentionally runs once on mount — cwd is only used for the initial
    // spawn (later project switches `cd` instead, below), and this component
    // is kept mounted (not remounted per-tab, see RightDock).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (termRef.current) termRef.current.options.theme = THEMES[theme];
  }, [theme]);

  // Switching projects moves the running shell into the new folder rather
  // than killing it (the shell is PowerShell, see terminal.rs).
  useEffect(() => {
    if (!cwd || cwd === lastCwd.current || !spawned) {
      lastCwd.current = cwd;
      return;
    }
    lastCwd.current = cwd;
    ptyWrite(`Set-Location -LiteralPath '${cwd.replace(/'/g, "''")}'\r`).catch(() => {});
  }, [cwd]);

  useEffect(() => {
    if (!visible) return;
    requestAnimationFrame(() => {
      const term = termRef.current;
      if (!term || !fitRef.current) return;
      fitRef.current.fit();
      ptyResize(term.cols, term.rows).catch(() => {});
      term.focus();
    });
  }, [visible]);

  return <div ref={containerRef} className="h-full w-full overflow-hidden bg-bg-primary px-2 py-1.5" />;
}
