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

// Pixels of the telestrator's yellow on the map's canvas.
const ink = page =>
  page.evaluate(() => {
    const c = document.getElementById("c");
    const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data;
    let n = 0;
    for (let i = 0; i < d.length; i += 4)
      if (d[i] > 245 && Math.abs(d[i + 1] - 210) < 8 && Math.abs(d[i + 2] - 63) < 12) n++;
    return n;
  });

test("the owner draws on the map and everyone in the room sees it", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-pen")).disabled));
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(viewer.page, m => document.getElementById("map").textContent === m, MAP);
  const clean = await ink(viewer.page);
  await page.keyboard.press("d");
  assert.equal(await page.$eval("#c", el => getComputedStyle(el).cursor), "crosshair", "the pen shows as a crosshair");
  await page.mouse.move(300, 300);
  await page.mouse.down();
  for (let i = 1; i <= 20; i++) await page.mouse.move(300 + i * 10, 300 + i * 5);
  await page.mouse.up();
  await viewer.page.waitForTimeout(500);
  assert.ok((await ink(viewer.page)) > clean + 100, "the stroke shows for the viewer");
  assert.equal(await page.$eval("#follow", el => el.hidden), true, "drawing follows nobody");
  assert.deepEqual([...errors, ...viewer.errors], []);
  await viewer.page.close();
  await page.close();
});

test("people in a room see each other and admins make others admin", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await page.fill("#myname", "coach");
  await page.press("#myname", "Enter");
  await until(page, () => document.getElementById("peoplelist").textContent.includes("coach"));
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(page, () => document.querySelectorAll("#peoplelist li").length === 2);
  assert.match(await viewer.page.textContent("#peoplelist"), /coachowner.*Guest 2you/);
  await page.click("#peoplelist button");
  await until(viewer.page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-play")).disabled));
  assert.match(await viewer.page.textContent("#peoplelist"), /Guest 2you, admin/);
  await page.$eval("#peoplelist input.ink", el => {
    el.value = "#00ff00";
    el.dispatchEvent(new Event("change"));
  });
  await until(viewer.page, () =>
    [...document.querySelectorAll("#peoplelist .ink")].some(
      el => getComputedStyle(el).backgroundColor === "rgb(0, 255, 0)",
    ),
  );
  assert.deepEqual([...errors, ...viewer.errors], []);
  await viewer.page.close();
  await page.close();
});

test("an admin has everyone follow the player they follow", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => !(/** @type {HTMLSelectElement} */ (document.getElementById("rb-view")).disabled));
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(page, () => document.querySelectorAll("#hud-blue .card").length > 0);
  await page.click("#hud-blue .card");
  const name = await until(
    page,
    () => !document.getElementById("follow").hidden && document.getElementById("follow-name").textContent,
  );
  await page.selectOption("#rb-view", "follow");
  await until(
    viewer.page,
    n => document.getElementById("follow-name").textContent === n && !document.getElementById("follow").hidden,
    name,
  );
  assert.equal(await viewer.page.$eval("#rb-view", el => el.value), "follow");
  await viewer.page.click("#follow-stop");
  assert.equal(await viewer.page.$eval("#follow", el => el.hidden), false, "a viewer cannot stop following");
  assert.deepEqual([...errors, ...viewer.errors], []);
  await viewer.page.close();
  await page.close();
});

test("another admin taking the view over keeps it", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  const other = await open();
  await other.page.goto(page.url());
  await until(page, () => document.querySelectorAll("#peoplelist button").length === 1);
  await page.click("#peoplelist button");
  await until(other.page, () => !(/** @type {HTMLSelectElement} */ (document.getElementById("rb-view")).disabled));
  await page.selectOption("#rb-view", "admin");
  await until(other.page, () => document.getElementById("rb-view").value === "admin");
  // Slow requests, as in a browser whose connections are busy: the leading admin's camera moves
  // still arrive after the other one took over.
  await page.route("**/view", r => setTimeout(() => r.continue(), 300));
  await page.mouse.move(400, 300);
  await page.mouse.down();
  const panning = page.mouse.move(700, 450, { steps: 60 });
  await page.waitForTimeout(150);
  await other.page.selectOption("#rb-view", "free");
  await panning;
  await page.mouse.up();
  await page.waitForTimeout(1000);
  for (const p of [page, other.page]) assert.equal(await p.$eval("#rb-view", el => el.value), "free");
  assert.deepEqual([...errors, ...other.errors], []);
  await other.page.close();
  await page.close();
});
