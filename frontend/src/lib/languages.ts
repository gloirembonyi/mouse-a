import type { Extension } from "@codemirror/state";
import { javascript } from "@codemirror/lang-javascript";
import { python } from "@codemirror/lang-python";
import { markdown } from "@codemirror/lang-markdown";
import { json } from "@codemirror/lang-json";
import { rust } from "@codemirror/lang-rust";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";

// Maps a fenced-code-block language tag (or a file extension) to its
// CodeMirror language extension. Shared by chat code blocks and the file
// editor pane so both highlight consistently.
const byLang: Record<string, () => Extension> = {
  js: () => javascript(),
  javascript: () => javascript(),
  jsx: () => javascript({ jsx: true }),
  ts: () => javascript({ typescript: true }),
  typescript: () => javascript({ typescript: true }),
  tsx: () => javascript({ typescript: true, jsx: true }),
  py: () => python(),
  python: () => python(),
  md: () => markdown(),
  markdown: () => markdown(),
  json: () => json(),
  rs: () => rust(),
  rust: () => rust(),
  html: () => html(),
  css: () => css(),
};

const byExt: Record<string, keyof typeof byLang> = {
  js: "js",
  mjs: "js",
  cjs: "js",
  jsx: "jsx",
  ts: "ts",
  tsx: "tsx",
  py: "py",
  md: "md",
  markdown: "md",
  json: "json",
  rs: "rust",
  html: "html",
  htm: "html",
  css: "css",
};

export function languageFor(langOrExt?: string): Extension[] {
  if (!langOrExt) return [];
  const key = langOrExt.toLowerCase().replace(/^\./, "");
  const factory = byLang[key] ?? byLang[byExt[key]];
  return factory ? [factory()] : [];
}

export function languageForPath(path: string): Extension[] {
  const ext = path.split(".").pop() ?? "";
  return languageFor(ext);
}
