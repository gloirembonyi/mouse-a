import { useEffect, useMemo, useRef, useState } from "react";
import CodeMirror, { type ViewUpdate } from "@uiw/react-codemirror";
import clsx from "clsx";
import { Braces, Check, ChevronRight, Code, Columns2, Copy, ExternalLink, Eye, FolderSearch, Loader2, Save, Sparkles, Table2 } from "lucide-react";
import {
  assetUrl,
  fsFileInfo,
  fsOpenDefault,
  fsReadDataUrl,
  fsReadTextFile,
  fsReveal,
  fsWriteTextFile,
  type FileInfo,
} from "../../lib/tauri";
import { languageForPath } from "../../lib/languages";
import { editorTheme } from "../../lib/editorTheme";
import { useTheme } from "../../lib/theme";
import { Markdown } from "../chat/Markdown";
import { IconButton } from "../ui/primitives";
import { EmptyNotice, ViewerSkeleton } from "./viewers/common";
import { cannotOpenDetail, errorText, formatSize } from "./viewers/util";
import { ImageViewer } from "./viewers/ImageViewer";
import { MediaViewer } from "./viewers/MediaViewer";
import { DocxViewer, PptxViewer } from "./viewers/DocumentViewers";
import { CsvTable, WorkbookViewer } from "./viewers/Sheets";

interface EditorProps {
  path: string;
  root: string;
  /** Only the visible tab answers Ctrl+S. */
  active: boolean;
  onDirtyChange: (dirty: boolean) => void;
  onAskAI: () => void;
}

const LANG_NAMES: Record<string, string> = {
  ts: "TypeScript",
  tsx: "TypeScript JSX",
  js: "JavaScript",
  jsx: "JavaScript JSX",
  mjs: "JavaScript",
  cjs: "JavaScript",
  py: "Python",
  rs: "Rust",
  md: "Markdown",
  mdx: "MDX",
  json: "JSON",
  html: "HTML",
  htm: "HTML",
  css: "CSS",
  csv: "CSV",
  tsv: "TSV",
};

const KIND_NAMES: Partial<Record<FileInfo["kind"], string>> = {
  pdf: "PDF",
  docx: "Word document",
  pptx: "Presentation",
  sheet: "Spreadsheet",
  audio: "Audio",
  video: "Video",
  binary: "Binary",
};

const TEXT_LIMIT = 5 * 1024 * 1024;
// Images/PDFs up to this go through IPC as a data URL; bigger ones stream
// from disk via the asset protocol.
const DATA_URL_LIMIT = 25 * 1024 * 1024;
// Image types the chat accepts as an image attachment (see attachments.ts).
const AI_IMAGE_EXTS = ["png", "jpg", "jpeg", "webp"];

/** Markdown/HTML: rendered / side by side / source. CSV: table ("preview") / raw ("code"). */
type ViewMode = "preview" | "split" | "code";

export function Editor({ path, root, active, onDirtyChange, onAskAI }: EditorProps) {
  const { theme } = useTheme();
  const [info, setInfo] = useState<FileInfo | null>(null);
  const [content, setContent] = useState("");
  const [original, setOriginal] = useState("");
  /** Image/PDF source: a data URL, or an asset URL for big files. */
  const [mediaSrc, setMediaSrc] = useState<string | null>(null);
  /** Extra status-bar items reported by the active viewer (dimensions, row counts…). */
  const [viewerStatus, setViewerStatus] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [justSaved, setJustSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [pos, setPos] = useState({ line: 1, col: 1 });

  const ext = (path.split(".").pop() ?? "").toLowerCase();
  const isMarkdown = ext === "md" || ext === "mdx" || ext === "markdown";
  const isHtml = ext === "html" || ext === "htm" || ext === "xhtml";
  const isCsv = ext === "csv" || ext === "tsv" || ext === "tab";
  const hasPreview = isMarkdown || isHtml || isCsv;
  // Markdown, HTML and CSV open rendered; switch to Split or Edit/Raw to change them.
  const [mode, setMode] = useState<ViewMode>("preview");
  const dirty = content !== original;
  const extensions = useMemo(() => [...languageForPath(path), editorTheme(theme)], [path, theme]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setMediaSrc(null);
    setViewerStatus([]);
    setMode("preview");
    (async () => {
      try {
        const i = await fsFileInfo(path);
        if (cancelled) return;
        setInfo(i);
        if (i.kind === "image" || i.kind === "pdf") {
          const url = i.size > DATA_URL_LIMIT ? assetUrl(path) : await fsReadDataUrl(path);
          if (!cancelled) setMediaSrc(url);
        } else if (i.kind === "text" && i.size <= TEXT_LIMIT) {
          // CodeMirror works in LF; CRLF is restored on save (see save()).
          const text = (await fsReadTextFile(path)).replace(/\r\n/g, "\n");
          if (!cancelled) {
            setContent(text);
            setOriginal(text);
          }
        }
        // Documents, sheets and media load inside their own viewers.
      } catch (err) {
        if (!cancelled) setError(errorText(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [path]);

  const onDirtyRef = useRef(onDirtyChange);
  onDirtyRef.current = onDirtyChange;
  useEffect(() => onDirtyRef.current(dirty), [dirty]);

  const save = async () => {
    if (!dirty || saving) return;
    setSaving(true);
    try {
      await fsWriteTextFile(path, info?.crlf ? content.replace(/\n/g, "\r\n") : content);
      setOriginal(content);
      setJustSaved(true);
      setTimeout(() => setJustSaved(false), 1400);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setSaving(false);
    }
  };

  const saveRef = useRef(save);
  saveRef.current = save;
  useEffect(() => {
    if (!active) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void saveRef.current();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active]);

  const copyPath = async () => {
    await navigator.clipboard.writeText(path).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1300);
  };

  const crumbs = (path.toLowerCase().startsWith(root.toLowerCase()) ? path.slice(root.length) : path)
    .split(/[\\/]/)
    .filter(Boolean);
  const lineCount = useMemo(() => content.split("\n").length, [content]);
  const isText = info?.kind === "text" && info.size <= TEXT_LIMIT;
  const kind = info?.kind;
  const canAskAI = isText || kind === "docx" || kind === "pptx" || kind === "sheet" || (kind === "image" && AI_IMAGE_EXTS.includes(ext));
  const codeVisible = isText && (!hasPreview || mode !== "preview");
  // Viewer-reported status only while that viewer is on screen.
  const showViewerStatus = !isText || (isCsv && mode === "preview");

  // Pretty-print JSON in place - an ordinary (undoable, unsaved) edit.
  const formatJson = () => {
    try {
      const next = JSON.stringify(JSON.parse(content), null, 2) + (content.endsWith("\n") ? "\n" : "");
      if (next !== content) setContent(next);
      setError(null);
    } catch (err) {
      setError(`Not valid JSON - ${errorText(err)}`);
    }
  };

  const codeView = (
    <div className="min-h-0 flex-1 overflow-hidden">
      <CodeMirror
        value={content}
        theme="none"
        height="100%"
        extensions={extensions}
        onChange={setContent}
        onUpdate={(u: ViewUpdate) => {
          if (!u.selectionSet && !u.docChanged) return;
          const head = u.state.selection.main.head;
          const line = u.state.doc.lineAt(head);
          setPos({ line: line.number, col: head - line.from + 1 });
        }}
        className="h-full text-[12.5px]"
      />
    </div>
  );

  const previewView = isCsv ? (
    <CsvTable text={content} ext={ext} onStatus={setViewerStatus} />
  ) : isHtml ? (
    // sandbox="" = no scripts, no same-origin access, no navigation: a static render.
    <iframe title={`Preview of ${path}`} sandbox="" srcDoc={content} className="min-h-0 w-full flex-1 border-0 bg-white" />
  ) : (
    <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto">
      <article className="doc-preview mx-auto max-w-[760px] px-8 py-6 text-[14px] leading-[1.75] text-text-primary">
        {content.trim() ? <Markdown>{content}</Markdown> : <p className="text-text-muted">This file is empty.</p>}
      </article>
    </div>
  );

  let body: React.ReactNode;
  if (loading) {
    body = <ViewerSkeleton />;
  } else if (error && !info) {
    body = <EmptyNotice title="Can't open this file" detail={cannotOpenDetail(error)} path={path} />;
  } else if (kind === "image" && mediaSrc) {
    body = <ImageViewer src={mediaSrc} path={path} onStatus={setViewerStatus} />;
  } else if (kind === "pdf" && mediaSrc) {
    body = <iframe title={path} src={mediaSrc} className="min-h-0 w-full flex-1 border-0 bg-white" />;
  } else if (kind === "docx") {
    body = <DocxViewer path={path} onStatus={setViewerStatus} />;
  } else if (kind === "pptx") {
    body = <PptxViewer path={path} onStatus={setViewerStatus} />;
  } else if (kind === "sheet") {
    body = <WorkbookViewer path={path} onStatus={setViewerStatus} />;
  } else if (kind === "audio" || kind === "video") {
    body = <MediaViewer kind={kind} path={path} onStatus={setViewerStatus} />;
  } else if (info && !isText) {
    body = (
      <EmptyNotice
        title={info.kind === "binary" ? "No preview for this file type" : "File too large to edit here"}
        detail={
          info.kind === "binary"
            ? `${formatSize(info.size)} binary file - open it with the app it belongs to.`
            : `${formatSize(info.size)} - files over ${formatSize(TEXT_LIMIT)} open better in their usual app.`
        }
        path={path}
      />
    );
  } else if (hasPreview && mode === "preview") {
    body = previewView;
  } else if (hasPreview && !isCsv && mode === "split") {
    body = (
      <div className="flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 flex-col border-r border-border-soft">{codeView}</div>
        <div className="flex min-w-0 flex-1 flex-col">{previewView}</div>
      </div>
    );
  } else {
    body = codeView;
  }

  return (
    <div className="flex h-full min-w-0 flex-1 flex-col">
      <div className="flex h-8 flex-shrink-0 items-center justify-between gap-2 border-b border-border-soft px-3">
        <div className="flex min-w-0 items-center gap-0.5 overflow-hidden text-[11.5px] text-text-muted" title={path}>
          {crumbs.map((c, i) => (
            <span key={i} className="flex min-w-0 items-center gap-0.5">
              {i > 0 && <ChevronRight size={11} className="flex-shrink-0 opacity-60" />}
              <span className={clsx("truncate", i === crumbs.length - 1 && "text-text-secondary")}>{c}</span>
            </span>
          ))}
        </div>
        <div className="flex flex-shrink-0 items-center gap-0.5">
          {hasPreview && isText && (
            <div className="mr-1 flex rounded-md bg-bg-tertiary p-0.5">
              {isCsv ? (
                <>
                  <ModeButton title="Table" icon={<Table2 size={12} />} active={mode === "preview"} onClick={() => setMode("preview")} />
                  <ModeButton title="Raw text" icon={<Code size={12} />} active={mode === "code"} onClick={() => setMode("code")} />
                </>
              ) : (
                <>
                  <ModeButton title="Preview" icon={<Eye size={12} />} active={mode === "preview"} onClick={() => setMode("preview")} />
                  <ModeButton title="Split" icon={<Columns2 size={12} />} active={mode === "split"} onClick={() => setMode("split")} />
                  <ModeButton title="Edit" icon={<Code size={12} />} active={mode === "code"} onClick={() => setMode("code")} />
                </>
              )}
            </div>
          )}
          {isText && ext === "json" && (
            <IconButton title="Format JSON" size="sm" onClick={formatJson}>
              <Braces size={12} />
            </IconButton>
          )}
          <IconButton title={copied ? "Copied" : "Copy path"} size="sm" onClick={copyPath}>
            {copied ? <Check size={12} /> : <Copy size={12} />}
          </IconButton>
          <IconButton title="Show in File Explorer" size="sm" onClick={() => fsReveal(path).catch(() => {})}>
            <FolderSearch size={13} />
          </IconButton>
          <IconButton title="Open with default app" size="sm" onClick={() => fsOpenDefault(path).catch(() => {})}>
            <ExternalLink size={12} />
          </IconButton>
          {canAskAI && (
            <button
              onClick={onAskAI}
              title="Attach this file to the chat and ask about it"
              className="ml-0.5 flex items-center gap-1 rounded-md px-2 py-1 text-[11.5px] text-text-secondary transition hover:bg-accent-soft hover:text-accent"
            >
              <Sparkles size={12} />
              Ask AI
            </button>
          )}
          {isText && (
            <button
              onClick={save}
              disabled={!dirty || saving}
              title="Save (Ctrl+S)"
              className={clsx(
                "flex items-center gap-1 rounded-md px-2 py-1 text-[11.5px] font-medium transition disabled:cursor-default",
                dirty ? "btn-brand" : "text-text-muted",
              )}
            >
              {saving ? (
                <Loader2 size={12} className="animate-spin" />
              ) : dirty ? (
                <Save size={12} />
              ) : (
                <Check size={12} className={justSaved ? "text-success" : ""} />
              )}
              {saving ? "Saving" : dirty ? "Save" : "Saved"}
            </button>
          )}
        </div>
      </div>

      {error && info && <div className="flex-shrink-0 bg-danger-soft px-3 py-1.5 text-[11.5px] text-danger">{error}</div>}

      {body}

      <div className="flex h-6 flex-shrink-0 items-center justify-end gap-3 border-t border-border-soft bg-bg-secondary px-3 text-[10.5px] text-text-muted">
        {dirty && <span className="text-warning">Unsaved</span>}
        {showViewerStatus && viewerStatus.map((s) => <span key={s}>{s}</span>)}
        {codeVisible && (
          <span>
            Ln {pos.line}, Col {pos.col}
          </span>
        )}
        {isText && <span>{lineCount} lines</span>}
        {info?.crlf && <span>CRLF</span>}
        {info && <span>{formatSize(info.size)}</span>}
        <span>
          {info && info.kind !== "text"
            ? (KIND_NAMES[info.kind] ?? (ext ? ext.toUpperCase() : info.kind.toUpperCase()))
            : (LANG_NAMES[ext] ?? (ext ? ext.toUpperCase() : "Plain text"))}
        </span>
      </div>
    </div>
  );
}

function ModeButton({ icon, active, onClick, title }: { icon: React.ReactNode; active: boolean; onClick: () => void; title: string }) {
  return (
    <button
      onClick={onClick}
      title={title}
      aria-label={title}
      className={clsx(
        "flex items-center justify-center rounded px-1.5 py-0.5 transition",
        active ? "bg-bg-primary text-text-primary shadow-sm" : "text-text-muted hover:text-text-secondary",
      )}
    >
      {icon}
    </button>
  );
}
