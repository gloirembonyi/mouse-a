// Markdown -> a real Word document (.docx), no dependencies. A TypeScript port of the
// extension's shared/docx.js (GhostDocx) - keep the two behaving identically.
//
// The agent writes documents (resumes, reports, letters) as Markdown. Handing a user a
// .md file means handing them "**" and "#" - so a Markdown file card's main download is
// a proper .docx: real headings, bold/italic, bullet and numbered lists, tables, links,
// code. Opens in Word, Google Docs, LibreOffice and Pages.
//
//   docxFromMarkdown(md, { title }) -> Uint8Array (a .docx zip)
//
// The zip is written "stored" (no compression) with CRC-32s - small documents, no
// inflate library needed.
import { blocks } from "./textFormat";

// ── ZIP (stored) ────────────────────────────────────────────────────────
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

export function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) c = CRC_TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

export interface ZipEntry {
  name: string;
  data: string | Uint8Array;
}

export function zip(files: ZipEntry[]): Uint8Array {
  const enc = new TextEncoder();
  const chunks: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  const u16 = (v: number) => [v & 0xff, (v >>> 8) & 0xff];
  const u32 = (v: number) => [v & 0xff, (v >>> 8) & 0xff, (v >>> 16) & 0xff, (v >>> 24) & 0xff];
  for (const f of files) {
    const name = enc.encode(f.name);
    const data = typeof f.data === "string" ? enc.encode(f.data) : f.data;
    const crc = crc32(data);
    const local = new Uint8Array([
      ...u32(0x04034b50), ...u16(20), ...u16(0x0800), ...u16(0), ...u16(0), ...u16(0x21),
      ...u32(crc), ...u32(data.length), ...u32(data.length), ...u16(name.length), ...u16(0),
    ]);
    chunks.push(local, name, data);
    central.push(
      new Uint8Array([
        ...u32(0x02014b50), ...u16(20), ...u16(20), ...u16(0x0800), ...u16(0), ...u16(0), ...u16(0x21),
        ...u32(crc), ...u32(data.length), ...u32(data.length), ...u16(name.length), ...u16(0), ...u16(0),
        ...u16(0), ...u16(0), ...u32(0), ...u32(offset),
      ]),
      name,
    );
    offset += local.length + name.length + data.length;
  }
  const centralSize = central.reduce((n, c) => n + c.length, 0);
  const end = new Uint8Array([
    ...u32(0x06054b50), ...u16(0), ...u16(0), ...u16(files.length), ...u16(files.length),
    ...u32(centralSize), ...u32(offset), ...u16(0),
  ]);
  const all = [...chunks, ...central, end];
  const out = new Uint8Array(all.reduce((n, c) => n + c.length, 0));
  let p = 0;
  for (const c of all) {
    out.set(c, p);
    p += c.length;
  }
  return out;
}

// ── WordprocessingML ────────────────────────────────────────────────────
const XML_ESC: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };
const x = (s: unknown) =>
  String(s ?? "")
    .replace(/[&<>"]/g, (c) => XML_ESC[c])
    // Characters XML 1.0 forbids would make Word call the file corrupt.
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F]/g, "");

export interface Run {
  text: string;
  b?: boolean;
  i?: boolean;
  code?: boolean;
  s?: boolean;
  link?: string;
}

/** Split inline Markdown into styled runs: [{text, b, i, code, s, link}] */
export function inlineRuns(text: unknown): Run[] {
  const runs: Run[] = [];
  const re =
    /(\*\*\*[^*]+\*\*\*|\*\*[^*]+\*\*|__[^_]+__|~~[^~]+~~|`[^`]+`|\[[^\]]+\]\([^)\s]+\)|(?:^|(?<=[\s(]))\*[^*\s][^*\n]*?\*(?=[\s).,!?:;]|$)|(?:^|(?<=[\s(]))_[^_\s][^_\n]*?_(?=[\s).,!?:;]|$))/g;
  let last = 0;
  let m: RegExpExecArray | null;
  const s = String(text || "");
  while ((m = re.exec(s))) {
    if (m.index > last) runs.push({ text: s.slice(last, m.index) });
    const t = m[0];
    if (t.startsWith("***")) runs.push({ text: t.slice(3, -3), b: true, i: true });
    else if (t.startsWith("**") || t.startsWith("__")) runs.push({ text: t.slice(2, -2), b: true });
    else if (t.startsWith("~~")) runs.push({ text: t.slice(2, -2), s: true });
    else if (t.startsWith("`")) runs.push({ text: t.slice(1, -1), code: true });
    else if (t.startsWith("[")) {
      const lm = t.match(/^\[([^\]]+)\]\(([^)\s]+)\)$/)!;
      runs.push({ text: lm[1], link: lm[2] });
    } else runs.push({ text: t.slice(1, -1), i: true });
    last = m.index + t.length;
  }
  if (last < s.length) runs.push({ text: s.slice(last) });
  return runs.filter((r) => r.text);
}

export function docxFromMarkdown(md: string, { title }: { title?: string } = {}): Uint8Array {
  const links: string[] = [];
  const lists: number[] = []; // numIds of ordered lists (each restarts at 1)
  const bulletNumId = 1;

  const runXml = (r: Run, extra = "") => {
    const props = [
      r.b ? "<w:b/>" : "",
      r.i ? "<w:i/>" : "",
      r.s ? "<w:strike/>" : "",
      r.code ? '<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:shd w:val="clear" w:color="auto" w:fill="F1F3F5"/>' : "",
      r.link ? '<w:rStyle w:val="Hyperlink"/>' : "",
      extra,
    ].join("");
    const t = `<w:r>${props ? `<w:rPr>${props}</w:rPr>` : ""}<w:t xml:space="preserve">${x(r.text)}</w:t></w:r>`;
    if (r.link && /^https?:|^mailto:/i.test(r.link)) {
      links.push(r.link);
      return `<w:hyperlink r:id="rIdL${links.length}" w:history="1">${t}</w:hyperlink>`;
    }
    return t;
  };
  const runsXml = (text: string, extra?: string) => inlineRuns(text).map((r) => runXml(r, extra)).join("");
  const para = (inner: string, pPr = "") => `<w:p>${pPr ? `<w:pPr>${pPr}</w:pPr>` : ""}${inner}</w:p>`;

  const body: string[] = [];
  let first = true;
  for (const b of blocks(md)) {
    if (b.t === "blank") continue;
    if (b.t === "h") {
      // The first top-level heading of a document is its title.
      const style = first && b.level === 1 ? "Title" : `Heading${Math.min(b.level, 3)}`;
      body.push(para(runsXml(b.text), `<w:pStyle w:val="${style}"/>`));
    } else if (b.t === "p") {
      body.push(para(b.lines.map((l, i) => (i ? "<w:r><w:br/></w:r>" : "") + runsXml(l)).join("")));
    } else if (b.t === "ul" || b.t === "ol") {
      let numId = bulletNumId;
      if (b.t === "ol") {
        numId = 100 + lists.length;
        lists.push(numId);
      }
      for (const it of b.items) {
        body.push(para(runsXml(it.text), `<w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="${it.depth}"/><w:numId w:val="${numId}"/></w:numPr>`));
      }
    } else if (b.t === "quote") {
      body.push(para(b.text.split("\n").map((l, i) => (i ? "<w:r><w:br/></w:r>" : "") + runsXml(l)).join(""), '<w:pStyle w:val="Quote"/>'));
    } else if (b.t === "code") {
      body.push(
        para(
          b.text.split("\n").map((l, i) => (i ? "<w:r><w:br/></w:r>" : "") + `<w:r><w:t xml:space="preserve">${x(l)}</w:t></w:r>`).join(""),
          '<w:pStyle w:val="Code"/>',
        ),
      );
    } else if (b.t === "hr") {
      body.push(para("", '<w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="C9D2DE"/></w:pBdr>'));
    } else if (b.t === "table" && b.rows.length) {
      const cols = Math.max(...b.rows.map((r) => r.length));
      const rows = b.rows
        .map(
          (r, ri) =>
            `<w:tr>${Array.from(
              { length: cols },
              (_, ci) =>
                `<w:tc><w:tcPr>${ri === 0 ? '<w:shd w:val="clear" w:color="auto" w:fill="EEF2F7"/>' : ""}</w:tcPr>` +
                para(runsXml(r[ci] ?? "", ri === 0 ? "<w:b/>" : ""), '<w:spacing w:before="40" w:after="40"/>') +
                "</w:tc>",
            ).join("")}</w:tr>`,
        )
        .join("");
      body.push(
        `<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="5000" w:type="pct"/>` +
          `<w:tblBorders>${["top", "left", "bottom", "right", "insideH", "insideV"].map((s) => `<w:${s} w:val="single" w:sz="4" w:space="0" w:color="C9D2DE"/>`).join("")}</w:tblBorders>` +
          `<w:tblCellMar><w:left w:w="100" w:type="dxa"/><w:right w:w="100" w:type="dxa"/></w:tblCellMar></w:tblPr>${rows}</w:tbl>`,
      );
      body.push(para(""));
    }
    first = false;
  }
  if (!body.length) body.push(para(""));

  const NS =
    'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"';
  const documentXml = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document ${NS}><w:body>${body.join("")}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1080" w:right="1080" w:bottom="1080" w:left="1080" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>`;

  const lvl = (ilvl: number, fmt: string, text: string, font?: string) =>
    `<w:lvl w:ilvl="${ilvl}"><w:start w:val="1"/><w:numFmt w:val="${fmt}"/><w:lvlText w:val="${text}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="${360 * (ilvl + 2)}" w:hanging="360"/></w:pPr>${font ? `<w:rPr><w:rFonts w:ascii="${font}" w:hAnsi="${font}" w:hint="default"/></w:rPr>` : ""}</w:lvl>`;
  const numberingXml = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering ${NS}>
<w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/>${lvl(0, "bullet", "•", "Calibri")}${lvl(1, "bullet", "◦", "Calibri")}${lvl(2, "bullet", "▪", "Calibri")}${lvl(3, "bullet", "•", "Calibri")}</w:abstractNum>
<w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="hybridMultilevel"/>${lvl(0, "decimal", "%1.")}${lvl(1, "lowerLetter", "%2.")}${lvl(2, "lowerRoman", "%3.")}${lvl(3, "decimal", "%4.")}</w:abstractNum>
<w:num w:numId="${bulletNumId}"><w:abstractNumId w:val="0"/></w:num>
${lists.map((id) => `<w:num w:numId="${id}"><w:abstractNumId w:val="1"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num>`).join("")}
</w:numbering>`;

  const style = (id: string, name: string, pPr: string, rPr: string, extra = "") =>
    `<w:style w:type="paragraph" w:styleId="${id}"><w:name w:val="${name}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/>${extra}<w:qFormat/><w:pPr>${pPr}</w:pPr><w:rPr>${rPr}</w:rPr></w:style>`;
  const stylesXml = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles ${NS}>
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:color w:val="1F2933"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault>
<w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>
${style("Title", "Title", '<w:spacing w:after="200"/><w:pBdr><w:bottom w:val="single" w:sz="8" w:space="6" w:color="3D6A9E"/></w:pBdr>', '<w:b/><w:sz w:val="44"/><w:szCs w:val="44"/><w:color w:val="14213D"/>')}
${style("Heading1", "heading 1", '<w:keepNext/><w:spacing w:before="320" w:after="120"/><w:outlineLvl w:val="0"/>', '<w:b/><w:sz w:val="32"/><w:szCs w:val="32"/><w:color w:val="14213D"/>')}
${style("Heading2", "heading 2", '<w:keepNext/><w:spacing w:before="260" w:after="100"/><w:pBdr><w:bottom w:val="single" w:sz="4" w:space="2" w:color="C9D2DE"/></w:pBdr><w:outlineLvl w:val="1"/>', '<w:b/><w:sz w:val="26"/><w:szCs w:val="26"/><w:color w:val="3D6A9E"/>')}
${style("Heading3", "heading 3", '<w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="2"/>', '<w:b/><w:sz w:val="23"/><w:szCs w:val="23"/><w:color w:val="1F2933"/>')}
${style("ListParagraph", "List Paragraph", '<w:spacing w:after="60"/><w:contextualSpacing/>', "")}
${style("Quote", "Quote", '<w:ind w:left="567"/><w:pBdr><w:left w:val="single" w:sz="18" w:space="8" w:color="9FB6D3"/></w:pBdr>', '<w:i/><w:color w:val="52606D"/>')}
${style("Code", "Code", '<w:spacing w:after="120" w:line="240" w:lineRule="auto"/><w:shd w:val="clear" w:color="auto" w:fill="F1F3F5"/>', '<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="19"/>')}
<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="1F5FAD"/><w:u w:val="single"/></w:rPr></w:style>
<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="C9D2DE"/></w:tblBorders></w:tblPr></w:style>
</w:styles>`;

  const docRels = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
<Relationship Id="rIdNumbering" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
${links.map((u, i) => `<Relationship Id="rIdL${i + 1}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="${x(u)}" TargetMode="External"/>`).join("\n")}
</Relationships>`;

  const now = new Date().toISOString().replace(/\.\d+Z$/, "Z");
  return zip([
    {
      name: "[Content_Types].xml",
      data: `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
<Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>`,
    },
    {
      name: "_rels/.rels",
      data: `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>`,
    },
    {
      name: "docProps/core.xml",
      data: `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<dc:title>${x(title || "Document")}</dc:title><dc:creator>Kudu</dc:creator>
<dcterms:created xsi:type="dcterms:W3CDTF">${now}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">${now}</dcterms:modified>
</cp:coreProperties>`,
    },
    { name: "word/document.xml", data: documentXml },
    { name: "word/styles.xml", data: stylesXml },
    { name: "word/numbering.xml", data: numberingXml },
    { name: "word/_rels/document.xml.rels", data: docRels },
  ]);
}
