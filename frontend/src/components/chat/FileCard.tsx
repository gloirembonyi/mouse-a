import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronDown, Download, Eye, ExternalLink, FileText, FileType, FolderSearch, Loader2, X } from "lucide-react";
import { Menu } from "../ui/Menu";
import { FileIcon } from "../editor/fileIcons";
import { Markdown } from "./Markdown";
import { MermaidBlock } from "./MermaidBlock";
import { detectDelimiter, parseDelimited } from "../../lib/csv";
import { fsReveal } from "../../lib/tauri";
import {
  decodeText,
  downloadKuduFile,
  extOf,
  formatBytes,
  openKuduFile,
  previewKind,
  type KuduFile,
  type SaveFormat,
} from "../../lib/kuduFiles";

// A file the browser agent wrote, shown as a card instead of its raw block (never
// the base64). Preview opens a modal; Download goes through the save dialog; Open
// saves into Documents\Kudu and hands it to the OS default app. A Markdown document
// never reaches the user as raw Markdown by default: Download and Open produce a
// formatted Word .docx, with .md / plain .txt in the Download menu.

export function FileCard({ file }: { file: KuduFile }) {
  const [previewing, setPreviewing] = useState(false);
  const [busy, setBusy] = useState<"download" | "open" | null>(null);
  const [status, setStatus] = useState<{ text: string; path?: string; error?: boolean } | null>(null);
  const available = Boolean(file.b64);
  const size = formatBytes(file.bytes);

  const isDoc = previewKind(file) === "markdown";
  const act = async (kind: "download" | "open", format?: SaveFormat) => {
    setBusy(kind);
    setStatus(null);
    try {
      if (kind === "download") {
        const path = await downloadKuduFile(file, format);
        if (path) setStatus({ text: `Saved to ${path}`, path });
      } else {
        const path = await openKuduFile(file);
        setStatus({ text: `Opened ${path}`, path });
      }
    } catch (e) {
      setStatus({ text: String(e instanceof Error ? e.message : e), error: true });
    } finally {
      setBusy(null);
    }
  };

  const btn =
    "flex items-center gap-1 rounded-md px-2 py-1 text-[11.5px] text-text-secondary transition hover:bg-bg-hover hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-40";

  return (
    <div className="my-2.5 rounded-xl border border-border-default bg-bg-secondary px-3 py-2.5">
      <div className="flex items-center gap-2.5">
        <span className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-lg bg-bg-tertiary">
          <FileIcon name={file.name} size={16} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] font-medium text-text-primary" title={file.name}>
            {file.name}
          </div>
          <div className="text-[11px] text-text-muted">
            {[extOf(file.name).toUpperCase() || "File", size, !available && "too large to transfer"].filter(Boolean).join(" · ")}
          </div>
        </div>
        <div className="flex flex-shrink-0 items-center gap-0.5">
          <button className={btn} disabled={!available} onClick={() => setPreviewing(true)} title="Preview">
            <Eye size={12} />
            Preview
          </button>
          <button
            className={btn}
            disabled={!available || busy !== null}
            onClick={() => act("download")}
            title={isDoc ? "Save as a formatted Word document…" : "Save a copy…"}
          >
            {busy === "download" ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
            {isDoc ? "Download .docx" : "Download"}
          </button>
          {isDoc && available && (
            <Menu
              placement="bottom"
              align="end"
              width={200}
              header="Download as"
              trigger={() => (
                <button className={`${btn} px-1`} disabled={busy !== null} title="Other formats">
                  <ChevronDown size={12} />
                </button>
              )}
              items={[
                { key: "docx", label: "Word (.docx)", icon: <FileType size={13} />, onSelect: () => act("download", "docx") },
                { key: "md", label: "Markdown (.md)", icon: <FileText size={13} />, onSelect: () => act("download", "md") },
                { key: "txt", label: "Plain text (.txt)", icon: <FileText size={13} />, onSelect: () => act("download", "txt") },
              ]}
            />
          )}
          <button className={btn} disabled={!available || busy !== null} onClick={() => act("open")} title={isDoc ? "Open as a Word document" : "Open with the default app"}>
            {busy === "open" ? <Loader2 size={12} className="animate-spin" /> : <ExternalLink size={12} />}
            Open
          </button>
        </div>
      </div>
      {status && (
        <div className={`mt-1.5 flex items-center gap-1.5 text-[11px] ${status.error ? "text-danger" : "text-text-muted"}`}>
          <span className="min-w-0 truncate" title={status.text}>
            {status.text}
          </span>
          {status.path && (
            <button onClick={() => fsReveal(status.path!).catch(() => {})} className="flex flex-shrink-0 items-center gap-1 text-accent hover:underline">
              <FolderSearch size={11} />
              Show
            </button>
          )}
        </div>
      )}
      {previewing && file.b64 && <PreviewModal file={file} onClose={() => setPreviewing(false)} />}
    </div>
  );
}

/** Shown while the fence is still streaming in (its JSON is not complete yet). */
export function FileCardPending() {
  return (
    <div className="my-2.5 flex items-center gap-2 rounded-xl border border-border-default bg-bg-secondary px-3 py-2.5 text-[12px] text-text-muted">
      <Loader2 size={13} className="animate-spin" />
      Receiving file…
    </div>
  );
}

function PreviewModal({ file, onClose }: { file: KuduFile; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return createPortal(
    <div className="fixed inset-0 z-[1000] flex items-center justify-center bg-black/50 p-4" onMouseDown={onClose}>
      <div
        className="flex max-h-full w-full max-w-[920px] flex-col overflow-hidden rounded-2xl border border-border-default bg-bg-primary shadow-pop"
        style={{ height: "min(86vh, 900px)" }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex flex-shrink-0 items-center gap-2 border-b border-border-soft px-4 py-2.5">
          <FileIcon name={file.name} size={14} />
          <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-text-primary">{file.name}</span>
          <span className="text-[11px] text-text-muted">{formatBytes(file.bytes)}</span>
          <button onClick={onClose} title="Close (Esc)" className="rounded-md p-1 text-text-muted transition hover:bg-bg-hover hover:text-text-primary">
            <X size={15} />
          </button>
        </div>
        <div className="scrollbar-thin min-h-0 flex-1 overflow-auto">
          <PreviewBody file={file} />
        </div>
      </div>
    </div>,
    document.body,
  );
}

const MAX_TABLE_ROWS = 2000;

function PreviewBody({ file }: { file: KuduFile }) {
  const kind = previewKind(file);
  const b64 = file.b64!;
  const text = useMemo(() => {
    if (kind === "image") return "";
    try {
      return decodeText(b64);
    } catch {
      return null;
    }
  }, [b64, kind]);

  if (text === null) return <p className="p-4 text-[12px] text-text-muted">This file couldn't be decoded.</p>;

  switch (kind) {
    case "markdown":
      return (
        <div className="px-5 py-4 text-[14px] leading-[1.7] text-text-primary">
          <Markdown>{text}</Markdown>
        </div>
      );
    case "table": {
      const parsed = parseDelimited(text, detectDelimiter(text, extOf(file.name)), MAX_TABLE_ROWS);
      const [head, ...rows] = parsed.rows;
      return (
        <div className="p-3">
          {parsed.truncated && <p className="mb-2 text-[11px] text-text-muted">Showing the first {MAX_TABLE_ROWS.toLocaleString()} rows.</p>}
          <table className="w-full border-collapse text-[12px]">
            {head && (
              <thead>
                <tr>
                  {head.map((h, i) => (
                    <th key={i} className="sticky top-0 border border-border-default bg-bg-tertiary px-2 py-1 text-left font-semibold">
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
            )}
            <tbody>
              {rows.map((r, i) => (
                <tr key={i}>
                  {r.map((c, j) => (
                    <td key={j} className="border border-border-default px-2 py-1 align-top">
                      {c}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    }
    case "json": {
      let pretty = text;
      try {
        pretty = JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        /* not valid JSON - show as is */
      }
      return <Mono text={pretty} />;
    }
    case "mermaid":
      return (
        <div className="p-3">
          <MermaidBlock code={text.trim()} />
        </div>
      );
    case "svg":
      // As an <img>: an SVG's scripts never run there.
      return (
        <div className="flex justify-center p-4">
          <img src={`data:image/svg+xml;base64,${b64}`} alt={file.name} className="max-w-full" />
        </div>
      );
    case "image":
      return (
        <div className="flex justify-center p-4">
          <img src={`data:${file.mime || "image/png"};base64,${b64}`} alt={file.name} className="max-w-full" />
        </div>
      );
    case "html":
      // sandbox="" = no scripts, no forms, no same-origin access.
      return <iframe title={file.name} sandbox="" srcDoc={text} className="h-full w-full border-0 bg-white" />;
    default:
      return <Mono text={text} />;
  }
}

function Mono({ text }: { text: string }) {
  return <pre className="whitespace-pre-wrap break-words px-4 py-3 font-mono text-[12px] leading-relaxed text-text-primary">{text}</pre>;
}
