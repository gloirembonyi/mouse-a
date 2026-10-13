import { useEffect, useRef } from "react";
import clsx from "clsx";
import { Image as ImageIcon, StickyNote } from "lucide-react";
import { docReadDocx, docReadPptx } from "../../../lib/tauri";
import { EmptyNotice, ViewerNote, ViewerSkeleton } from "./common";
import { cannotOpenDetail, useLoaded, type StatusReporter } from "./util";

function useStatus(onStatus: StatusReporter, items: string[] | null) {
  const ref = useRef(onStatus);
  ref.current = onStatus;
  const itemsRef = useRef(items);
  itemsRef.current = items;
  // `key` stands in for `items` (a fresh array every render).
  const key = items?.join("|") ?? "";
  useEffect(() => {
    if (itemsRef.current) ref.current(itemsRef.current);
  }, [key]);
}

// Word document, read-only. The HTML is generated in Rust from
// word/document.xml with every bit of text escaped and only simple
// formatting tags emitted, so it's safe to inject.
export function DocxViewer({ path, onStatus }: { path: string; onStatus: StatusReporter }) {
  const { data, error, loading } = useLoaded(path, docReadDocx);
  useStatus(onStatus, data ? [`${data.words.toLocaleString()} words`, "Read-only"] : null);

  if (loading) return <ViewerSkeleton />;
  if (error || !data) return <EmptyNotice title="Can't preview this document" detail={cannotOpenDetail(error ?? "")} path={path} />;
  if (!data.html.trim()) return <EmptyNotice title="Nothing to show" detail="This document has no readable text." path={path} />;
  return (
    <>
      {data.truncated && <ViewerNote>Long document - showing the beginning. Open it in Word for the rest.</ViewerNote>}
      <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto bg-bg-secondary">
        <article
          className="doc-preview docx-view mx-auto my-6 max-w-[780px] rounded-xl border border-border-soft bg-bg-primary px-10 py-9 text-[14px] leading-[1.75] text-text-primary shadow-pop"
          dangerouslySetInnerHTML={{ __html: data.html }}
        />
      </div>
    </>
  );
}

// PowerPoint, read-only: one card per slide with its title, bullet text
// (indented by level), picture count and speaker notes.
export function PptxViewer({ path, onStatus }: { path: string; onStatus: StatusReporter }) {
  const { data, error, loading } = useLoaded(path, docReadPptx);
  useStatus(onStatus, data ? [`${data.length} slide${data.length === 1 ? "" : "s"}`, "Read-only"] : null);

  if (loading) return <ViewerSkeleton />;
  if (error || !data) return <EmptyNotice title="Can't preview this presentation" detail={cannotOpenDetail(error ?? "")} path={path} />;
  if (!data.length) return <EmptyNotice title="No slides" detail="This presentation has no slides." path={path} />;
  return (
    <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto bg-bg-secondary">
      <div className="mx-auto flex max-w-[760px] flex-col gap-4 px-5 py-6">
        {data.map((s) => {
          const empty = !s.title && !s.lines.length;
          return (
            <section key={s.number} className="overflow-hidden rounded-xl border border-border-default bg-bg-primary shadow-pop">
              <div className="flex items-center gap-2 border-b border-border-soft px-4 py-2">
                <span className="flex h-5 min-w-5 items-center justify-center rounded-md bg-accent-soft px-1.5 text-[10.5px] font-semibold tabular-nums text-accent">
                  {s.number}
                </span>
                <span className="text-[11px] uppercase tracking-wide text-text-muted">Slide</span>
                {s.images > 0 && (
                  <span className="ml-auto flex items-center gap-1 text-[11px] text-text-muted">
                    <ImageIcon size={11} />
                    {s.images} {s.images === 1 ? "picture" : "pictures"}
                  </span>
                )}
              </div>
              <div className="px-6 py-5">
                {s.title && <h3 className="mb-3 text-[17px] font-semibold leading-snug text-text-primary">{s.title}</h3>}
                {s.lines.length > 0 && (
                  <ul className="flex flex-col gap-1.5 text-[13px] leading-relaxed text-text-secondary">
                    {s.lines.map((l, i) => (
                      <li key={i} className="flex gap-2 whitespace-pre-wrap" style={{ paddingLeft: l.level * 18 }}>
                        <span className={clsx("mt-[9px] h-1 w-1 flex-shrink-0 rounded-full", l.level ? "bg-text-muted" : "bg-accent")} />
                        <span className="min-w-0">{l.text}</span>
                      </li>
                    ))}
                  </ul>
                )}
                {empty && <p className="text-[12px] italic text-text-muted">No text on this slide.</p>}
              </div>
              {s.notes && (
                <div className="flex gap-2 border-t border-border-soft bg-bg-secondary px-6 py-3 text-[12px] leading-relaxed text-text-muted">
                  <StickyNote size={12} className="mt-[3px] flex-shrink-0" />
                  <p className="whitespace-pre-wrap">{s.notes}</p>
                </div>
              )}
            </section>
          );
        })}
      </div>
    </div>
  );
}
