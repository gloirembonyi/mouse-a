import { useMemo, useState } from "react";
import clsx from "clsx";
import type { EditorView } from "@codemirror/view";
import { BookOpen, Check, Sparkles, X } from "lucide-react";
import { Switch } from "../ui/primitives";
import { GROUP_COLOR, GROUP_LABEL, CATEGORY_LABEL, groupOf, isSafeFix, textStats, type QuillGroup } from "../../lib/quill/engine";
import { applyIssue, fixAll, gotoIssue, ignoreOne, refreshQuill, type Issue } from "../../lib/quill/cmQuill";
import { removeWord, updateQuillPrefs, useQuillPrefs } from "../../lib/quill/store";

interface QuillPanelProps {
  view: EditorView | null;
  issues: Issue[];
  onClose: () => void;
}

const GROUPS: QuillGroup[] = ["correctness", "grammar", "clarity", "style", "delivery"];

const fmtTime = (s: number) => (s < 60 ? `${s}s` : `${Math.round(s / 60)} min`);

// The issues list, Fix all, picky mode and the personal dictionary for the open file. Everything runs
// on this device (engine.ts), so it works with no internet and no sign-in.
export function QuillPanel({ view, issues, onClose }: QuillPanelProps) {
  const prefs = useQuillPrefs();
  const [tab, setTab] = useState<"issues" | "settings">("issues");
  const [off, setOff] = useState<Set<QuillGroup>>(new Set());

  const counts = useMemo(() => {
    const c: Record<QuillGroup, number> = { correctness: 0, grammar: 0, clarity: 0, style: 0, delivery: 0 };
    for (const i of issues) c[groupOf(i.match.category)]++;
    return c;
  }, [issues]);
  const shown = useMemo(() => issues.filter((i) => !off.has(groupOf(i.match.category))), [issues, off]);
  const safe = useMemo(() => issues.filter((i) => isSafeFix(i.match)).length, [issues]);
  // Issues change after every check, which is exactly when the numbers can have changed.
  // oxlint-disable-next-line react-hooks/exhaustive-deps
  const stats = useMemo(() => textStats(view?.state.doc.toString() ?? ""), [view, issues]);

  const rerun = () => view?.dispatch({ effects: refreshQuill.of(null) });
  const setPicky = (v: boolean) => {
    updateQuillPrefs({ picky: v });
    rerun();
  };

  return (
    <aside className="flex w-[268px] flex-shrink-0 flex-col border-l border-border-soft bg-bg-secondary text-[12px]">
      <div className="flex h-9 flex-shrink-0 items-center justify-between border-b border-border-soft px-3">
        <div className="flex items-center gap-1.5 font-medium text-text-primary">
          <Sparkles size={13} className="text-accent" />
          Kudu Quill
        </div>
        <button onClick={onClose} aria-label="Close Kudu Quill" className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text-primary">
          <X size={13} />
        </button>
      </div>

      <div className="flex border-b border-border-soft">
        {(["issues", "settings"] as const).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={clsx("flex-1 py-1.5 capitalize transition", tab === t ? "border-b-2 border-accent text-text-primary" : "text-text-muted hover:text-text-secondary")}
          >
            {t}
            {t === "issues" && issues.length > 0 && <span className="ml-1 rounded-full bg-bg-tertiary px-1.5 text-[10px]">{issues.length}</span>}
          </button>
        ))}
      </div>

      {tab === "issues" ? (
        <div className="scrollbar-thin flex min-h-0 flex-1 flex-col overflow-y-auto">
          <div className="flex flex-wrap gap-1 border-b border-border-soft p-2">
            {GROUPS.map((g) => (
              <button
                key={g}
                onClick={() => setOff((s) => { const n = new Set(s); if (n.has(g)) n.delete(g); else n.add(g); return n; })}
                title={off.has(g) ? `Show ${GROUP_LABEL[g].toLowerCase()}` : `Hide ${GROUP_LABEL[g].toLowerCase()}`}
                className={clsx("flex items-center gap-1 rounded-full border border-border-soft px-2 py-0.5 text-[11px] transition", off.has(g) ? "opacity-40" : "text-text-secondary hover:bg-bg-hover")}
              >
                <span className="h-1.5 w-1.5 rounded-full" style={{ background: GROUP_COLOR[g] }} />
                {counts[g]} {GROUP_LABEL[g].toLowerCase()}
              </button>
            ))}
          </div>

          {safe > 0 && (
            <button
              onClick={() => view && fixAll(view)}
              className="btn-brand m-2 flex items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-[11.5px] font-medium"
            >
              <Check size={12} />
              Fix {safe} {safe === 1 ? "issue" : "issues"}
            </button>
          )}

          {shown.length === 0 ? (
            <div className="px-4 py-8 text-center text-text-muted">
              {issues.length === 0 ? (stats.words ? "No issues found. Nice writing." : "Start typing and Kudu Quill checks as you go.") : "Everything here is hidden by the filters above."}
            </div>
          ) : (
            <ul className="flex flex-col">
              {shown.map((i, idx) => {
                const g = groupOf(i.match.category);
                const first = i.match.replacements[0]?.value;
                return (
                  <li key={`${i.from}-${i.match.id}-${idx}`} className="group border-b border-border-soft px-3 py-2 hover:bg-bg-hover">
                    <button onClick={() => view && gotoIssue(view, i)} className="block w-full text-left">
                      <div className="mb-0.5 flex items-center gap-1.5 text-[10.5px] font-semibold uppercase tracking-wide text-text-muted">
                        <span className="h-1.5 w-1.5 rounded-full" style={{ background: GROUP_COLOR[g] }} />
                        {CATEGORY_LABEL[i.match.category] ?? i.match.category}
                      </div>
                      <div className="text-text-secondary">
                        <span className="font-medium text-text-primary underline decoration-2 underline-offset-2" style={{ textDecorationColor: GROUP_COLOR[g] }}>
                          {i.text.length > 40 ? `${i.text.slice(0, 40)}…` : i.text}
                        </span>
                      </div>
                      <div className="mt-0.5 text-text-muted">{i.match.message}</div>
                    </button>
                    <div className="mt-1.5 flex items-center gap-1.5">
                      {first !== undefined && view && (
                        <button onClick={() => applyIssue(view, i, first)} className="rounded bg-accent-soft px-2 py-0.5 font-medium text-accent hover:brightness-110">
                          {first === "" ? "Remove" : first}
                        </button>
                      )}
                      <button onClick={() => view && ignoreOne(view, i)} className="rounded px-1.5 py-0.5 text-text-muted hover:bg-bg-tertiary hover:text-text-primary">
                        Ignore
                      </button>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}

          <div className="mt-auto border-t border-border-soft px-3 py-2 text-[11px] text-text-muted">
            <div className="flex justify-between"><span>Words</span><span className="text-text-secondary">{stats.words}</span></div>
            <div className="flex justify-between"><span>Reading time</span><span className="text-text-secondary">{fmtTime(stats.readingTimeSeconds)}</span></div>
            {stats.readabilityLabel && <div className="flex justify-between"><span>Readability</span><span className="text-text-secondary">{stats.readabilityLabel}</span></div>}
          </div>
        </div>
      ) : (
        <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto p-3">
          <div className="mb-4 flex items-start justify-between gap-3">
            <div>
              <div className="font-medium text-text-primary">Check as I write</div>
              <div className="mt-0.5 text-text-muted">Underline problems in text and Markdown files.</div>
            </div>
            <Switch on={prefs.enabled} onChange={(v) => { updateQuillPrefs({ enabled: v }); if (!v) onClose(); }} label="Check as I write" />
          </div>

          <div className="flex items-start justify-between gap-3">
            <div>
              <div className="font-medium text-text-primary">Picky mode</div>
              <div className="mt-0.5 text-text-muted">Stricter style and clarity: passive voice, weak words, clichés, long sentences.</div>
            </div>
            <Switch on={prefs.picky} onChange={setPicky} label="Picky mode" />
          </div>

          <div className="mt-4 flex items-center gap-1.5 font-medium text-text-primary">
            <BookOpen size={12} /> Personal dictionary
            <span className="rounded-full bg-bg-tertiary px-1.5 text-[10px] font-normal text-text-muted">{prefs.dictionary.length}</span>
          </div>
          {prefs.dictionary.length === 0 ? (
            <div className="mt-1 text-text-muted">Words you add from a suggestion card appear here.</div>
          ) : (
            <div className="mt-1.5 flex flex-wrap gap-1">
              {prefs.dictionary.map((w) => (
                <span key={w} className="flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-text-secondary">
                  {w}
                  <button aria-label={`Remove ${w}`} onClick={() => { removeWord(w); rerun(); }} className="text-text-muted hover:text-danger">
                    <X size={10} />
                  </button>
                </span>
              ))}
            </div>
          )}

          <div className="mt-4 font-medium text-text-primary">Rules turned off</div>
          {prefs.disabledRules.length === 0 ? (
            <div className="mt-1 text-text-muted">None. Use "Turn rule off" on a suggestion card.</div>
          ) : (
            <ul className="mt-1.5 flex flex-col gap-1">
              {prefs.disabledRules.map((r) => (
                <li key={r} className="flex items-center justify-between rounded bg-bg-tertiary px-2 py-1 text-text-secondary">
                  <span className="truncate">{r}</span>
                  <button onClick={() => { updateQuillPrefs({ disabledRules: prefs.disabledRules.filter((x) => x !== r) }); rerun(); }} className="ml-2 text-accent hover:underline">
                    Turn on
                  </button>
                </li>
              ))}
            </ul>
          )}

          {prefs.ignored.length > 0 && (
            <button onClick={() => { updateQuillPrefs({ ignored: [] }); rerun(); }} className="mt-4 text-accent hover:underline">
              Reset {prefs.ignored.length} ignored {prefs.ignored.length === 1 ? "issue" : "issues"}
            </button>
          )}

          <div className="mt-5 rounded-md border border-border-soft p-2 text-[11px] leading-relaxed text-text-muted">
            Checks run on this computer. Nothing is sent anywhere, and it works offline.
          </div>
        </div>
      )}
    </aside>
  );
}
