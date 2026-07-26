import { useEffect, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { oneDark } from "@codemirror/theme-one-dark";
import { Check, Code, Eye, Loader2, Save } from "lucide-react";
import { fsReadTextFile, fsWriteTextFile } from "../../lib/tauri";
import { languageForPath } from "../../lib/languages";
import { Markdown } from "../chat/Markdown";

interface EditorProps {
  path: string | null;
}

export function Editor({ path }: EditorProps) {
  const [content, setContent] = useState("");
  const [original, setOriginal] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<"code" | "preview">("code");

  const isMarkdown = /\.mdx?$/i.test(path ?? "");
  const dirty = content !== original;

  useEffect(() => {
    if (!path) return;
    setLoading(true);
    setError(null);
    setMode("code");
    fsReadTextFile(path)
      .then((text) => {
        setContent(text);
        setOriginal(text);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoading(false));
  }, [path]);

  const save = async () => {
    if (!path || !dirty) return;
    setSaving(true);
    try {
      await fsWriteTextFile(path, content);
      setOriginal(content);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "s") {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, content, dirty]);

  if (!path) {
    return <div className="flex flex-1 items-center justify-center text-xs text-text-muted">Select a file to open it.</div>;
  }

  return (
    <div className="flex h-full min-w-0 flex-1 flex-col">
      <div className="flex flex-shrink-0 items-center justify-between border-b border-border-default px-3 py-1.5">
        <div className="flex items-center gap-1.5 overflow-hidden text-[11.5px]">
          <span className="truncate text-text-secondary">{path}</span>
          {dirty && <span className="h-1.5 w-1.5 flex-shrink-0 rounded-full bg-accent" title="Unsaved changes" />}
        </div>
        <div className="flex flex-shrink-0 items-center gap-1">
          {isMarkdown && (
            <div className="flex rounded-md border border-border-default p-0.5">
              <ModeButton icon={<Code size={12} />} active={mode === "code"} onClick={() => setMode("code")} />
              <ModeButton icon={<Eye size={12} />} active={mode === "preview"} onClick={() => setMode("preview")} />
            </div>
          )}
          <button
            onClick={save}
            disabled={!dirty || saving}
            title="Save (Ctrl+S)"
            className="flex items-center gap-1 rounded-md border border-border-default px-2 py-1 text-[11px] font-medium text-text-secondary transition hover:border-accent hover:bg-accent-soft hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
          >
            {saving ? <Loader2 size={11} className="animate-spin" /> : dirty ? <Save size={11} /> : <Check size={11} />}
            {saving ? "Saving…" : dirty ? "Save" : "Saved"}
          </button>
        </div>
      </div>

      {error && <div className="flex-shrink-0 bg-danger-soft px-3 py-1.5 text-[11px] text-danger">{error}</div>}

      {loading ? (
        <div className="flex flex-1 items-center justify-center text-xs text-text-muted">Loading…</div>
      ) : mode === "preview" ? (
        <div className="scrollbar-thin flex-1 overflow-y-auto px-4 py-3 text-[13px] text-text-primary">
          <Markdown>{content}</Markdown>
        </div>
      ) : (
        <div className="scrollbar-thin flex-1 overflow-y-auto">
          <CodeMirror
            value={content}
            theme={oneDark}
            height="100%"
            extensions={languageForPath(path)}
            onChange={setContent}
            className="h-full text-[12.5px]"
          />
        </div>
      )}
    </div>
  );
}

function ModeButton({ icon, active, onClick }: { icon: React.ReactNode; active: boolean; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className={`flex items-center justify-center rounded px-1.5 py-1 transition ${
        active ? "bg-accent-soft text-accent" : "text-text-muted hover:text-text-secondary"
      }`}
    >
      {icon}
    </button>
  );
}
