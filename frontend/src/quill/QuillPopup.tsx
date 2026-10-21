import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import clsx from "clsx";
import { Check, Copy, Loader2, Sparkles, WifiOff, X } from "lucide-react";
import { onQuillCapture, quillApply, quillHide, quillPending, type QuillCapture } from "../lib/tauri";
import {
  CATEGORY_LABEL, GROUP_COLOR, GROUP_LABEL, applyAll, checkText, groupOf, isSafeFix, textStats, type QuillMatch,
} from "../lib/quill/engine";
import { addWord, ignoreIssue, ignoreKey, toOptions, useQuillPrefs } from "../lib/quill/store";

type Segment = { text: string; issue?: number };

// Kudu Quill anywhere: the window Ctrl+Alt+G opens. It holds the text copied from the app the user
// is writing in, underlines what is wrong (the same engines as the extension, running here, so no
// internet is needed), lets them fix it, and "Apply" pastes the result back over the original.
export function QuillPopup() {
  const prefs = useQuillPrefs();
  const [capture, setCapture] = useState<QuillCapture | null>(null);
  const [text, setText] = useState("");
  const [issues, setIssues] = useState<QuillMatch[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [checking, setChecking] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const original = useRef("");

  const take = useCallback((c: QuillCapture) => {
    setCapture(c);
    setText(c.text);
    original.current = c.text;
    setSelected(null);
    setIssues([]);
    setError(c.error ?? null);
  }, []);

  useEffect(() => {
    void quillPending().then((c) => c && take(c));
    const un = onQuillCapture(take);
    return () => {
      void un.then((f) => f());
    };
  }, [take]);

  // Check whenever the text or the user's settings change.
  useEffect(() => {
    if (!text.trim()) {
      setIssues([]);
      return;
    }
    let live = true;
    setChecking(true);
    const t = window.setTimeout(() => {
      void checkText(text, toOptions(prefs))
        .then((found) => {
          if (!live) return;
          const ignored = new Set(prefs.ignored);
          setIssues(found.filter((m) => !ignored.has(ignoreKey(m.id, text.slice(m.offset, m.offset + m.length)))));
          setSelected(null);
        })
        .catch((e) => live && setError(String(e)))
        .finally(() => live && setChecking(false));
    }, 120);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [text, prefs]);

  const apply = useCallback(async () => {
    if (!text || applying) return;
    setApplying(true);
    setError(null);
    try {
      await quillApply(text);
    } catch (e) {
      setError(String(e));
    } finally {
      setApplying(false);
    }
  }, [text, applying]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") void quillHide();
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        void apply();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [apply]);

  const segments = useMemo<Segment[]>(() => {
    const out: Segment[] = [];
    let at = 0;
    issues.forEach((m, i) => {
      if (m.offset < at) return;
      if (m.offset > at) out.push({ text: text.slice(at, m.offset) });
      out.push({ text: text.slice(m.offset, m.offset + m.length), issue: i });
      at = m.offset + m.length;
    });
    if (at < text.length) out.push({ text: text.slice(at) });
    return out;
  }, [issues, text]);

  const replaceAt = (m: QuillMatch, value: string) => setText((t) => t.slice(0, m.offset) + value + t.slice(m.offset + m.length));
  const sel = selected !== null ? issues[selected] : null;
  const safe = issues.filter(isSafeFix);
  const changed = text !== original.current;
  const stats = useMemo(() => textStats(text), [text]);

  const copy = async () => {
    await navigator.clipboard.writeText(text).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1200);
  };

  return (
    <div className="flex h-screen flex-col overflow-hidden rounded-xl border border-border-strong bg-bg-primary text-[12.5px] text-text-primary">
      <div data-tauri-drag-region className="flex h-9 flex-shrink-0 items-center justify-between border-b border-border-soft px-3">
        <div data-tauri-drag-region className="flex min-w-0 items-center gap-1.5 font-medium">
          <Sparkles size={13} className="flex-shrink-0 text-accent" />
          <span>Kudu Quill</span>
          {capture && !capture.error && (
            <span className="truncate text-[11px] font-normal text-text-muted">
              · {capture.scope === "all" ? "whole text" : "selection"}
              {capture.app ? ` in ${capture.app}` : ""}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1.5">
          <span title="Checks run on this computer - no internet needed" className="flex items-center gap-1 text-[10.5px] text-text-muted">
            <WifiOff size={11} /> offline-ready
          </span>
          <button onClick={() => void quillHide()} aria-label="Close" className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text-primary">
            <X size={13} />
          </button>
        </div>
      </div>

      {error && <div className="flex-shrink-0 bg-danger-soft px-3 py-2 text-[11.5px] text-danger">{error}</div>}

      {!capture ? (
        <div className="flex flex-1 items-center justify-center px-6 text-center text-text-muted">
          Select text in any app and press <kbd className="mx-1 rounded bg-bg-tertiary px-1.5 py-0.5 text-[11px]">Ctrl+Alt+G</kbd>
        </div>
      ) : (
        <>
          <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto px-3.5 py-3">
            <p className="whitespace-pre-wrap break-words text-[13.5px] leading-[1.7]">
              {segments.map((s, i) =>
                s.issue === undefined ? (
                  <span key={i}>{s.text}</span>
                ) : (
                  <span
                    key={i}
                    role="button"
                    tabIndex={0}
                    onClick={() => setSelected(s.issue ?? null)}
                    onKeyDown={(e) => e.key === "Enter" && setSelected(s.issue ?? null)}
                    className={clsx("cursor-pointer rounded-sm underline decoration-2 underline-offset-[3px]", selected === s.issue && "bg-bg-hover")}
                    style={{ textDecorationColor: GROUP_COLOR[groupOf(issues[s.issue].category)] }}
                  >
                    {s.text}
                  </span>
                ),
              )}
            </p>
          </div>

          {sel && (
            <div className="flex-shrink-0 border-t border-border-soft bg-bg-secondary px-3.5 py-2.5">
              <div className="mb-1 flex items-center gap-1.5 text-[10.5px] font-semibold uppercase tracking-wide text-text-muted">
                <span className="h-2 w-2 rounded-full" style={{ background: GROUP_COLOR[groupOf(sel.category)] }} />
                {CATEGORY_LABEL[sel.category] ?? GROUP_LABEL[groupOf(sel.category)]}
              </div>
              <div className="text-text-secondary">{sel.message}</div>
              {sel.replacements.length > 0 && (
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {sel.replacements.slice(0, 5).map((r, i) => (
                    <button
                      key={r.value + i}
                      onClick={() => replaceAt(sel, r.value)}
                      className={clsx("rounded-lg px-2.5 py-1 font-semibold", i === 0 ? "btn-brand" : "bg-bg-tertiary hover:bg-bg-hover")}
                    >
                      {r.value === "" ? "Remove" : r.value}
                    </button>
                  ))}
                </div>
              )}
              <div className="mt-2 flex gap-3 text-[11.5px] text-text-muted">
                <button className="hover:text-text-primary" onClick={() => ignoreIssue(sel.id, text.slice(sel.offset, sel.offset + sel.length))}>Ignore</button>
                {sel.id === "SPELLING" && (
                  <button className="hover:text-text-primary" onClick={() => addWord(text.slice(sel.offset, sel.offset + sel.length))}>Add to dictionary</button>
                )}
              </div>
            </div>
          )}

          <div className="flex flex-shrink-0 items-center justify-between gap-2 border-t border-border-soft px-3 py-2">
            <div className="flex min-w-0 items-center gap-2 text-[11px] text-text-muted">
              {checking ? <Loader2 size={11} className="animate-spin" /> : null}
              <span>{issues.length === 0 ? (checking ? "Checking…" : "No issues") : `${issues.length} ${issues.length === 1 ? "issue" : "issues"}`}</span>
              <span>· {stats.words} words</span>
            </div>
            <div className="flex items-center gap-1.5">
              {safe.length > 0 && (
                <button onClick={() => setText((t) => applyAll(t, safe))} className="rounded-md bg-bg-tertiary px-2.5 py-1 font-medium hover:bg-bg-hover">
                  Fix {safe.length}
                </button>
              )}
              <button onClick={() => void copy()} title="Copy the text" className="flex items-center gap-1 rounded-md px-2 py-1 text-text-secondary hover:bg-bg-hover">
                {copied ? <Check size={12} /> : <Copy size={12} />}
              </button>
              <button
                onClick={() => void apply()}
                disabled={applying || !changed}
                title="Paste back into the app (Ctrl+Enter)"
                className="btn-brand flex items-center gap-1 rounded-md px-3 py-1 font-semibold disabled:opacity-40"
              >
                {applying ? <Loader2 size={12} className="animate-spin" /> : <Check size={12} />}
                Apply
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
