// Files the browser agent (Kudu Connect) hands back with its answer. Shared contract
// with the extension - do not change it:
//
//   ```kudu-file
//   {"name":"x.md","path":"x.md","mime":"text/markdown","bytes":5123,"b64":"<base64 UTF-8>"}
//   ```
//
// One line of JSON per fence; `b64` is omitted (and `truncated: true` set) for files
// over 256 KB. The Rust side (src-tauri/src/kudu_files.rs) saves the same blocks when
// they arrive through delegate_to_browser.
import { save } from "@tauri-apps/plugin-dialog";
import { fsOpenDefault, fsWriteBase64, kuduFileSave } from "./tauri";
import { docxFromMarkdown } from "./docx";
import { toPlain } from "./textFormat";

export interface KuduFile {
  name: string;
  path?: string;
  mime?: string;
  bytes?: number;
  b64?: string;
  truncated?: boolean;
}

/** The fence's body → a file, or null when it is not a usable block (the caller
 *  then shows it as a plain code block). */
export function parseKuduFile(body: string): KuduFile | null {
  try {
    const v = JSON.parse(body.trim());
    if (!v || typeof v !== "object") return null;
    const name = typeof v.name === "string" && v.name.trim() ? v.name : typeof v.path === "string" ? v.path : "";
    if (!name.trim()) return null;
    if (v.b64 != null && typeof v.b64 !== "string") return null;
    return {
      name: baseName(name),
      path: typeof v.path === "string" ? v.path : undefined,
      mime: typeof v.mime === "string" ? v.mime : undefined,
      bytes: typeof v.bytes === "number" ? v.bytes : undefined,
      b64: typeof v.b64 === "string" && v.b64.trim() ? v.b64.replace(/\s+/g, "") : undefined,
      truncated: v.truncated === true,
    };
  } catch {
    return null;
  }
}

function baseName(p: string) {
  return p.split(/[\\/]/).pop()?.trim() || p;
}

export function extOf(name: string) {
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(i + 1).toLowerCase() : "";
}

export function formatBytes(n: number | undefined) {
  if (n == null || !Number.isFinite(n)) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function decodeBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function decodeText(b64: string): string {
  return new TextDecoder("utf-8").decode(decodeBytes(b64));
}

/** UTF-8 text → base64 (for saving generated text, e.g. a diagram's SVG). */
export function encodeText(text: string): string {
  return encodeBytes(new TextEncoder().encode(text));
}

export function encodeBytes(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

export type PreviewKind = "markdown" | "table" | "json" | "mermaid" | "svg" | "image" | "html" | "text";

export function previewKind(file: KuduFile): PreviewKind {
  const ext = extOf(file.name);
  const mime = (file.mime ?? "").toLowerCase();
  if (["md", "markdown", "mdx"].includes(ext) || mime === "text/markdown") return "markdown";
  if (["csv", "tsv", "tab"].includes(ext) || mime === "text/csv" || mime === "text/tab-separated-values") return "table";
  if (ext === "json" || mime === "application/json") return "json";
  if (["mmd", "mermaid"].includes(ext) || mime === "text/vnd.mermaid") return "mermaid";
  if (ext === "svg" || mime === "image/svg+xml") return "svg";
  if (["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"].includes(ext) || mime.startsWith("image/")) return "image";
  if (["html", "htm"].includes(ext) || mime === "text/html") return "html";
  return "text";
}

/** What a card can save. A Markdown document is never handed over as raw Markdown by
 *  default ("**" and "#" mean nothing in Word or an email): "docx" is its main format,
 *  "md" and "txt" (toPlain) are the alternatives. Other files: "original" only. */
export type SaveFormat = "original" | "docx" | "md" | "txt";

function stem(name: string) {
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(0, i) : name;
}

/** The file as it will be written in `format` (name + base64 bytes). */
export function asFormat(file: KuduFile, format: SaveFormat): { name: string; b64: string } {
  if (!file.b64) throw new Error("This file was too large to transfer.");
  if (format === "original" || format === "md") return { name: file.name, b64: file.b64 };
  const text = decodeText(file.b64);
  if (format === "txt") return { name: `${stem(file.name)}.txt`, b64: encodeText(toPlain(text)) };
  return { name: `${stem(file.name)}.docx`, b64: encodeBytes(docxFromMarkdown(text, { title: stem(file.name) })) };
}

/** What Download / Open use when nothing else is picked: .docx for Markdown documents. */
export function defaultFormat(file: KuduFile): SaveFormat {
  return previewKind(file) === "markdown" ? "docx" : "original";
}

/** Download: the native save dialog (default name = the file's name in that format).
 *  Returns the saved path, or null when the user cancelled. Without a dialog it lands
 *  in Documents\Kudu. */
export async function downloadKuduFile(file: KuduFile, format: SaveFormat = defaultFormat(file)): Promise<string | null> {
  const out = asFormat(file, format);
  let target: string | null;
  try {
    target = await save({ defaultPath: out.name, title: "Save file" });
  } catch {
    return kuduFileSave(out.name, out.b64, null);
  }
  if (!target) return null;
  await fsWriteBase64(target, out.b64);
  return target;
}

// Open = save once into Documents\Kudu, then the OS default app - a Markdown document
// opens as its .docx (in Word), never as raw .md. The saved path is remembered per
// file so opening twice doesn't make "-1", "-2" copies.
const opened = new Map<string, string>();

export async function openKuduFile(file: KuduFile): Promise<string> {
  const out = asFormat(file, defaultFormat(file));
  const key = `${out.name}:${file.b64!.length}:${file.b64!.slice(0, 64)}:${file.b64!.slice(-64)}`;
  let path = opened.get(key);
  if (!path) {
    path = await kuduFileSave(out.name, out.b64, null);
    opened.set(key, path);
  }
  await fsOpenDefault(path);
  return path;
}
