// Markdown -> what a plain field or a rich editor understands. A TypeScript port of the
// extension's shared/text-format.js (GhostTextFormat) - keep the two behaving
// identically: blocks() is shared by toPlain(), toHtml() and the .docx writer (docx.ts).
//   • plain fields / typed text -> toPlain(): headings as their own lines, "• " bullets,
//     no ** / __ / # / backticks
//   • rich editors              -> toHtml(): simple b/i/a/ul/ol/p/br HTML
// The Rust side has the same toPlain for text the agent types (src-tauri/src/text_format.rs).

const MD_SIGNS = [
  /^\s{0,3}#{1,6}\s+\S/m, // heading
  /\*\*[^*\n]+\*\*/, // bold
  /__[^_\n]+__/, // bold
  /^\s*[-*+]\s+\S/m, // bullet
  /^\s*\d+[.)]\s+\S/m, // numbered (weak on its own)
  /\[[^\]\n]+\]\(https?:[^)\s]+\)/, // link
  /`[^`\n]+`/, // inline code
  /^\s*```/m, // fence
  /^\s*>\s+\S/m, // quote
  /^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/m, // rule
  /^\s*\|.+\|\s*$/m, // table row
];

/** True when the text carries Markdown syntax worth converting. */
export function looksLikeMarkdown(text: unknown): boolean {
  const s = String(text || "");
  if (s.length < 3) return false;
  let hits = 0;
  for (const re of MD_SIGNS) if (re.test(s)) hits++;
  // A lone "1. " or "- " list is also how people write plain text - only convert when
  // something unambiguous is present.
  return hits >= 2 || /\*\*[^*\n]+\*\*|^\s{0,3}#{1,6}\s|\[[^\]\n]+\]\(https?:|^\s*```/m.test(s);
}

const ESC: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };
const esc = (s: unknown) => String(s).replace(/[&<>"]/g, (c) => ESC[c]);

export function inlinePlain(s: unknown): string {
  return String(s)
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, (_, t: string, u: string) => (t.trim() === u.trim() ? u : `${t} (${u})`))
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\*\*\*([^*]+)\*\*\*/g, "$1")
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/__([^_]+)__/g, "$1")
    .replace(/(^|[\s(])\*([^*\s][^*\n]*?)\*(?=[\s).,!?:;]|$)/g, "$1$2")
    .replace(/(^|[\s(])_([^_\s][^_\n]*?)_(?=[\s).,!?:;]|$)/g, "$1$2")
    .replace(/~~([^~]+)~~/g, "$1");
}

export function inlineHtml(s: unknown): string {
  let out = esc(s);
  out = out.replace(/`([^`]+)`/g, "<code>$1</code>");
  out = out.replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2">$1</a>');
  out = out.replace(/\*\*\*([^*]+)\*\*\*/g, "<b><i>$1</i></b>");
  out = out.replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>");
  out = out.replace(/__([^_]+)__/g, "<b>$1</b>");
  out = out.replace(/(^|[\s(])\*([^*\s][^*\n]*?)\*(?=[\s).,!?:;]|$)/g, "$1<i>$2</i>");
  out = out.replace(/(^|[\s(])_([^_\s][^_\n]*?)_(?=[\s).,!?:;]|$)/g, "$1<i>$2</i>");
  out = out.replace(/~~([^~]+)~~/g, "<s>$1</s>");
  return out;
}

export interface ListItem {
  depth: number;
  text: string;
}

export type Block =
  | { t: "code"; text: string }
  | { t: "blank" }
  | { t: "h"; level: number; text: string }
  | { t: "hr" }
  | { t: "table"; rows: string[][] }
  | { t: "ul" | "ol"; items: ListItem[] }
  | { t: "quote"; text: string }
  | { t: "p"; lines: string[] };

/** Parse Markdown into simple blocks both renderers share. */
export function blocks(md: unknown): Block[] {
  const lines = String(md || "").replace(/\r\n?/g, "\n").split("\n");
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (/^\s*```/.test(line)) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) body.push(lines[i++]);
      i++;
      out.push({ t: "code", text: body.join("\n") });
      continue;
    }
    if (!line.trim()) {
      out.push({ t: "blank" });
      i++;
      continue;
    }
    let m: RegExpMatchArray | null;
    if ((m = line.match(/^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/))) {
      out.push({ t: "h", level: m[1].length, text: m[2] });
      i++;
      continue;
    }
    if (/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      out.push({ t: "hr" });
      i++;
      continue;
    }
    if (/^\s*\|.+\|\s*$/.test(line)) {
      const rows: string[][] = [];
      while (i < lines.length && /^\s*\|.+\|\s*$/.test(lines[i])) {
        const cells = lines[i].trim().replace(/^\||\|$/g, "").split("|").map((c) => c.trim());
        if (!cells.every((c) => /^:?-{2,}:?$/.test(c))) rows.push(cells);
        i++;
      }
      out.push({ t: "table", rows });
      continue;
    }
    if ((m = line.match(/^(\s*)([-*+]|\d+[.)])\s+(.*)$/))) {
      const ordered = /\d/.test(m[2]);
      const items: ListItem[] = [];
      while (i < lines.length && (m = lines[i].match(/^(\s*)([-*+]|\d+[.)])\s+(.*)$/)) && /\d/.test(m[2]) === ordered) {
        items.push({ depth: Math.min(3, Math.floor(m[1].replace(/\t/g, "    ").length / 2)), text: m[3] });
        i++;
      }
      out.push({ t: ordered ? "ol" : "ul", items });
      continue;
    }
    if ((m = line.match(/^\s*>\s?(.*)$/))) {
      const body: string[] = [];
      while (i < lines.length && (m = lines[i].match(/^\s*>\s?(.*)$/))) {
        body.push(m[1]);
        i++;
      }
      out.push({ t: "quote", text: body.join("\n") });
      continue;
    }
    const para: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() &&
      !/^\s*(```|#{1,6}\s|[-*+]\s|\d+[.)]\s|>|\|)/.test(lines[i]) &&
      !/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(lines[i])
    ) {
      para.push(lines[i++]);
    }
    if (!para.length) para.push(lines[i++]);
    out.push({ t: "p", lines: para });
  }
  return out;
}

/** Clean plain text: what a person would type into a plain field. */
export function toPlain(md: unknown): string {
  const parts: string[] = [];
  for (const b of blocks(md)) {
    if (b.t === "blank") {
      if (parts.length && parts[parts.length - 1] !== "") parts.push("");
      continue;
    }
    if (b.t === "h") parts.push(inlinePlain(b.text));
    else if (b.t === "hr") {
      if (parts.length && parts[parts.length - 1] !== "") parts.push("");
    } else if (b.t === "code") parts.push(b.text);
    else if (b.t === "quote") parts.push(...b.text.split("\n").map((l) => `“${inlinePlain(l)}”`.replace(/^“”$/, "")));
    else if (b.t === "ul") parts.push(...b.items.map((it) => `${"  ".repeat(it.depth)}• ${inlinePlain(it.text)}`));
    else if (b.t === "ol") {
      let n = 0;
      parts.push(...b.items.map((it) => `${"  ".repeat(it.depth)}${it.depth ? "-" : `${++n}.`} ${inlinePlain(it.text)}`));
    } else if (b.t === "table") parts.push(...b.rows.map((r) => r.map(inlinePlain).join(" | ")));
    else if (b.t === "p") parts.push(...b.lines.map(inlinePlain));
  }
  return parts.join("\n").replace(/\n{3,}/g, "\n\n").trim();
}

/** Simple, editor-friendly HTML (b/i/a/ul/ol/p/br) - what rich editors accept on paste. */
export function toHtml(md: unknown): string {
  const html: string[] = [];
  for (const b of blocks(md)) {
    if (b.t === "blank" || b.t === "hr") continue;
    if (b.t === "h") html.push(`<p><b>${inlineHtml(b.text)}</b></p>`);
    else if (b.t === "code") html.push(`<pre>${esc(b.text)}</pre>`);
    else if (b.t === "quote") html.push(`<blockquote>${b.text.split("\n").map(inlineHtml).join("<br>")}</blockquote>`);
    else if (b.t === "ul" || b.t === "ol") html.push(`<${b.t}>${b.items.map((it) => `<li>${inlineHtml(it.text)}</li>`).join("")}</${b.t}>`);
    else if (b.t === "table") html.push(`<p>${b.rows.map((r) => r.map(inlineHtml).join(" | ")).join("<br>")}</p>`);
    else if (b.t === "p") html.push(`<p>${b.lines.map(inlineHtml).join("<br>")}</p>`);
  }
  return html.join("");
}

// Sites whose text boxes ARE Markdown: converting there would lose formatting.
const MARKDOWN_HOSTS =
  /(^|\.)(github\.com|gitlab\.com|bitbucket\.org|discord\.com|stackoverflow\.com|stackexchange\.com|superuser\.com|serverfault\.com|askubuntu\.com|dev\.to|hackmd\.io|obsidian\.md|linear\.app)$/i;
export function hostSpeaksMarkdown(host: unknown): boolean {
  return MARKDOWN_HOSTS.test(String(host || ""));
}
