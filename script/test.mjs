// Runs every check CI runs, one at a time or (-p) all at once, and shows how each went.
// See `script/test --help`. No dependencies: Node's own modules only.
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const CI = process.env.CI === "true";
const NPM = { file: "node_modules", hint: "npm ci" };

/**
 * Each check: its section, the command, the one that fixes what it finds (if there is one) and
 * what it needs besides the command's own program.
 * @typedef {{ section: string; cmd: string; fix?: string; env?: { [k: string]: string };
 *   needs?: { file: string; hint: string } }} Task
 * @type {{ [name: string]: Task }}
 */
const TASKS = {
  rustfmt: { section: "checks", cmd: "cargo fmt --check", fix: "cargo fmt" },
  clippy: { section: "checks", cmd: "cargo clippy --all-targets --locked --quiet -- -D warnings" },
  rustdoc: { section: "checks", cmd: "cargo doc --no-deps --locked --quiet", env: { RUSTDOCFLAGS: "-D warnings" } },
  // ESLint's fixes come before Prettier's, which has the last word on layout.
  eslint: { section: "checks", cmd: "npm run --silent lint", fix: "npm run --silent lint -- --fix", needs: NPM },
  prettier: { section: "checks", cmd: "npm run --silent format:check", fix: "npm run --silent format", needs: NPM },
  tsc: { section: "checks", cmd: "npm run --silent typecheck", needs: NPM },
  shellcheck: { section: "checks", cmd: "shellcheck bin/build bin/deploy .githooks/pre-commit script/test" },
  actionlint: { section: "checks", cmd: "actionlint" },
  zizmor: { section: "checks", cmd: "zizmor --offline --quiet .github/workflows" },
  hadolint: { section: "checks", cmd: "hadolint Dockerfile" },
  "page-tests": { section: "tests", cmd: "npm test --silent", needs: NPM },
  // Without the maps in testdata/maps (see .github/workflows/ci.yml) the BSP tests skip them.
  "cargo-test": { section: "tests", cmd: "cargo test --locked --quiet" },
  // Fetches the advisory database.
  "cargo-deny": { section: "audit", cmd: "cargo deny --locked check" },
};
const SECTIONS = [...new Set(Object.values(TASKS).map(t => t.section))];

const HELP = `Usage: script/test [options]

Runs the checks (${SECTIONS.join(", ")}) and reports how each went; exits 1 if one failed.

  -p, --parallel        run the checks at the same time instead of one after another
  -a, --fix             fix what can be fixed (${Object.keys(TASKS)
    .filter(n => TASKS[n].fix)
    .join(", ")}) instead of only checking
  -s, --section NAMES   only these sections (comma-separated)
  -t, --task NAMES      only these checks (comma-separated)
      --skip NAMES      leave these checks out (comma-separated)
  -F, --fail-fast       start no more checks after one fails
  -v, --verbose         show every check's output as it comes
  -l, --list            list the checks and their commands
  -h, --help            this help

A check whose program is not installed is skipped (\`mise install\` installs the linters), but
fails in CI.`;

const { values: opt } = parseArgs({
  options: {
    parallel: { type: "boolean", short: "p" },
    fix: { type: "boolean", short: "a" },
    section: { type: "string", short: "s" },
    task: { type: "string", short: "t" },
    skip: { type: "string" },
    "fail-fast": { type: "boolean", short: "F" },
    verbose: { type: "boolean", short: "v" },
    list: { type: "boolean", short: "l" },
    help: { type: "boolean", short: "h" },
  },
});

if (opt.help) {
  console.log(HELP);
  process.exit(0);
}
if (opt.list) {
  for (const s of SECTIONS) {
    console.log(`${s}:`);
    for (const [n, t] of Object.entries(TASKS))
      if (t.section === s)
        console.log(`  ${n.padEnd(12)} ${t.cmd}${t.fix ? `\n  ${"".padEnd(12)} fix: ${t.fix}` : ""}`);
  }
  process.exit(0);
}

/** @param {string | undefined} list @param {string[]} known @param {string} what */
function names(list, known, what) {
  if (list == null) return null;
  const given = list
    .split(",")
    .map(s => s.trim())
    .filter(Boolean);
  const unknown = given.filter(n => !known.includes(n));
  if (unknown.length) {
    console.error(`Unknown ${what}: ${unknown.join(", ")}. Known: ${known.join(", ")}`);
    process.exit(2);
  }
  return given;
}
const onlySections = names(opt.section, SECTIONS, "section");
const onlyTasks = names(opt.task, Object.keys(TASKS), "check");
const skipped = names(opt.skip, Object.keys(TASKS), "check") ?? [];
const chosen = Object.keys(TASKS).filter(
  n =>
    (!onlySections || onlySections.includes(TASKS[n].section)) &&
    (!onlyTasks || onlyTasks.includes(n)) &&
    !skipped.includes(n),
);

/**
 * A check as it runs.
 * @typedef {{ name: string; cmd: string; env?: { [k: string]: string }; missing: string | null;
 *   status: "waiting" | "running" | "passed" | "failed" | "skipped"; why?: string; output: string;
 *   start?: number; ms?: number }} Run
 */

// Why a check cannot run here (its program or node_modules is missing), or null.
/** @param {string} name @param {string} cmd */
function missing(name, cmd) {
  const needs = TASKS[name].needs;
  if (needs && !existsSync(path.join(ROOT, needs.file))) return `no ${needs.file} (${needs.hint})`;
  const program = cmd.split(" ")[0];
  const found = spawnSync("sh", ["-c", `command -v ${program} && ${program} --version`], { cwd: ROOT });
  return found.status === 0 ? null : `${program} is not installed (mise install)`;
}

/** @type {Run[]} */
const runs = chosen.map(name => {
  const t = TASKS[name],
    cmd = opt.fix && t.fix ? t.fix : t.cmd;
  return { name, cmd, env: t.env, missing: missing(name, cmd), status: "waiting", output: "" };
});

// ---- Display: a live list on a terminal, a line per finished check otherwise. ----

const FANCY = process.stdout.isTTY && !CI && !opt.verbose;
const FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
const colour = (/** @type {number} */ code) => (/** @type {string} */ s) =>
  process.stdout.isTTY && !process.env.NO_COLOR ? `\x1b[${code}m${s}\x1b[0m` : s;
const green = colour(92),
  red = colour(91),
  yellow = colour(93),
  dim = colour(2);
const width = Math.max(...runs.map(r => r.name.length));
const secs = (/** @type {number} */ ms) => `${(ms / 1000).toFixed(1)}s`;
let frame = 0,
  drawn = 0;

/** @param {Run} r */
function line(r) {
  const name = r.name.padEnd(width);
  switch (r.status) {
    case "waiting":
      return dim(`· ${name}`);
    case "running":
      return `${yellow(FRAMES[frame % FRAMES.length])} ${name} ${dim(secs(performance.now() - (r.start ?? 0)))}`;
    case "passed":
      return `${green("✓")} ${name} ${dim(secs(r.ms ?? 0))}`;
    case "failed":
      return `${red("✗")} ${name} ${dim(secs(r.ms ?? 0))}`;
    case "skipped":
      return `${yellow("–")} ${name} ${dim(r.why ?? "")}`;
  }
}
function draw() {
  if (!FANCY) return;
  frame++;
  process.stdout.write((drawn ? `\x1b[${drawn}A` : "") + runs.map(r => `\x1b[2K${line(r)}\n`).join(""));
  drawn = runs.length;
}
/** @param {Run} r */
function finished(r) {
  if (!FANCY) console.log(line(r));
}

// ---- Running ----

/** @param {Run} r @returns {Promise<void>} */
function run(r) {
  if (r.missing) {
    r.status = CI ? "failed" : "skipped";
    r.why = r.missing;
    r.output = r.missing;
    r.ms = 0;
    finished(r);
    return Promise.resolve();
  }
  r.status = "running";
  r.start = performance.now();
  return new Promise(resolve => {
    const child = spawn("sh", ["-c", r.cmd], {
      cwd: ROOT,
      env: { ...process.env, ...r.env, ...(process.stdout.isTTY ? { CARGO_TERM_COLOR: "always" } : {}) },
      stdio: ["ignore", "pipe", "pipe"],
    });
    /** @param {Buffer} chunk */
    const take = chunk => {
      r.output += chunk;
      if (opt.verbose) process.stdout.write(chunk.toString().replace(/^(?=.)/gm, dim(`[${r.name}] `)));
    };
    child.stdout.on("data", take);
    child.stderr.on("data", take);
    child.on("close", code => {
      r.ms = performance.now() - (r.start ?? 0);
      r.status = code === 0 ? "passed" : "failed";
      finished(r);
      resolve();
    });
  });
}

/** @param {Run[]} list @param {boolean} together */
async function runAll(list, together) {
  const stop = () => opt["fail-fast"] && runs.some(r => r.status === "failed");
  /** @param {Run} r */
  const next = r => {
    if (!stop()) return run(r);
    r.status = "skipped";
    r.why = "after a failure (--fail-fast)";
    finished(r);
  };
  if (together) await Promise.all(list.map(next));
  else for (const r of list) await next(r);
}

const t0 = performance.now();
if (FANCY) process.stdout.write("\x1b[?25l");
const restore = () => FANCY && process.stdout.write("\x1b[?25h");
process.on("exit", restore);
process.on("SIGINT", () => process.exit(130));
const ticker = setInterval(draw, 100);
draw();
// Fixers write files, so they go one at a time before everything else.
const fixing = runs.filter(r => opt.fix && TASKS[r.name].fix);
await runAll(fixing, false);
await runAll(
  runs.filter(r => !fixing.includes(r)),
  Boolean(opt.parallel),
);
clearInterval(ticker);
draw();

const failed = runs.filter(r => r.status === "failed");
// With --verbose the output has already been shown.
for (const r of opt.verbose ? [] : failed) {
  const rule = "─".repeat(Math.min(80, process.stdout.columns || 80));
  console.log(`\n${rule}\n${red("✗")} ${r.name}: ${r.cmd}\n${rule}\n${r.output.trimEnd()}`);
}
const count = (/** @type {Run["status"]} */ s) => runs.filter(r => r.status === s).length;
const summary = [
  green(`${count("passed")} passed`),
  failed.length && red(`${failed.length} failed`),
  count("skipped") && yellow(`${count("skipped")} skipped`),
]
  .filter(Boolean)
  .join(", ");
console.log(`\n${summary} in ${secs(performance.now() - t0)}`);
process.exit(failed.length ? 1 : 0);
