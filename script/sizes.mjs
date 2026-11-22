#!/usr/bin/env node
// No Rust source file grows past LIMIT lines of code (blank and comment lines aside). Those that
// already are past it are listed in sizes.json with the most they may have: they may shrink, not
// grow. `--lower` writes the list anew from what the files have now, never raising an entry.
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const LIMIT = 600;
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const list = join(root, "script", "sizes.json");

/** @param {string} dir @returns {string[]} */
const sources = dir =>
  readdirSync(dir, { withFileTypes: true }).flatMap(e => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return sources(path);
    return e.name.endsWith(".rs") ? [path] : [];
  });
const code = (/** @type {string} */ path) =>
  readFileSync(path, "utf8")
    .split("\n")
    .filter(l => l.trim() && !l.trim().startsWith("//")).length;

/** @type {Record<string, number>} */
const allowed = JSON.parse(readFileSync(list, "utf8"));
const sizes = Object.fromEntries(sources(join(root, "src")).map(p => [relative(root, p), code(p)]));
if (process.argv.includes("--lower")) {
  const lowered = Object.fromEntries(
    Object.entries(allowed)
      .filter(([path]) => sizes[path] > LIMIT)
      .map(([path, most]) => [path, Math.min(most, sizes[path])])
      .sort(),
  );
  writeFileSync(list, `${JSON.stringify(lowered, null, 2)}\n`);
  process.exit(0);
}
const grown = Object.entries(sizes).filter(([path, lines]) => lines > (allowed[path] ?? LIMIT));
for (const [path, lines] of grown)
  console.error(`${path}: ${lines} lines of code, at most ${allowed[path] ?? LIMIT}: split it`);
process.exit(grown.length ? 1 : 0);
