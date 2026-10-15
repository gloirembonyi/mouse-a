#!/usr/bin/env node
// Cut a release from a laptop:
//
//   npm run release -- 0.2.0            # bump, commit, tag v0.2.0, push
//   npm run release -- 0.2.0 --dry-run  # show what would happen
//
// Pushing the tag starts .github/workflows/build.yml, which builds Windows, macOS and
// Linux and publishes the GitHub release with all installers once every platform has
// finished. Follow it at https://github.com/<owner>/<repo>/actions.
import { execSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const dry = args.includes("--dry-run");
const version = (args.find((a) => !a.startsWith("--")) || "").replace(/^v/, "");
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error("usage: npm run release -- <semver> [--dry-run]");
  process.exit(2);
}
const sh = (cmd, opts = {}) => execSync(cmd, { cwd: root, stdio: "pipe", encoding: "utf8", ...opts }).trim();
const run = (cmd) => { console.log(`$ ${cmd}`); if (!dry) execSync(cmd, { cwd: root, stdio: "inherit" }); };

const branch = sh("git rev-parse --abbrev-ref HEAD");
if (branch !== "main") { console.error(`releases are cut from main (you are on ${branch})`); process.exit(1); }
if (sh("git status --porcelain")) { console.error("working tree is not clean - commit or stash first"); process.exit(1); }
sh("git fetch origin main --tags");
if (sh("git rev-list HEAD..origin/main --count") !== "0") { console.error("main is behind origin/main - git pull first"); process.exit(1); }
if (sh(`git tag -l v${version}`)) { console.error(`tag v${version} already exists`); process.exit(1); }

run(`node scripts/bump-version.mjs ${version}`);
// The version files may already say this version (first release, or a re-cut after a
// failed build); then there is nothing to commit and the tag goes on HEAD.
if (dry || sh("git status --porcelain")) {
  run("git add -A");
  run(`git commit -m "chore(release): v${version}"`);
} else {
  console.log(`(version files already at ${version} - tagging HEAD)`);
}
run(`git tag -a v${version} -m "Mouse AI v${version}"`);
run("git push origin main");
run(`git push origin v${version}`);

const remote = sh("git remote get-url origin").replace(/\.git$/, "").replace(/^git@github\.com:/, "https://github.com/");
console.log(dry ? "\n(dry run - nothing changed)" : `\nBuilding v${version}: ${remote}/actions\nThe release appears at ${remote}/releases/tag/v${version} when all three platforms finish.`);
