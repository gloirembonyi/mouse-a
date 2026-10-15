#!/usr/bin/env node
// Sets the app version everywhere it is declared, so a tag, the installer file names,
// the About screen and Cargo agree. Used by scripts/release.mjs and by the Release
// workflow; safe to run alone: `node scripts/bump-version.mjs 0.2.0`.
import fs from "node:fs";
import path from "node:path";
import { execSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const version = (process.argv[2] || "").trim().replace(/^v/, "");
const SEMVER = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;
if (!SEMVER.test(version)) {
  console.error(`usage: node scripts/bump-version.mjs <semver>   (got "${version || ""}")`);
  process.exit(2);
}

const edits = [];
function editJson(rel) {
  const p = path.join(root, rel);
  const j = JSON.parse(fs.readFileSync(p, "utf8"));
  if (j.version === version) return;
  j.version = version;
  fs.writeFileSync(p, JSON.stringify(j, null, 2) + "\n");
  edits.push(rel);
}
editJson("src-tauri/tauri.conf.json");
editJson("package.json");

const cargoPath = path.join(root, "src-tauri/Cargo.toml");
const cargo = fs.readFileSync(cargoPath, "utf8");
// Only the [package] version (the first `version = "..."` line), never a dependency's.
const bumped = cargo.replace(/^(\[package\][\s\S]*?^version\s*=\s*")[^"]*(")/m, `$1${version}$2`);
if (bumped !== cargo) {
  fs.writeFileSync(cargoPath, bumped);
  edits.push("src-tauri/Cargo.toml");
  // Cargo.lock records the package's own version; refresh just that entry so the CI
  // build does not have to touch the lock file (and `--locked` builds keep working).
  try {
    execSync("cargo update --workspace --offline", { cwd: path.join(root, "src-tauri"), stdio: "pipe" });
    edits.push("src-tauri/Cargo.lock");
  } catch (e) {
    console.warn("cargo update --workspace failed (is cargo installed?); Cargo.lock will update on the next build:", String(e.stderr || e.message).trim().split("\n").pop());
  }
}

if (!edits.length) console.log(`already at ${version}`);
else console.log(`version ${version} -> ${edits.join(", ")}`);
