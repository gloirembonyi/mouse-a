// Kudu Quill inside a CodeMirror editor: underlines, a hover card with the fix, and the helpers the
// issues panel uses. Checks run locally (engine.ts) a moment after the user stops typing, so it is
// the same experience online and offline.
import { StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, hoverTooltip, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { CATEGORY_LABEL, GROUP_COLOR, checkText, groupOf, isSafeFix, type QuillMatch } from "./engine";
import { addWord, disableRule, getQuillPrefs, ignoreIssue, ignoreKey, toOptions } from "./store";

export interface Issue {
  match: QuillMatch;
  from: number;
  to: number;
  /** The flagged text when the check ran - an edit inside the span drops the issue until the next check. */
  text: string;
}

const setIssues = StateEffect.define<Issue[]>();
/** Dispatch to re-run the check now (a setting changed). */
export const refreshQuill = StateEffect.define<null>();

const decorate = (issues: Issue[]): DecorationSet =>
  Decoration.set(
    issues.map((i) => Decoration.mark({ class: `quill-ul quill-${groupOf(i.match.category)}` }).range(i.from, i.to)),
    true,
  );

interface FieldValue { issues: Issue[]; deco: DecorationSet }

export const issuesField = StateField.define<FieldValue>({
  create: () => ({ issues: [], deco: Decoration.none }),
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setIssues)) return { issues: e.value, deco: decorate(e.value) };
    if (!tr.docChanged) return value;
    const issues: Issue[] = [];
    for (const i of value.issues) {
      const from = tr.changes.mapPos(i.from, 1);
      const to = tr.changes.mapPos(i.to, -1);
      if (to > from && tr.state.doc.sliceString(from, to) === i.text) issues.push({ ...i, from, to });
    }
    return { issues, deco: decorate(issues) };
  },
  provide: (f) => EditorView.decorations.from(f, (v) => v.deco),
});

export const getIssues = (view: EditorView): Issue[] => view.state.field(issuesField).issues;

const keepIssues = (view: EditorView, issues: Issue[]) => view.dispatch({ effects: setIssues.of(issues) });

// ── Actions ──────────────────────────────────────────────────────────────

export function applyIssue(view: EditorView, issue: Issue, value: string) {
  view.dispatch({ changes: { from: issue.from, to: issue.to, insert: value }, userEvent: "input.quill" });
  view.focus();
}

export function gotoIssue(view: EditorView, issue: Issue) {
  view.dispatch({ selection: { anchor: issue.from, head: issue.to }, effects: EditorView.scrollIntoView(issue.from, { y: "center" }) });
  view.focus();
}

/** Apply every objective correction in ONE transaction (so one Ctrl+Z undoes them all). */
export function fixAll(view: EditorView): number {
  const changes = getIssues(view)
    .filter((i) => isSafeFix(i.match))
    .map((i) => ({ from: i.from, to: i.to, insert: i.match.replacements[0].value }));
  if (changes.length) view.dispatch({ changes, userEvent: "input.quill" });
  return changes.length;
}

export function ignoreOne(view: EditorView, issue: Issue) {
  ignoreIssue(issue.match.id, issue.text);
  keepIssues(view, getIssues(view).filter((i) => i !== issue && !(i.match.id === issue.match.id && i.text.toLowerCase() === issue.text.toLowerCase())));
}
export function addToDictionary(view: EditorView, issue: Issue) {
  addWord(issue.text);
  keepIssues(view, getIssues(view).filter((i) => !(i.match.id === "SPELLING" && i.text.toLowerCase() === issue.text.toLowerCase())));
}
export function turnRuleOff(view: EditorView, issue: Issue) {
  disableRule(issue.match.id);
  keepIssues(view, getIssues(view).filter((i) => i.match.id !== issue.match.id));
}

// ── Hover card ───────────────────────────────────────────────────────────

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function card(view: EditorView, issue: Issue): HTMLElement {
  const m = issue.match;
  const root = el("div", "quill-card");
  const head = el("div", "quill-card-head");
  const dot = el("span", "quill-dot");
  dot.style.background = GROUP_COLOR[groupOf(m.category)];
  head.append(dot, el("span", "quill-card-cat", CATEGORY_LABEL[m.category] ?? m.category));
  root.append(head);

  if (m.id === "SPELLING" && m.replacements.length) {
    const row = el("div", "quill-card-was");
    row.append(el("s", undefined, issue.text), el("span", undefined, " → "));
    root.append(row);
  }
  root.append(el("div", "quill-card-msg", m.message));

  if (m.replacements.length) {
    const reps = el("div", "quill-card-reps");
    m.replacements.slice(0, 5).forEach((r, idx) => {
      const b = el("button", idx === 0 ? "quill-rep quill-rep-main" : "quill-rep", r.value === "" ? "Remove" : r.value);
      b.type = "button";
      b.addEventListener("mousedown", (e) => { e.preventDefault(); applyIssue(view, issue, r.value); });
      reps.append(b);
    });
    root.append(reps);
  }

  const acts = el("div", "quill-card-acts");
  const act = (label: string, run: () => void) => {
    const b = el("button", "quill-act", label);
    b.type = "button";
    b.addEventListener("mousedown", (e) => { e.preventDefault(); run(); });
    acts.append(b);
  };
  act("Ignore", () => ignoreOne(view, issue));
  if (m.id === "SPELLING") act("Add to dictionary", () => addToDictionary(view, issue));
  else if (m.source === "rules" || m.source === "languagetool") act("Turn rule off", () => turnRuleOff(view, issue));
  root.append(acts);
  return root;
}

const hover = hoverTooltip(
  (view, pos) => {
    const issue = getIssues(view).find((i) => pos >= i.from && pos <= i.to);
    if (!issue) return null;
    return { pos: issue.from, end: issue.to, above: true, create: () => ({ dom: card(view, issue) }) };
  },
  { hoverTime: 120 },
);

// ── The checker ──────────────────────────────────────────────────────────

/** `onIssues` fires whenever the set of issues changes, for the panel and status bar. */
export function quillExtension(onIssues: (issues: Issue[]) => void): Extension {
  const plugin = ViewPlugin.fromClass(
    class {
      timer = 0;
      seq = 0;
      view: EditorView;
      constructor(view: EditorView) {
        this.view = view;
        this.schedule(60);
        onIssues(getIssues(view));
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.transactions.some((tr) => tr.effects.some((e) => e.is(refreshQuill)))) this.schedule(u.docChanged ? 380 : 0);
        const before = u.startState.field(issuesField);
        const after = u.state.field(issuesField);
        if (before !== after) onIssues(after.issues);
      }
      schedule(delay: number) {
        clearTimeout(this.timer);
        this.timer = window.setTimeout(() => void this.run(), delay);
      }
      async run() {
        const view = this.view;
        const text = view.state.doc.toString();
        const seq = ++this.seq;
        const prefs = getQuillPrefs();
        let matches: QuillMatch[];
        try {
          matches = await checkText(text, toOptions(prefs));
        } catch {
          return;
        }
        // The user kept typing, or another check started: this answer is for text that no longer exists.
        if (seq !== this.seq || view.state.doc.toString() !== text) return;
        const ignored = new Set(prefs.ignored);
        const issues: Issue[] = [];
        for (const m of matches) {
          const from = m.offset;
          const to = m.offset + m.length;
          const flagged = text.slice(from, to);
          if (to <= from || ignored.has(ignoreKey(m.id, flagged))) continue;
          issues.push({ match: m, from, to, text: flagged });
        }
        view.dispatch({ effects: setIssues.of(issues) });
      }
      destroy() {
        clearTimeout(this.timer);
        this.seq++;
      }
    },
  );
  return [issuesField, plugin, hover];
}
