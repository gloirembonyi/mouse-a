import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { onPtyExit, onPtyOutput, ptyResize, ptySpawn, ptyWrite } from "../../lib/tauri";

interface TerminalPaneProps {
  cwd: string | null;
}

// A single global PTY session backs this pane (see terminal.rs — one
// writer/master slot, not a pool) — spawn is guarded at module scope so
// switching away from the Terminal tab and back doesn't start a second shell
// process on top of the first.
let spawned = false;

export function TerminalPane({ cwd }: TerminalPaneProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new Terminal({
      fontFamily: "'JetBrains Mono', 'Fira Code', Consolas, monospace",
      fontSize: 12.5,
      theme: {
        background: "#0a0d12",
        foreground: "#e9edf3",
        cursor: "#769fcd",
        selectionBackground: "rgba(118, 159, 205, 0.3)",
      },
      cursorBlink: true,
      scrollback: 5000,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);
    fitAddon.fit();
    termRef.current = term;

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
    // spawn, and this component is kept mounted (not remounted per-tab, see
    // RightDock) for the app's lifetime.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return <div ref={containerRef} className="h-full w-full overflow-hidden px-2 py-1.5" />;
}
