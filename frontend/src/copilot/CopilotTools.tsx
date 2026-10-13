import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Check, ClipboardPaste, Copy, Keyboard, Plus, Square, Trash2, Wrench, X } from "lucide-react";
import { Markdown } from "../components/chat/Markdown";
import { codeBlocks, speakable, type InsertFn, type useCopilotAgent } from "./copilotHooks";

// Hands-on pieces of the copilot bar (copilot.rs): the "Fix it" agent run,
// private paste/type into the user's app, and cue cards for teaching/demos.
// Everything here renders only in the bar, which is invisible to screen share.

// ── Fix it: the desktop agent, from the bar ──────────────────────

export function AgentPanel({ agent, onInsert }: { agent: ReturnType<typeof useCopilotAgent>; onInsert: InsertFn }) {
  const { run } = agent;
  const logRef = useRef<HTMLOListElement>(null);
  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [run?.steps.length]);
  if (!run) return null;
  const visible = run.text ? run.steps.slice(-3) : run.steps.slice(-6);
  return (
    <section className="cp-agent" aria-live="polite">
      <div className="cp-agent-head">
        <Wrench size={13} />
        <span className="cp-agent-task" title={run.task}>
          {run.task}
        </span>
        {run.running ? (
          <button type="button" className="cp-agent-stop" onClick={agent.stop} title="Stop the agent">
            <Square size={10} fill="currentColor" />
            Stop
          </button>
        ) : (
          <button type="button" className="cp-icon" onClick={agent.dismiss} aria-label="Dismiss">
            <X size={14} />
          </button>
        )}
      </div>

      {visible.length > 0 && (
        <ol ref={logRef} className="cp-agent-log">
          {visible.map((s, i) => (
            <li key={run.steps.length - visible.length + i} className={clsx(`is-${s.kind}`, run.running && i === visible.length - 1 && "is-current")}>
              {s.message.length > 160 ? `${s.message.slice(0, 160)}…` : s.message}
            </li>
          ))}
        </ol>
      )}
      {run.running && visible.length === 0 && <p className="cp-agent-wait">Looking at your screen and project…</p>}

      {run.confirm && (
        <div className="cp-confirm">
          <p>{run.confirm.message}</p>
          <div>
            <button type="button" className="cp-confirm-deny" onClick={() => agent.answer(false)}>
              Deny
            </button>
            <button type="button" className="cp-confirm-allow" onClick={() => agent.answer(true)}>
              Allow
            </button>
          </div>
        </div>
      )}

      {run.error && <p className="cp-error">{run.error}</p>}
      {run.text && (
        <div className="cp-answer">
          <Markdown>{run.text}</Markdown>
          <InsertBar text={run.text} onInsert={onInsert} />
        </div>
      )}
    </section>
  );
}

// ── Private insert ───────────────────────────────────────────────

/** Paste / Type buttons for each code block (or the answer itself). */
export function InsertBar({ text, onInsert }: { text: string; onInsert: InsertFn }) {
  const blocks = codeBlocks(text).slice(0, 3);
  const items = blocks.length
    ? blocks.map((b, i) => ({ label: blocks.length > 1 ? `${b.lang} ${i + 1}` : b.lang, text: b.code }))
    : [{ label: "answer", text: speakable(text) }];
  if (!items[0]?.text) return null;
  return (
    <div className="cp-insert">
      {items.map((it) => (
        <span key={it.label} className="cp-insert-group">
          <span className="cp-insert-label">{it.label}</span>
          <button type="button" onClick={() => onInsert(it.text, "paste", it.label)} title="Paste into the app you were working in - your clipboard is restored after">
            <ClipboardPaste size={12} />
            Paste
          </button>
          <button type="button" onClick={() => onInsert(it.text, "type", it.label)} title="Type it out naturally into your app, as if you were typing live. Click anywhere to stop.">
            <Keyboard size={12} />
            Type
          </button>
        </span>
      ))}
    </div>
  );
}

// ── Cue cards ────────────────────────────────────────────────────
// Private notes for a demo or lesson: talking points, the code to write next.
// Only the presenter sees them; Paste/Type puts them into the shared app.

interface Cue {
  id: string;
  title: string;
  body: string;
}

const CUES_KEY = "mouse-ai-copilot-cues";

function loadCues(): Cue[] {
  try {
    const raw = JSON.parse(localStorage.getItem(CUES_KEY) ?? "[]");
    return Array.isArray(raw) ? raw.filter((c) => c && typeof c.body === "string") : [];
  } catch {
    return [];
  }
}

export function CuesView({ onInsert }: { onInsert: InsertFn }) {
  const [cues, setCues] = useState<Cue[]>(loadCues);
  const [open, setOpen] = useState<string | null>(() => loadCues()[0]?.id ?? null);
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [copied, setCopied] = useState<string | null>(null);

  const save = (next: Cue[]) => {
    setCues(next);
    localStorage.setItem(CUES_KEY, JSON.stringify(next));
  };

  const add = () => {
    if (!body.trim()) return;
    const cue = { id: crypto.randomUUID(), title: title.trim() || body.trim().split("\n")[0].slice(0, 40), body };
    save([...cues, cue]);
    setOpen(cue.id);
    setTitle("");
    setBody("");
    setAdding(false);
  };

  const idx = cues.findIndex((c) => c.id === open);

  return (
    <section className="cp-cues">
      <div className="cp-cues-head">
        <span>Cue cards</span>
        <span className="cp-cues-hint">Only you see these</span>
        {cues.length > 1 && idx >= 0 && (
          <span className="cp-cues-step tabular-nums">
            <button type="button" disabled={idx <= 0} onClick={() => setOpen(cues[idx - 1].id)} aria-label="Previous cue">
              ‹
            </button>
            {idx + 1}/{cues.length}
            <button type="button" disabled={idx >= cues.length - 1} onClick={() => setOpen(cues[idx + 1].id)} aria-label="Next cue">
              ›
            </button>
          </span>
        )}
        <button type="button" className="cp-cues-add" onClick={() => setAdding((v) => !v)}>
          {adding ? <X size={13} /> : <Plus size={13} />}
          {adding ? "Cancel" : "New"}
        </button>
      </div>

      {adding && (
        <div className="cp-cue-form">
          <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Title (e.g. Step 3 - add the route)" />
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            rows={5}
            placeholder={"Talking points, or code in ``` fences - Paste/Type puts it into your app"}
          />
          <button type="button" className="cp-chip cp-chip-accent" onClick={add} disabled={!body.trim()}>
            Save cue
          </button>
        </div>
      )}

      {cues.length === 0 && !adding ? (
        <p className="cp-empty">
          Prepare what you'll say or type during a demo or lesson. It stays on your side; Paste or Type sends it into the app you're
          sharing.
        </p>
      ) : (
        <ul className="cp-cue-list">
          {cues.map((c) => (
            <li key={c.id} className={clsx(open === c.id && "is-open")}>
              <button type="button" className="cp-cue-title" onClick={() => setOpen(open === c.id ? null : c.id)}>
                {c.title}
              </button>
              {open === c.id && (
                <div className="cp-cue-body">
                  <div className="cp-answer">
                    <Markdown>{c.body}</Markdown>
                  </div>
                  <div className="cp-cue-actions">
                    <InsertBar text={c.body} onInsert={onInsert} />
                    <button
                      type="button"
                      className="cp-icon"
                      title="Copy"
                      onClick={() => {
                        void navigator.clipboard.writeText(codeBlocks(c.body)[0]?.code ?? c.body);
                        setCopied(c.id);
                        setTimeout(() => setCopied(null), 1200);
                      }}
                    >
                      {copied === c.id ? <Check size={13} /> : <Copy size={13} />}
                    </button>
                    <button type="button" className="cp-icon" title="Delete cue" onClick={() => save(cues.filter((x) => x.id !== c.id))}>
                      <Trash2 size={13} />
                    </button>
                  </div>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
