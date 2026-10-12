import { useEffect, useMemo, useRef, useState } from "react";
import CodeMirror, { type ViewUpdate } from "@uiw/react-codemirror";
import clsx from "clsx";
import { Check, ChevronRight, Code, Eye, Loader2, Save, Sparkles } from "lucide-react";
import { fsReadTextFile, fsWriteTextFile } from "../../lib/tauri";
import { languageForPath } from "../../lib/languages";
import { editorTheme } from "../../lib/editorTheme";
import { useTheme } from "../../lib/theme";
import { Markdown } from "../chat/Markdown";

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
  json: "JSON",
  html: "HTML",
  css: "CSS",
};

export function Editor({ path, root, active, onDirtyChange, onAskAI }: EditorProps) {
  const { theme } = useTheme();
  const [content, setContent] = useState("");
  const [original, setOriginal] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [justSaved, setJustSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<"code" | "preview">("code");
  const [pos, setPos] = useState({ line: 1, col: 1 });

  const ext = (path.split(".").pop() ?? "").toLowerCase();
  const isMarkdown = ext === "md" || ext === "mdx";
  const dirty = content !== original;
  const extensions = useMemo(() => [...languageForPath(path), editorTheme(theme)], [path, theme]);

  useEffect(() => {
    setLoading(true);
    setError(null);
    fsReadTextFile(path)
      .then((text) => {
        setContent(text);
        setOriginal(text);
      })
      .catch((err) => {
        const msg = err instanceof Error ? err.message : String(err);
        setError(/utf-8|invalid data|stream did not contain/i.test(msg) ? "This looks like a binary file — it can't be edited as text." : msg);
      })
      .finally(() => setLoading(false));
  }, [path]);

  const onDirtyRef = useRef(onDirtyChange);
  onDirtyRef.current = onDirtyChange;
  useEffect(() => onDirtyRef.current(dirty), [dirty]);

  const save = async () => {
    if (!dirty || saving) return;
    setSaving(true);
    try {
      await fsWriteTextFile(path, content);
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

  const crumbs = (path.startsWith(root) ? path.slice(root.length) : path).split(/[\\/]/).filter(Boolean);
  const lineCount = useMemo(() => content.split("\n").length, [content]);

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
        <div className="flex flex-shrink-0 items-center gap-1">
          {isMarkdown && (
            <div className="flex rounded-md bg-bg-tertiary p-0.5">
              <ModeButton title="Code" icon={<Code size={12} />} active={mode === "code"} onClick={() => setMode("code")} />
              <ModeButton title="Preview" icon={<Eye size={12} />} active={mode === "preview"} onClick={() => setMode("preview")} />
            </div>
          )}
          <button
            onClick={onAskAI}
            title="Attach this file to the chat and ask about it"
            className="flex items-center gap-1 rounded-md px-2 py-1 text-[11.5px] text-text-secondary transition hover:bg-accent-soft hover:text-accent"
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
            {saving ? <Loader2 size={12} className="animate-spin" /> : dirty ? <Save size={12} /> : <Check size={12} className={justSaved ? "text-success" : ""} />}
            {saving ? "Saving" : dirty ? "Save" : "Saved"}
          </button>
        </div>
      </div>

      {error && <div className="flex-shrink-0 bg-danger-soft px-3 py-1.5 text-[11.5px] text-danger">{error}</div>}

      {loading ? (
        <div className="flex flex-col gap-2 p-4">
          {[60, 80, 40, 70, 55].map((w, i) => (
            <div key={i} className="h-3 animate-pulse rounded bg-bg-hover" style={{ width: `${w}%` }} />
          ))}
        </div>
      ) : error && !content ? null : mode === "preview" ? (
        <div className="scrollbar-thin flex-1 overflow-y-auto px-6 py-4 text-[13px] text-text-primary">
          <Markdown>{content}</Markdown>
        </div>
      ) : (
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
      )}

      <div className="flex h-6 flex-shrink-0 items-center justify-end gap-3 border-t border-border-soft bg-bg-secondary px-3 text-[10.5px] text-text-muted">
        {dirty && <span className="text-warning">Unsaved</span>}
        <span>
          Ln {pos.line}, Col {pos.col}
        </span>
        <span>{lineCount} lines</span>
        <span>{LANG_NAMES[ext] ?? (ext ? ext.toUpperCase() : "Plain text")}</span>
      </div>
    </div>
  );
}

function ModeButton({ icon, active, onClick, title }: { icon: React.ReactNode; active: boolean; onClick: () => void; title: string }) {
  return (
    <button
      onClick={onClick}
      title={title}
      className={clsx(
        "flex items-center justify-center rounded px-1.5 py-0.5 transition",
        active ? "bg-bg-primary text-text-primary shadow-sm" : "text-text-muted hover:text-text-secondary",
      )}
    >
      {icon}
    </button>
  );
}
