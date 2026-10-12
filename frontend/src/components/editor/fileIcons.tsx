import { File, FileCode2, FileImage, FileJson, FileText, Settings2 } from "lucide-react";

// Color-coded file glyphs (the way VS Code / Codex tint by language) so a
// tree or tab strip is scannable without reading every extension.
const CODE: Record<string, string> = {
  ts: "#3b82f6",
  tsx: "#38bdf8",
  js: "#eab308",
  jsx: "#facc15",
  mjs: "#eab308",
  cjs: "#eab308",
  rs: "#f97316",
  py: "#60a5fa",
  go: "#22d3ee",
  java: "#ef4444",
  cs: "#a855f7",
  html: "#f97316",
  css: "#6366f1",
  scss: "#ec4899",
  vue: "#22c55e",
  svelte: "#f97316",
  sh: "#22c55e",
  ps1: "#60a5fa",
  sql: "#14b8a6",
};

export function FileIcon({ name, size = 14 }: { name: string; size?: number }) {
  const lower = name.toLowerCase();
  const ext = lower.includes(".") ? lower.split(".").pop()! : "";
  if (lower.startsWith(".env") || ["toml", "yaml", "yml", "ini", "lock", "config"].includes(ext) || lower.startsWith(".")) {
    return <Settings2 size={size} className="flex-shrink-0 text-text-muted" />;
  }
  if (ext === "json") return <FileJson size={size} className="flex-shrink-0" style={{ color: "#eab308" }} />;
  if (["md", "mdx", "txt", "rst"].includes(ext)) return <FileText size={size} className="flex-shrink-0 text-accent" />;
  if (["png", "jpg", "jpeg", "gif", "svg", "webp", "ico"].includes(ext))
    return <FileImage size={size} className="flex-shrink-0" style={{ color: "#a855f7" }} />;
  if (CODE[ext]) return <FileCode2 size={size} className="flex-shrink-0" style={{ color: CODE[ext] }} />;
  return <File size={size} className="flex-shrink-0 text-text-muted" />;
}
