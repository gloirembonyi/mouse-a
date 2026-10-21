#!/usr/bin/env node
// Short, human "What's new" notes for a release, written from the commits since the last one.
//
//   node scripts/release-notes.mjs <previous-tag|""> <out-file>
//
// The output is a Markdown block between <!--kudu-whats-new--> markers with "New",
// "Improved" and "Fixed" lists. CI puts it at the top of the GitHub release (the website's
// /whats-new page and the in-app updater both read it) and into latest.json, so what users
// see is always this, never a raw commit or pull-request list.
//
// How a commit becomes a line:
//   feat(...)  → New       fix(...)  → Fixed       perf(...) → Improved
//   everything else (chore, docs, ci, build, test, refactor, style, release) is internal and skipped.
//   Write the subject in plain words - it is shown as written, minus "type(scope):".
//   Want different wording for users? Add a line to the commit body:  Notes: <what users should read>
//   and that sentence is used instead (several "Notes:" lines are fine).
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";

const [prev = "", out = "whats-new.md"] = process.argv.slice(2);

const GROUPS = { feat: "New", perf: "Improved", fix: "Fixed" };
const ORDER = ["New", "Improved", "Fixed"];
// Commits that are about the repo, not the product.
const INTERNAL = /(injected commit|obfuscated|payload files|^merge |^revert |chore\(release\)|bump version|\[skip ci\])/i;

function git(args) {
  try {
    return execFileSync("git", args, { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  } catch {
    return "";
  }
}

const range = prev ? [`${prev}..HEAD`] : ["-n", "40"];
const raw = git(["log", "--no-merges", "--format=%s%x1f%b%x1e", ...range]);

const clean = (s) => {
  let t = s.trim().replace(/\s+/g, " ").replace(/\.$/, "");
  if (!t) return "";
  t = t.charAt(0).toUpperCase() + t.slice(1);
  return t.length > 160 ? t.slice(0, 157).trimEnd() + "…" : t;
};

const groups = { New: [], Improved: [], Fixed: [] };
const seen = new Set();
const add = (group, text) => {
  const line = clean(text);
  const key = line.toLowerCase();
  if (!line || seen.has(key)) return;
  seen.add(key);
  groups[group].push(line);
};

for (const record of raw.split("\x1e")) {
  const [subject = "", body = ""] = record.replace(/^\n/, "").split("\x1f");
  const s = subject.trim();
  if (!s || INTERNAL.test(s)) continue;
  const m = s.match(/^(\w+)(?:\([^)]*\))?(!)?:\s*(.+)$/);
  if (!m) continue; // not a conventional commit: not safe to show users
  const group = GROUPS[m[1].toLowerCase()];
  if (!group) continue;
  const custom = [...body.matchAll(/^Notes?:\s*(.+)$/gim)].map((x) => x[1]);
  if (custom.length) custom.forEach((c) => add(group, c));
  else add(group, m[3]);
}

if (!ORDER.some((g) => groups[g].length)) groups.Improved.push("Behind-the-scenes improvements and fixes");

let md = "<!--kudu-whats-new-->\n";
for (const g of ORDER) {
  if (!groups[g].length) continue;
  md += `### ${g}\n${groups[g].slice(0, 12).map((l) => `- ${l}`).join("\n")}\n\n`;
}
md += "<!--/kudu-whats-new-->\n";

writeFileSync(out, md);
console.log(md);
