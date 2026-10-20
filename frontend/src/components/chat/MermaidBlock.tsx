import { useEffect, useState } from "react";
import { Check, Code2, Copy, Download, Image as ImageIcon } from "lucide-react";
import { CodeBlock } from "./CodeBlock";
import { useTheme, type Theme } from "../../lib/theme";
import { downloadKuduFile, encodeText } from "../../lib/kuduFiles";

// ```mermaid fences as real diagrams. mermaid is big, so it is loaded on first use
// (its own chunk). The caller only mounts this once the fence is CLOSED, so a
// streaming answer never re-renders the diagram per token; the code + theme are the
// only inputs, so a finished message renders exactly once (again on a theme switch).

type MermaidApi = typeof import("mermaid").default;
let api: Promise<MermaidApi> | null = null;
function loadMermaid() {
  api ??= import("mermaid").then((m) => m.default);
  return api;
}

// mermaid.render is not re-entrant (shared config + a scratch node) - one at a time.
let queue: Promise<unknown> = Promise.resolve();
let seq = 0;

async function renderSvg(code: string, theme: Theme): Promise<string> {
  const run = queue.then(async () => {
    const mermaid = await loadMermaid();
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: theme === "dark" ? "dark" : "default",
      fontFamily: "inherit",
    });
    const id = `kudu-mermaid-${++seq}`;
    try {
      const { svg } = await mermaid.render(id, code);
      return svg;
    } finally {
      // A failed render leaves its scratch element (and an error SVG) in <body>.
      document.getElementById(id)?.remove();
      document.getElementById(`d${id}`)?.remove();
    }
  });
  queue = run.catch(() => {});
  return run;
}

export function MermaidBlock({ code }: { code: string }) {
  const { theme } = useTheme();
  const [svg, setSvg] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [showSource, setShowSource] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let alive = true;
    setFailed(false);
    renderSvg(code, theme)
      .then((s) => alive && setSvg(s))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [code, theme]);

  if (failed) {
    return (
      <div>
        <CodeBlock code={code} language="mermaid" />
        <p className="-mt-1 mb-2 text-[11px] text-text-muted">Diagram couldn't be drawn - showing the source.</p>
      </div>
    );
  }

  const copy = async () => {
    await navigator.clipboard.writeText(code).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  const downloadSvg = () => {
    if (!svg) return;
    downloadKuduFile({ name: "diagram.svg", mime: "image/svg+xml", b64: encodeText(svg) }).catch(() => {});
  };
  const btn =
    "flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-text-muted transition hover:bg-bg-hover hover:text-text-primary disabled:opacity-40";

  return (
    <div className="my-2.5 overflow-hidden rounded-xl border border-border-default bg-bg-secondary">
      <div className="flex items-center justify-between border-b border-border-soft px-3 py-1.5">
        <span className="font-mono text-[10px] uppercase tracking-wide text-text-muted">diagram</span>
        <span className="flex items-center gap-1">
          <button onClick={() => setShowSource((v) => !v)} className={btn} title={showSource ? "Show the diagram" : "Show the Mermaid source"}>
            {showSource ? <ImageIcon size={11} /> : <Code2 size={11} />}
            {showSource ? "Diagram" : "Source"}
          </button>
          <button onClick={copy} className={btn} title="Copy the Mermaid source">
            {copied ? <Check size={11} /> : <Copy size={11} />}
            {copied ? "Copied" : "Copy"}
          </button>
          <button onClick={downloadSvg} disabled={!svg} className={btn} title="Download as SVG">
            <Download size={11} />
            SVG
          </button>
        </span>
      </div>
      {showSource ? (
        <pre className="scrollbar-thin overflow-x-auto whitespace-pre px-3 py-2 font-mono text-[12px] text-text-secondary">{code}</pre>
      ) : svg ? (
        // mermaid's own output under securityLevel "strict" (labels escaped, no scripts).
        <div className="flex justify-center overflow-x-auto p-3 [&_svg]:h-auto [&_svg]:max-w-full" dangerouslySetInnerHTML={{ __html: svg }} />
      ) : (
        <div className="flex h-24 items-center justify-center text-[12px] text-text-muted">Drawing diagram…</div>
      )}
    </div>
  );
}
