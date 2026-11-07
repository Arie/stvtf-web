// Headless Chromium watches a recording: the page, the overlay, an iframe and <stv-map>, each
// without an error. match.jsonl.gz is 25 s of cp_gullywash, players renamed. Offline: the map
// comes from testdata/maps, demos.tf's overview list is empty.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const MAP = "cp_gullywash_f9";
const BIN = path.join(ROOT, "target/debug/stvtf-web");
const WAIT = 20_000;

/** @type {import("node:child_process").ChildProcess} */
let server;
/** @type {import("playwright-core").Browser} */
let browser;
let root = "",
  base = "";
const cache = mkdtempSync(path.join(tmpdir(), "stvtf-browser-"));

const freePort = () =>
  new Promise(resolve => {
    const s = createServer().listen(0, "127.0.0.1", () => {
      const { port } = /** @type {import("node:net").AddressInfo} */ (s.address());
      s.close(() => resolve(port));
    });
  });

before(async () => {
  copyFileSync(path.join(ROOT, `testdata/maps/${MAP}.bsp`), path.join(cache, `${MAP}.bsp`));
  mkdirSync(path.join(cache, "demostf"));
  writeFileSync(path.join(cache, "demostf/boundaries.json"), "{}");
  const listen = `127.0.0.1:${await freePort()}`;
  root = `http://${listen}/`;
  base = `${root}w/demo/`;
  const args = [
    "--demo",
    path.join(ROOT, "test/browser/match.jsonl.gz"),
    "--listen",
    listen,
    "--cache",
    cache,
    "--no-warm",
  ];
  server = spawn(BIN, args, { stdio: ["ignore", "ignore", "pipe"] });
  await new Promise((resolve, reject) => {
    server.on("error", reject);
    server.on("exit", code => reject(new Error(`stvtf-web exited with ${code}`)));
    server.stderr?.on("data", d => String(d).includes("http://") && resolve(null));
  });
  browser = await chromium.launch();
});

after(async () => {
  await browser?.close();
  server?.kill();
  rmSync(cache, { recursive: true, force: true });
});

// A page whose errors fail the test, except the expected 404 for the map's overview.
async function open() {
  const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
  /** @type {string[]} */
  const errors = [];
  page.on("pageerror", e => errors.push(e.stack ?? e.message));
  page.on("console", m => {
    const at = m.location().url;
    if (m.type() === "error" && !at.includes("/overview/")) errors.push(`${m.text()} (${at})`);
  });
  return { page, errors };
}

// Waits until `fn` (run in the page) is true and returns its value.
const until = (page, fn, arg) => page.waitForFunction(fn, arg, { timeout: WAIT }).then(h => h.jsonValue());

// How many colours the map's canvas has: a few once the map is drawn, more with players on it.
const colours = page =>
  page.evaluate(() => {
    const c = document.getElementById("c");
    const data = c.getContext("2d").getImageData(0, 0, c.width, c.height).data;
    const seen = new Set();
    for (let i = 0; i < data.length; i += 4 * 97) seen.add((data[i] << 16) | (data[i + 1] << 8) | data[i + 2]);
    return seen.size;
  });

// The first message of `type` (and an event of `event`) that the host page got.
const message = (page, type, event) =>
  until(page, ([t, e]) => window.got.find(m => m.type === t && (!e || m.event?.type === e)), [type, event]);

test("the page shows the match: map, players, kills", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await until(page, m => document.getElementById("map").textContent === m, MAP);
  await until(page, () => ["blue", "red"].every(t => document.getElementById(`hud-${t}`).children.length >= 6));
  await until(page, () => document.getElementById("feed").children.length > 0);
  assert.ok((await colours(page)) > 20, "the map is drawn");
  await page.keyboard.down("Tab");
  await until(page, () => !document.getElementById("board").hidden);
  assert.match(await page.textContent("#board"), /Player \d+/, "the scoreboard lists the players");
  assert.deepEqual(errors, []);
  await page.close();
});

test("the overlay draws the match", async () => {
  const { page, errors } = await open();
  await page.goto(`${base}overlay?names=1&score=1`);
  await until(page, m => document.getElementById("map").textContent === m, MAP);
  await until(page, () => document.getElementById("empty").style.display === "none");
  assert.ok((await colours(page)) > 20, "the map is drawn");
  assert.deepEqual(errors, []);
  await page.close();
});

test("in an iframe it tells its parent what happens", async () => {
  const { page, errors } = await open();
  await page.setContent(`<script>
    window.got = [];
    addEventListener("message", e => e.data?.stvtf === 1 && got.push(e.data));
  </script><iframe src="${base}?side=0" style="width: 960px; height: 540px"></iframe>`);
  await message(page, "ready");
  assert.equal((await message(page, "map")).map, MAP);
  assert.equal((await message(page, "state")).players.length, 12);
  await message(page, "event", "kill");
  assert.deepEqual(errors, []);
  await page.close();
});

test("<stv-map> fires the messages as events", async () => {
  const { page, errors } = await open();
  await page.setContent(`<script type="module" src="${root}embed/v1/stv-map.js"></script>
    <stv-map src="${base}" side="0" style="width: 960px"></stv-map>
    <script>
      window.got = [];
      const el = document.querySelector("stv-map");
      for (const t of ["map", "state"]) el.addEventListener(t, e => got.push(e.detail));
    </script>`);
  assert.equal((await message(page, "map")).map, MAP);
  await message(page, "state");
  assert.deepEqual(errors, []);
  await page.close();
});

test("a room is driven by its owner and followed by everyone else", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, m => document.getElementById("map").textContent === m, MAP);
  await until(page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-play")).disabled));
  await page.click("#rb-play");
  await until(page, () => document.getElementById("status").textContent === "paused");
  await page.$eval("#rb-time", el => {
    el.value = "0";
    el.dispatchEvent(new Event("change"));
  });
  await until(page, () => document.getElementById("rb-clock").textContent.startsWith("0:00 /"));
  assert.equal(await page.textContent("#status"), "paused", "a seek keeps it paused");

  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(viewer.page, () => document.getElementById("status").textContent === "paused");
  assert.ok(await viewer.page.$eval("#rb-play", el => el.disabled), "only the owner controls it");
  await page.click("#rb-play");
  await until(viewer.page, () => document.getElementById("status").textContent === "live");
  assert.deepEqual([...errors, ...viewer.errors], []);
  await viewer.page.close();
  await page.close();
});
