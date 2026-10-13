// Small RFC 4180-style parser for the editor's CSV/TSV table view: quoted
// fields, doubled quotes ("") and newlines inside quotes. Stops after
// `maxRows` so a huge file can't build a giant array.

export interface ParsedTable {
  rows: string[][];
  /** More rows exist past maxRows. */
  truncated: boolean;
}

/** Tab for .tsv; otherwise whichever of , ; | \t appears most in the first line
 * (European Excel exports use ";"). */
export function detectDelimiter(text: string, ext: string): string {
  if (ext === "tsv" || ext === "tab") return "\t";
  const end = text.indexOf("\n");
  const line = end === -1 ? text.slice(0, 4096) : text.slice(0, end);
  let best = ",";
  let bestCount = 0;
  for (const d of [",", ";", "\t", "|"]) {
    let count = 0;
    let quoted = false;
    for (const ch of line) {
      if (ch === '"') quoted = !quoted;
      else if (ch === d && !quoted) count++;
    }
    if (count > bestCount) {
      best = d;
      bestCount = count;
    }
  }
  return best;
}

export function parseDelimited(text: string, delimiter: string, maxRows: number): ParsedTable {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  let i = text.charCodeAt(0) === 0xfeff ? 1 : 0; // UTF-8 BOM from Excel
  const n = text.length;

  const endRow = () => {
    row.push(field);
    field = "";
    // A blank line is not a row.
    if (!(row.length === 1 && row[0] === "")) rows.push(row);
    row = [];
  };

  while (i < n) {
    const ch = text[i];
    if (quoted) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i += 2;
          continue;
        }
        quoted = false;
      } else {
        field += ch;
      }
      i++;
      continue;
    }
    if (ch === '"' && field === "") {
      quoted = true;
    } else if (ch === delimiter) {
      row.push(field);
      field = "";
    } else if (ch === "\n" || ch === "\r") {
      if (ch === "\r" && text[i + 1] === "\n") i++;
      endRow();
      if (rows.length > maxRows) break;
    } else {
      field += ch;
    }
    i++;
  }
  if (field !== "" || row.length) endRow();

  const truncated = rows.length > maxRows;
  if (truncated) rows.length = maxRows;
  return { rows, truncated };
}
