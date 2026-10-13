import { useEffect, useMemo, useRef, useState } from "react";
import clsx from "clsx";
import { Loader2 } from "lucide-react";
import { docReadSheet, type SheetData } from "../../../lib/tauri";
import { detectDelimiter, parseDelimited } from "../../../lib/csv";
import { EmptyNotice, ViewerNote, ViewerSkeleton } from "./common";
import { cannotOpenDetail, columnName, errorText, type StatusReporter } from "./util";

const ROW_H = 26;
const OVERSCAN = 12;
const NUMERIC = /^[-+]?[$€£¥]?\d[\d,.\s]*%?$/;
const MAX_TABLE_ROWS = 5000;

interface GridProps {
  /** Column header labels (letters for workbooks, the first row for CSV). */
  header: string[];
  rows: string[][];
  rowLabel: (i: number) => number;
  /** Reference shown in the cell bar, e.g. "B12". */
  cellRef: (row: number, col: number) => string;
}

// Spreadsheet-style grid: sticky header and row numbers, fixed row height
// with simple vertical windowing (only the visible rows are in the DOM), so a
// few thousand rows scroll smoothly. Click a cell to see its full value.
function SheetGrid({ header, rows, rowLabel, cellRef }: GridProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(600);
  const [sel, setSel] = useState<{ r: number; c: number } | null>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    setSel(null);
    scrollRef.current?.scrollTo({ top: 0, left: 0 });
  }, [rows]);

  const cols = Math.max(header.length, ...rows.slice(0, 2000).map((r) => r.length), 1);
  // Width from a sample of the content, so columns don't jump while scrolling.
  const widths = useMemo(() => {
    const w: number[] = [];
    for (let c = 0; c < cols; c++) {
      let chars = (header[c] ?? "").length;
      for (let r = 0; r < Math.min(rows.length, 300); r++) chars = Math.max(chars, (rows[r][c] ?? "").length);
      w.push(Math.min(300, Math.max(64, chars * 7.2 + 20)));
    }
    return w;
  }, [header, rows, cols]);
  const numW = Math.max(40, String(rowLabel(rows.length - 1)).length * 8 + 18);
  const total = numW + widths.reduce((a, b) => a + b, 0);

  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((scrollTop + viewH) / ROW_H) + OVERSCAN);
  const selValue = sel ? (rows[sel.r]?.[sel.c] ?? "") : "";

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-7 flex-shrink-0 items-center gap-2 border-b border-border-soft bg-bg-secondary px-2 text-[11.5px]">
        <span className="min-w-[56px] rounded bg-bg-tertiary px-1.5 py-0.5 text-center font-mono text-[11px] text-text-secondary">
          {sel ? cellRef(sel.r, sel.c) : "-"}
        </span>
        <span className="min-w-0 flex-1 select-text truncate text-text-primary" title={selValue}>
          {sel ? selValue || <span className="text-text-muted">(empty)</span> : <span className="text-text-muted">Click a cell to see its value</span>}
        </span>
      </div>
      <div ref={scrollRef} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)} className="scrollbar-thin min-h-0 flex-1 overflow-auto">
        <table className="border-separate border-spacing-0 text-[12px]" style={{ tableLayout: "fixed", width: total }}>
          <colgroup>
            <col style={{ width: numW }} />
            {widths.map((w, i) => (
              <col key={i} style={{ width: w }} />
            ))}
          </colgroup>
          <thead>
            <tr>
              <th className="sticky left-0 top-0 z-20 border-b border-r border-border-default bg-bg-tertiary" style={{ height: ROW_H }} />
              {Array.from({ length: cols }, (_, c) => (
                <th
                  key={c}
                  title={header[c]}
                  className={clsx(
                    "sticky top-0 z-10 truncate border-b border-r border-border-default bg-bg-tertiary px-2 text-left font-semibold text-text-secondary",
                    sel?.c === c && "text-accent",
                  )}
                  style={{ height: ROW_H }}
                >
                  {header[c] ?? ""}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {first > 0 && (
              <tr style={{ height: first * ROW_H }}>
                <td colSpan={cols + 1} />
              </tr>
            )}
            {rows.slice(first, last).map((row, i) => {
              const r = first + i;
              return (
                <tr key={r} style={{ height: ROW_H }} className="group">
                  <td
                    className={clsx(
                      "sticky left-0 z-[5] border-b border-r border-border-soft bg-bg-secondary px-2 text-right font-mono text-[10.5px] tabular-nums text-text-muted",
                      sel?.r === r && "text-accent",
                    )}
                  >
                    {rowLabel(r)}
                  </td>
                  {Array.from({ length: cols }, (_, c) => {
                    const v = row[c] ?? "";
                    const selected = sel?.r === r && sel.c === c;
                    return (
                      <td
                        key={c}
                        onClick={() => setSel({ r, c })}
                        title={v.length > 30 ? v : undefined}
                        className={clsx(
                          "cursor-cell truncate border-b border-r border-border-soft px-2 text-text-primary group-hover:bg-bg-hover/50",
                          NUMERIC.test(v) && "text-right tabular-nums",
                          selected && "bg-accent-soft outline outline-1 -outline-offset-1 outline-accent",
                        )}
                      >
                        {v}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
            {last < rows.length && (
              <tr style={{ height: (rows.length - last) * ROW_H }}>
                <td colSpan={cols + 1} />
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// Excel / LibreOffice workbook (read-only). Each sheet is fetched on demand;
// sheet tabs along the bottom like Excel.
export function WorkbookViewer({ path, onStatus }: { path: string; onStatus: StatusReporter }) {
  const [data, setData] = useState<SheetData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [switching, setSwitching] = useState<string | null>(null);
  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;

  // Only the latest request may land (tab clicks can overtake each other).
  const req = useRef(0);
  const load = (sheet?: string) => {
    const id = ++req.current;
    if (sheet) setSwitching(sheet);
    docReadSheet(path, sheet)
      .then((d) => {
        if (id !== req.current) return;
        setData(d);
        setError(null);
        onStatusRef.current([`${d.total_rows.toLocaleString()} rows × ${d.total_cols} cols`, "Read-only"]);
      })
      .catch((err) => id === req.current && setError(errorText(err)))
      .finally(() => id === req.current && setSwitching(null));
  };
  const loadRef = useRef(load);
  loadRef.current = load;

  useEffect(() => {
    setData(null);
    setError(null);
    loadRef.current();
  }, [path]);

  if (error && !data) return <EmptyNotice title="Can't preview this spreadsheet" detail={cannotOpenDetail(error)} path={path} />;
  if (!data) return <ViewerSkeleton />;

  const header = Array.from({ length: Math.max(1, ...data.rows.slice(0, 2000).map((r) => r.length)) }, (_, c) => columnName(data.first_col + c));
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {data.truncated && (
        <ViewerNote>
          Showing the first {data.rows.length.toLocaleString()} of {data.total_rows.toLocaleString()} rows
          {data.total_cols > 200 ? " and 200 columns" : ""} - open it in Excel for everything.
        </ViewerNote>
      )}
      {error && <div className="flex-shrink-0 bg-danger-soft px-3 py-1 text-[11.5px] text-danger">{error}</div>}
      {data.rows.length ? (
        <SheetGrid
          header={header}
          rows={data.rows}
          rowLabel={(i) => data.first_row + i}
          cellRef={(r, c) => `${columnName(data.first_col + c)}${data.first_row + r}`}
        />
      ) : (
        <div className="flex flex-1 items-center justify-center text-[12px] text-text-muted">This sheet is empty.</div>
      )}
      {data.sheets.length > 1 && (
        <div className="scrollbar-thin flex h-8 flex-shrink-0 items-stretch gap-0.5 overflow-x-auto border-t border-border-soft bg-bg-secondary px-1.5">
          {data.sheets.map((s) => (
            <button
              key={s}
              onClick={() => s !== data.sheet && load(s)}
              className={clsx(
                "flex flex-shrink-0 items-center gap-1.5 border-t-2 px-3 text-[11.5px] transition",
                s === data.sheet ? "border-accent bg-bg-primary font-medium text-text-primary" : "border-transparent text-text-muted hover:text-text-secondary",
              )}
            >
              {switching === s && <Loader2 size={11} className="animate-spin" />}
              {s}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// CSV / TSV as a table. Parses the editor's live text, so switching back
// from Raw shows unsaved edits too.
export function CsvTable({ text, ext, onStatus }: { text: string; ext: string; onStatus: StatusReporter }) {
  const parsed = useMemo(() => {
    const delimiter = detectDelimiter(text, ext);
    return { delimiter, ...parseDelimited(text, delimiter, MAX_TABLE_ROWS + 1) };
  }, [text, ext]);
  const header = parsed.rows[0] ?? [];
  const body = useMemo(() => parsed.rows.slice(1), [parsed]);
  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;
  const delimName = { ",": "Comma", ";": "Semicolon", "\t": "Tab", "|": "Pipe" }[parsed.delimiter] ?? "Comma";
  useEffect(() => {
    onStatusRef.current([`${body.length.toLocaleString()}${parsed.truncated ? "+" : ""} rows`, `${delimName}-separated`]);
  }, [body.length, parsed.truncated, delimName]);

  if (!parsed.rows.length) return <div className="flex flex-1 items-center justify-center text-[12px] text-text-muted">This file is empty.</div>;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {parsed.truncated && <ViewerNote>Showing the first {MAX_TABLE_ROWS.toLocaleString()} rows - switch to Raw to see the whole file.</ViewerNote>}
      <SheetGrid header={header} rows={body} rowLabel={(i) => i + 1} cellRef={(r, c) => `${header[c] || columnName(c + 1)} · ${r + 1}`} />
    </div>
  );
}
