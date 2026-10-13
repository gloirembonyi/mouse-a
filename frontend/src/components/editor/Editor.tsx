import { useEffect, useMemo, useRef, useState } from "react";
import CodeMirror, { type ViewUpdate } from "@uiw/react-codemirror";
import clsx from "clsx";
import {
  Check,
  ChevronRight,
  Code,
  Columns2,
  Copy,
  ExternalLink,
  Eye,
  FileWarning,
  FolderSearch,
  Loader2,
  Save,
  Sparkles,
} from "lucide-react";
import {
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
  css: "CSS",
};

const TEXT_LIMIT = 5 * 1024 * 1024;

type MdMode = "preview" | "split" | "code";

function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function Editor({ path, root, active, onDirtyChange, onAskAI }: EditorProps) {
  const { theme } = useTheme();
  const [info, setInfo] = useState<FileInfo | null>(null);
  const [content, setContent] = useState("");
  const [original, setOriginal] = useState("");
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [justSaved, setJustSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [pos, setPos] = useState({ line: 1, col: 1 });

  const ext = (path.split(".").pop() ?? "").toLowerCase();
  const isMarkdown = ext === "md" || ext === "mdx" || ext === "markdown";
  // Markdown opens as a readable document; switch to Split or Edit to change it.
  const [mdMode, setMdMode] = useState<MdMode>("preview");
  const dirty = content !== original;
  const extensions = useMemo(() => [...languageForPath(path), editorTheme(theme)], [path, theme]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setDataUrl(null);
    setMdMode("preview");
    (async () => {
      try {
        const i = await fsFileInfo(path);
        if (cancelled) return;
        setInfo(i);
        if (i.kind === "image" || i.kind === "pdf") {
          const url = await fsReadDataUrl(path);
          if (!cancelled) setDataUrl(url);
        } else if (i.kind === "text" && i.size <= TEXT_LIMIT) {
          // CodeMirror works in LF; CRLF is restored on save (see save()).
          const text = (await fsReadTextFile(path)).replace(/\r\n/g, "\n");
          if (!cancelled) {
            setContent(text);
            setOriginal(text);
          }
        }
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
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
      setError(err instanceof Error ? err.message : String(err));
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

  const previewView = (
    <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto">
      <article className="doc-preview mx-auto max-w-[760px] px-8 py-6 text-[14px] leading-[1.75] text-text-primary">
        {content.trim() ? <Markdown>{content}</Markdown> : <p className="text-text-muted">This file is empty.</p>}
      </article>
    </div>
  );

  let body: React.ReactNode;
  if (loading) {
    body = (
      <div className="flex flex-col gap-2 p-4">
        {[60, 80, 40, 70, 55].map((w, i) => (
          <div key={i} className="h-3 animate-pulse rounded bg-bg-hover" style={{ width: `${w}%` }} />
        ))}
      </div>
    );
  } else if (error && !info) {
    body = (
      <EmptyNotice
        title="Can't open this file"
        detail={/denied|os error 5/i.test(error) ? "Windows won't let apps read it (it's protected or in use)." : error}
        path={path}
      />
    );
  } else if (info?.kind === "image" && dataUrl) {
    body = (
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-auto bg-[repeating-conic-gradient(var(--mx-bg-tertiary)_0%_25%,transparent_0%_50%)] bg-[length:16px_16px] p-6">
        <img src={dataUrl} alt={path} className="max-h-full max-w-full rounded-lg shadow-pop" />
      </div>
    );
  } else if (info?.kind === "pdf" && dataUrl) {
    body = <iframe title={path} src={dataUrl} className="min-h-0 w-full flex-1 border-0 bg-white" />;
  } else if (info && !isText) {
    body = (
      <EmptyNotice
        title={info.kind === "binary" ? "Binary file" : "File too large to edit here"}
        detail={`${formatSize(info.size)} - open it with its usual app instead.`}
        path={path}
      />
    );
  } else if (isMarkdown && mdMode === "preview") {
    body = previewView;
  } else if (isMarkdown && mdMode === "split") {
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
          {isMarkdown && isText && (
            <div className="mr-1 flex rounded-md bg-bg-tertiary p-0.5">
              <ModeButton title="Preview" icon={<Eye size={12} />} active={mdMode === "preview"} onClick={() => setMdMode("preview")} />
              <ModeButton title="Split" icon={<Columns2 size={12} />} active={mdMode === "split"} onClick={() => setMdMode("split")} />
              <ModeButton title="Edit" icon={<Code size={12} />} active={mdMode === "code"} onClick={() => setMdMode("code")} />
            </div>
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
          {isText && (
            <>
              <button
                onClick={onAskAI}
                title="Attach this file to the chat and ask about it"
                className="ml-0.5 flex items-center gap-1 rounded-md px-2 py-1 text-[11.5px] text-text-secondary transition hover:bg-accent-soft hover:text-accent"
              >
                <Sparkles size={12} />
                Ask AI
              </button>
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
            </>
          )}
        </div>
      </div>

      {error && info && <div className="flex-shrink-0 bg-danger-soft px-3 py-1.5 text-[11.5px] text-danger">{error}</div>}

      {body}

      <div className="flex h-6 flex-shrink-0 items-center justify-end gap-3 border-t border-border-soft bg-bg-secondary px-3 text-[10.5px] text-text-muted">
        {dirty && <span className="text-warning">Unsaved</span>}
        {isText && (!isMarkdown || mdMode !== "preview") && (
          <span>
            Ln {pos.line}, Col {pos.col}
          </span>
        )}
        {isText && <span>{lineCount} lines</span>}
        {info?.crlf && <span>CRLF</span>}
        {info && <span>{formatSize(info.size)}</span>}
        <span>
          {info && info.kind !== "text" ? info.kind.toUpperCase() : (LANG_NAMES[ext] ?? (ext ? ext.toUpperCase() : "Plain text"))}
        </span>
      </div>
    </div>
  );
}

function EmptyNotice({ title, detail, path }: { title: string; detail: string; path: string }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
      <FileWarning size={28} className="text-text-muted" />
      <div>
        <p className="text-[13px] font-medium text-text-primary">{title}</p>
        <p className="mt-1 max-w-[320px] text-[12px] text-text-muted">{detail}</p>
      </div>
      <div className="flex gap-2">
        <button
          onClick={() => fsOpenDefault(path).catch(() => {})}
          className="btn-brand flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[12px] font-medium"
        >
          <ExternalLink size={12} />
          Open with default app
        </button>
        <button
          onClick={() => fsReveal(path).catch(() => {})}
          className="flex items-center gap-1.5 rounded-lg border border-border-default px-3 py-1.5 text-[12px] text-text-secondary transition hover:bg-bg-hover"
        >
          <FolderSearch size={12} />
          Show in Explorer
        </button>
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
