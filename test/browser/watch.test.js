// Headless Chromium watches a recording: the page, the overlay, an iframe and <stv-map>, each
// without an error. match.jsonl.gz is 25 s of cp_gullywash, payload.jsonl.gz 25 s of pl_upward,
// players renamed. Offline: the maps come from testdata/maps, demos.tf's overview list is empty.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

// Picks a view from the room bar's view menu.
/** @param {import("playwright-core").Page} page @param {string} mode */
async function choose(page, mode) {
  await page.click("#rb-view");
  await page.click(`#rb-viewmenu button[data-mode="${mode}"]`);
}

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const MAP = "cp_gullywash_f9";
const PAYLOAD_MAP = "pl_upward_f12";
const BIN = path.join(ROOT, "target/debug/stvtf-web");
const WAIT = 20_000;

/** @type {import("node:child_process").ChildProcess} */
let server;
/** @type {import("playwright-core").Browser} */
let browser;
let root = "",
  base = "",
  payload = "";
const cache = mkdtempSync(path.join(tmpdir(), "stvtf-browser-"));

const freePort = () =>
  new Promise(resolve => {
    const s = createServer().listen(0, "127.0.0.1", () => {
      const { port } = /** @type {import("node:net").AddressInfo} */ (s.address());
      s.close(() => resolve(port));
    });
  });

before(async () => {
  for (const map of [MAP, PAYLOAD_MAP])
    copyFileSync(path.join(ROOT, `testdata/maps/${map}.bsp`), path.join(cache, `${map}.bsp`));
  mkdirSync(path.join(cache, "demostf"));
  writeFileSync(path.join(cache, "demostf/boundaries.json"), "{}");
  const listen = `127.0.0.1:${await freePort()}`;
  root = `http://${listen}/`;
  base = `${root}w/demo/`;
  payload = `${root}w/payload/`;
  const args = [
    "--demo",
    path.join(ROOT, "test/browser/match.jsonl.gz"),
    "--demos",
    path.join(ROOT, "test/browser"),
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
  await page.addInitScript(viewState);
  page.on("console", m => {
    const at = m.location().url;
    if (m.type() === "error" && !at.includes("/overview/")) errors.push(`${m.text()} (${at})`);
  });
  return { page, errors };
}

// A page's map state, from its own modules, as `window.viewState` once loaded; and its zoom.
const viewState = () =>
  addEventListener("load", async () => {
    const main = [...document.scripts].find(s => s.src.includes("main.js"));
    if (main) window.viewState = (await import(main.src.replace("main.js", "core/core.js"))).state;
  });
const scale = page => page.evaluate(() => window.viewState.view.scale);

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

test("what goes wrong on the page is told to the server", async () => {
  const page = await browser.newPage();
  try {
    await page.goto(base);
    await until(page, m => document.getElementById("map").textContent === m, MAP);
    const told = page.waitForRequest(r => r.url() === `${root}fault`);
    await page.evaluate(() => void setTimeout(() => void Promise.reject(new RangeError("out of it"))));
    const fault = JSON.parse((await told).postData() ?? "");
    assert.equal(fault.kind, "RangeError");
    assert.equal(fault.message, "out of it");
    assert.equal(fault.page, "/w/demo/");
    assert.match(fault.build, /^[0-9a-f]{16}$/);
  } finally {
    await page.close();
  }
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

test("Watch together shows greyed out and spinning until the timeline is ready", async () => {
  const { page, errors } = await open();
  let ready = false;
  await page.route("**/w/*/room", route =>
    route.request().method() === "GET" && !ready ? route.fulfill({ status: 409, body: "not yet" }) : route.continue(),
  );
  await page.goto(base);
  await until(page, () => {
    const b = /** @type {HTMLButtonElement} */ (document.getElementById("roomopen"));
    return !b.hidden && b.disabled && b.classList.contains("busy");
  });
  ready = true;
  await until(page, () => {
    const b = /** @type {HTMLButtonElement} */ (document.getElementById("roomopen"));
    return !b.disabled && !b.classList.contains("busy");
  });
  assert.deepEqual(
    errors.filter(e => !e.includes("409")),
    [],
  );
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
  // The HUD and points stay as they are through a jump, updated rather than built again.
  await until(page, () => document.querySelector("#hud-blue .card") && document.querySelector("#cps > *"));
  await page.evaluate(() => (window.card = document.querySelector("#hud-blue .card")));
  await page.$eval("#rb-time", el => {
    el.value = "0";
    el.dispatchEvent(new Event("change"));
  });
  await until(page, () => document.getElementById("rb-clock").textContent.startsWith("0:00 /"));
  assert.equal(await page.textContent("#status"), "paused", "a seek keeps it paused");
  await page.waitForTimeout(300);
  assert.ok(await page.evaluate(() => window.card.isConnected), "the HUD's cards are kept");

  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(viewer.page, () => document.getElementById("status").textContent === "paused");
  assert.ok(await viewer.page.$eval("#rb-play", el => el.disabled), "only the owner controls it");
  // A linked moment moves the room for its owner only, once.
  const room = page.url(),
    clock = (/** @type {import("playwright-core").Page} */ p, /** @type {string} */ at) =>
      until(p, a => document.getElementById("rb-clock").textContent.startsWith(`${a} /`), at);
  await viewer.page.goto(`${room}?t=0:20`);
  await until(viewer.page, () => document.getElementById("status").textContent === "paused");
  await viewer.page.waitForTimeout(500);
  await clock(viewer.page, "0:00");
  await page.goto(`${room}?t=0:20`);
  await clock(page, "0:20");
  await clock(viewer.page, "0:20");
  await page.$eval("#rb-time", el => {
    el.value = "0";
    el.dispatchEvent(new Event("change"));
  });
  await clock(viewer.page, "0:00");
  await page.reload();
  await until(page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-play")).disabled));
  await page.waitForTimeout(500);
  await clock(viewer.page, "0:00");
  await page.click("#rb-play");
  // A demo plays as a replay.
  await until(viewer.page, () => document.getElementById("status").textContent === "replay");
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
  await until(page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-marker")).disabled));
  // No slow motion in a room (TF2 shows it as a stutter): 1×, then 2×.
  await until(page, () => document.getElementById("rb-speed")?.title === "Speed 1×: 2× next");
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(viewer.page, m => document.getElementById("map").textContent === m, MAP);
  const clean = await ink(viewer.page);
  // A drag that slips off the map selects no text; what a viewer cannot use shows no hand.
  assert.equal(await viewer.page.$eval("#app", el => getComputedStyle(el).userSelect), "none");
  assert.equal(await viewer.page.$eval("#myname", el => getComputedStyle(el).userSelect), "text");
  for (const id of ["#chat", "#feed"])
    assert.equal(await viewer.page.$eval(id, el => getComputedStyle(el).userSelect), "text", `${id} copies`);
  assert.equal(await viewer.page.$eval("#app", el => el.classList.contains("seekable")), false);
  await page.keyboard.press("d");
  assert.equal(
    await page.$eval("#c", el => getComputedStyle(el).cursor),
    "crosshair",
    "the marker shows as a crosshair",
  );
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

test("the marker stays, for those who come later too, until an admin clears it", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-marker")).disabled));
  const clean = await ink(page);
  await page.keyboard.press("m");
  await page.mouse.move(300, 300);
  await page.mouse.down();
  for (let i = 1; i <= 20; i++) await page.mouse.move(300 + i * 10, 300 + i * 5);
  await page.mouse.up();
  await page.waitForTimeout(500);
  assert.ok((await ink(page)) > clean + 100, "the marker shows");
  const later = await open();
  await later.page.goto(page.url());
  await until(later.page, m => document.getElementById("map").textContent === m, MAP);
  // Once it has drawn it.
  let marked = await ink(later.page);
  for (let i = 0; i < 50 && marked <= clean + 100; i++) {
    await later.page.waitForTimeout(100);
    marked = await ink(later.page);
  }
  await page.click("#rb-clear");
  await later.page.waitForTimeout(500);
  assert.ok(marked > (await ink(later.page)) + 100, "one who came later saw it, until it was cleared");
  assert.deepEqual([...errors, ...later.errors], []);
  await later.page.close();
  await page.close();
});

test("people in a room see each other and admins make others admin", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await page.fill("#myname", "coach");
  await page.press("#myname", "Enter");
  // One's own name is the box in one's row, with the room's name for one as its placeholder.
  await until(page, () => /** @type {HTMLInputElement} */ (document.getElementById("myname")).placeholder === "coach");
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(page, () => document.querySelectorAll("#peoplelist li").length === 2);
  assert.match(await viewer.page.textContent("#peoplelist"), /coach owner.*\(you\)/);
  assert.equal(
    await viewer.page.$eval("#peoplelist #myname", e => /** @type {HTMLInputElement} */ (e).placeholder),
    "Guest 2",
  );
  await page.click("#peoplelist button");
  await until(viewer.page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-play")).disabled));
  assert.match(await viewer.page.textContent("#peoplelist"), /\(you\) admin/);
  await page.$eval("#peoplelist input.ink", el => {
    el.value = "#00ff00";
    el.dispatchEvent(new Event("change"));
  });
  await until(viewer.page, () =>
    [...document.querySelectorAll("#peoplelist .ink")].some(
      el => getComputedStyle(el).backgroundColor === "rgb(0, 255, 0)",
    ),
  );
  // The marker, on, shows the ink.
  const markerShows = (/** @type {string} */ colour) =>
    until(
      page,
      c => getComputedStyle(/** @type {Element} */ (document.getElementById("rb-marker"))).backgroundColor === c,
      colour,
    );
  await page.keyboard.press("d");
  await markerShows("rgb(0, 255, 0)");
  await page.keyboard.press("r");
  await markerShows("rgb(208, 69, 63)");
  await page.keyboard.press("y");
  await markerShows("rgb(0, 255, 0)");
  assert.deepEqual([...errors, ...viewer.errors], []);
  await viewer.page.close();
  await page.close();
});

test("an admin clicking a player has everyone follow them", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-view")).disabled));
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(page, () => document.querySelectorAll("#hud-blue .card").length > 0);
  await page.click("#hud-blue .card");
  const name = await until(
    page,
    () => !document.getElementById("follow").hidden && document.getElementById("follow-name").textContent,
  );
  await until(
    viewer.page,
    n => document.getElementById("follow-name").textContent === n && !document.getElementById("follow").hidden,
    name,
  );
  assert.equal(await viewer.page.$eval("#rb-view", el => /** @type {HTMLElement} */ (el).dataset.mode), "follow");
  await viewer.page.click("#follow-stop");
  assert.equal(await viewer.page.$eval("#follow", el => el.hidden), false, "a viewer cannot stop following");
  // The admin scrolls in on them: so does everyone.
  const was = await scale(viewer.page);
  await page.mouse.move(500, 400);
  for (let i = 0; i < 5; i++) await page.mouse.wheel(0, -200);
  await until(viewer.page, w => window.viewState.view.scale > w * 1.5, was);
  assert.deepEqual([...errors, ...viewer.errors], []);
  await viewer.page.close();
  await page.close();
});

test("admins take the view over by choosing or moving it", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  const other = await open();
  await other.page.goto(page.url());
  await until(page, () => document.querySelectorAll("#peoplelist button").length === 1);
  await page.click("#peoplelist button");
  await until(other.page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-view")).disabled));
  await choose(page, "admin");
  await until(other.page, () => document.getElementById("rb-view")?.dataset.mode === "admin");
  // Slow requests, as in a browser whose connections are busy: the leading admin's camera moves
  // still arrive after the other one took over.
  await page.route("**/view", r => setTimeout(() => r.continue(), 300));
  await page.mouse.move(400, 300);
  await page.mouse.down();
  const panning = page.mouse.move(700, 450, { steps: 60 });
  await page.waitForTimeout(150);
  await choose(other.page, "free");
  await panning;
  await page.mouse.up();
  await page.waitForTimeout(1000);
  for (const p of [page, other.page])
    assert.equal(await p.$eval("#rb-view", el => /** @type {HTMLElement} */ (el).dataset.mode), "free");
  await page.unroute("**/view");
  await choose(page, "admin");
  await until(other.page, () => document.getElementById("rb-lead").textContent === "by Guest 1");
  // The other admin moving the view by hand takes it over, and the first moving it takes it back.
  await other.page.mouse.move(400, 300);
  await other.page.mouse.wheel(0, -200);
  await until(page, () => document.getElementById("rb-lead").textContent === "by Guest 2");
  assert.equal(
    await page.$eval("#rb-view", el => /** @type {HTMLElement} */ (el).dataset.mode),
    "admin",
    "moving it keeps the view",
  );
  await page.mouse.move(400, 300);
  await page.mouse.down();
  await page.mouse.move(450, 330, { steps: 5 });
  await page.mouse.up();
  await until(
    other.page,
    () =>
      document.getElementById("rb-lead").textContent === "by you" ||
      document.getElementById("rb-lead").textContent === "by Guest 1",
  );
  await until(page, () => document.getElementById("rb-lead").textContent === "by you");
  assert.deepEqual([...errors, ...other.errors], []);
  await other.page.close();
  await page.close();
});

test("the leader switching from their view to auto zoom stays on auto zoom", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-view")).disabled));
  await choose(page, "admin");
  await until(viewer.page, () => document.getElementById("rb-view")?.dataset.mode === "admin");
  // The camera moving does not touch the menu, which would flicker while open.
  await page.evaluate(() => {
    window.menuChanges = 0;
    new MutationObserver(r => (window.menuChanges += r.length)).observe(document.getElementById("rb-view"), {
      subtree: true,
      childList: true,
      characterData: true,
      attributes: true,
    });
  });
  await page.mouse.move(400, 300);
  await page.mouse.down();
  await page.mouse.move(500, 350, { steps: 20 });
  await page.mouse.up();
  await page.waitForTimeout(300);
  assert.equal(await page.evaluate(() => window.menuChanges), 0);
  // Slow requests: camera moves sent before the switch arrive after it.
  await page.route("**/view", r => setTimeout(() => r.continue(), 300));
  await page.mouse.move(400, 300);
  await page.mouse.down();
  const panning = page.mouse.move(700, 450, { steps: 40 });
  await page.waitForTimeout(150);
  await choose(page, "auto");
  await panning;
  await page.mouse.up();
  const seen = [];
  for (let i = 0; i < 10; i++) {
    seen.push(await viewer.page.$eval("#rb-view", el => /** @type {HTMLElement} */ (el).dataset.mode));
    await page.waitForTimeout(150);
  }
  // Once on auto zoom, it never goes back.
  const from = seen.indexOf("auto");
  assert.ok(from >= 0 && seen.slice(from).every(v => v === "auto"), seen.join(" "));
  assert.equal(await page.$eval("#rb-view", el => /** @type {HTMLElement} */ (el).dataset.mode), "auto");
  await until(page, () => document.getElementById("autozoom").hidden);
  assert.deepEqual([...errors, ...viewer.errors], []);
  await viewer.page.close();
  await page.close();
});

test("an admin's view is by hand, for them too, until they click a player", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(page, () => !(/** @type {HTMLButtonElement} */ (document.getElementById("rb-view")).disabled));
  await choose(page, "auto");
  await until(page, () => document.getElementById("autozoom").hidden);
  await choose(page, "admin");
  // Auto zoom is off for the admin: its button shows.
  await until(page, () => !document.getElementById("autozoom").hidden);
  await until(viewer.page, () => document.getElementById("rb-view")?.dataset.mode === "admin");
  await until(page, () => document.querySelectorAll("#hud-red .card").length > 0);
  await page.click("#hud-red .card");
  const name = await until(
    page,
    () => !document.getElementById("follow").hidden && document.getElementById("follow-name").textContent,
  );
  await until(
    viewer.page,
    n => !document.getElementById("follow").hidden && document.getElementById("follow-name").textContent === n,
    name,
  );
  await choose(page, "admin");
  await until(page, () => document.getElementById("follow").hidden);
  assert.deepEqual([...errors, ...viewer.errors], []);
  await viewer.page.close();
  await page.close();
});

test("an admin clicking an event's time watches from just before it", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => document.documentElement.querySelector(".stvtf.seekable"));
  const tick = await until(page, () => document.querySelector("#feed time[data-tick]")?.getAttribute("data-tick"));
  await page.waitForTimeout(3000);
  const seconds = async () => {
    const [m, s] = (await page.textContent("#rb-clock")).split(" / ")[0].split(":").map(Number);
    return m * 60 + s;
  };
  const was = await seconds();
  await page.click(`#feed time[data-tick="${tick}"]`);
  // Back to before it, and on from there: the event comes round again.
  await until(
    page,
    w => Number(document.getElementById("rb-clock").textContent.split(" / ")[0].split(":")[1]) < w,
    was,
  );
  assert.ok((await seconds()) < was);
  await until(page, t => document.querySelectorAll(`#feed time[data-tick="${t}"]`).length > 0, tick);
  assert.deepEqual(errors, []);
  await page.close();
});

test("a viewer of a match page goes back on their own", async () => {
  const { page, errors } = await open();
  /** @type {string[]} */
  const streams = [];
  page.on("request", r => r.url().includes("/events") && streams.push(new URL(r.url()).pathname));
  await page.goto(base);
  await until(
    page,
    () => !document.getElementById("roombar").hidden && document.getElementById("rb-clock").textContent,
  );
  assert.equal(await page.$eval("#rb-marker", el => getComputedStyle(el).display), "none", "no room controls");
  const box = await page.$eval("#rb-track", e => {
    const r = e.getBoundingClientRect();
    return [r.x, r.y, r.width];
  });
  await page.mouse.click(box[0] + box[2] * 0.1, box[1] + 20);
  await until(page, () => document.getElementById("status").textContent === "replay");
  await page.waitForTimeout(500);
  assert.ok(
    streams.some(s => s.startsWith("/r/")),
    streams.join(" "),
  );
  // A demo has no live to go back to.
  assert.ok(await page.$eval("#rb-live", el => el.hidden));
  assert.deepEqual(errors, []);
  await page.close();
});

test("the wheel zooms the timeline in about the pointer, and out to all of it again", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await until(
    page,
    () => !document.getElementById("roombar").hidden && document.getElementById("rb-clock").textContent,
  );
  const shows = () =>
    page.$eval("#rb-time", (/** @type {HTMLInputElement} */ t) => [
      Number(t.min),
      Number(t.max),
      /** @type {HTMLElement} */ (t.parentElement).classList.contains("zoomed"),
    ]);
  const [from, to, zoomed] = await shows();
  assert.equal(zoomed, false);
  const box = await page.$eval("#rb-track", e => {
    const r = e.getBoundingClientRect();
    return [r.x, r.y, r.width];
  });
  await page.mouse.move(box[0] + box[2] * 0.9, box[1] + 20);
  await page.mouse.wheel(0, -100);
  await until(page, () => document.getElementById("rb-track").classList.contains("zoomed"));
  const [inFrom, inTo] = await shows();
  assert.ok(inFrom > from && inTo <= to + 1 && inTo - inFrom < to - from, `${inFrom} to ${inTo} of ${from} to ${to}`);
  await page.mouse.wheel(0, 400);
  await until(page, () => !document.getElementById("rb-track").classList.contains("zoomed"));
  assert.equal((await shows())[0], from);
  assert.deepEqual(errors, []);
  await page.close();
});

test("a link to a moment watches from there", async () => {
  const { page, errors } = await open();
  await page.goto(`${base}?t=0:10`);
  await until(page, () => /^0:1\d \//.test(document.getElementById("rb-clock").textContent ?? ""));
  await until(page, () => document.getElementById("status").textContent === "replay");
  // The address stays as it was opened.
  await until(page, () => /^0:1[2-9] \//.test(document.getElementById("rb-clock").textContent ?? ""));
  assert.match(page.url(), /[?]t=0:10$/);
  // A reload watches on from where one was.
  // Kept every so often: a little before, at most.
  await until(page, () => /^0:(1[5-9]|[2-9]\d) \//.test(document.getElementById("rb-clock").textContent ?? ""));
  await page.reload();
  await until(page, () => document.getElementById("status").textContent === "replay");
  const clock = await page.$eval("#rb-clock", e => e.textContent ?? "");
  assert.match(clock, /^0:(1[3-9]|[2-9]\d) \//);
  assert.deepEqual(errors, []);
  await page.close();
});

test("a payload match shows its cart, buildings, airblasts, über and capture", async () => {
  // Two pages, to take half as long: from the start, and from just before the über.
  const [early, late] = await Promise.all([open(), open()]);
  await Promise.all([early.page.goto(`${payload}?t=0:00`), late.page.goto(`${payload}?t=0:10`)]);
  for (const { page } of [early, late])
    await until(page, m => document.getElementById("map").textContent === m, PAYLOAD_MAP);
  const drawn = (/** @type {import("playwright-core").Page} */ page, /** @type {string} */ kind) =>
    until(page, k => window.viewState.effects.some(e => e.kind === k), kind);
  const fed = (/** @type {string} */ text) =>
    until(late.page, t => document.getElementById("feed").textContent.includes(t), text);
  await drawn(early.page, "airblast");
  await until(early.page, () => {
    const snap = window.viewState.snaps.at(-1);
    return snap?.objectives?.some(o => o.kind === "cart") && snap.buildings?.some(b => b.kind === "sentry");
  });
  assert.ok((await colours(early.page)) > 20, "the map is drawn");
  await drawn(late.page, "uber");
  await fed("ÜBER");
  await until(late.page, () =>
    window.viewState.snaps.at(-1)?.objectives?.some(o => o.kind === "cart" && o.cappers > 0),
  );
  await fed("sapped");
  await fed("destroyed");
  await fed("BLU captured point 1");
  assert.deepEqual([...early.errors, ...late.errors], []);
  await early.page.close();
  await late.page.close();
});

test("h hides the player HUD and s the scores, each on its own", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await until(page, () => !(/** @type {HTMLElement} */ (document.getElementById("scoreboard")).hidden));
  const shown = () =>
    page.evaluate(() =>
      ["hud-blue", "scoreboard"].map(
        id => getComputedStyle(/** @type {HTMLElement} */ (document.getElementById(id))).display !== "none",
      ),
    );
  await page.keyboard.press("h");
  assert.deepEqual(await shown(), [false, true], "h: no HUD, the scores still");
  await page.keyboard.press("h");
  await page.keyboard.press("s");
  assert.deepEqual(await shown(), [true, false], "s: no scores, the HUD still");
  await page.keyboard.press("s");
  assert.deepEqual(errors, []);
  await page.close();
});

// The speakers list, open, on a page that may set up comms. `site` answers for stv.tf; `posted`
// keeps what the page posts.
/** @param {{ comms?: object; voices?: object[]; demos?: object[]; refuse?: string }} site */
async function openSpeakers(site) {
  const { page, errors } = await open();
  /** @type {{ path: string; body: any }[]} */
  const posted = [];
  const json = (/** @type {string} */ at, /** @type {() => unknown} */ body) =>
    page.route(`**/w/demo/${at}`, route => {
      const request = route.request();
      if (request.method() !== "POST") return route.fulfill({ json: body() });
      posted.push({ path: at, body: request.postDataJSON() });
      return site.refuse ? route.fulfill({ status: 400, body: site.refuse }) : route.fulfill({ status: 204 });
    });
  await json("voice-invite", () => ({ invite: "0123456789abcdef", bot: "https://discord.test/bot" }));
  await json("voice-comms", () => site.comms ?? { discord: null, mumble: [] });
  await json("voices", () => site.voices ?? []);
  await json("demos", () => site.demos ?? []);
  await json("voice-mumble", () => ({}));
  await json("voice-match", () => ({}));
  await page.goto(base);
  await until(page, () => ["blue", "red"].every(t => document.getElementById(`hud-${t}`).children.length >= 6));
  await page.click("#voicetoggle");
  await page.waitForSelector("#voicelist .vrow[data-steamid]");
  const shows = (/** @type {string} */ text) =>
    until(page, t => document.getElementById("voicelist").textContent.includes(t), text);
  // What the `n`th post to `at` said.
  const sent = async (/** @type {string} */ at, /** @type {number} */ n = 1) => {
    const there = () => posted.filter(p => p.path === at);
    for (let i = 0; i < 100 && there().length < n; i++) await page.waitForTimeout(50);
    return there()[n - 1]?.body;
  };
  return { page, errors, shows, sent };
}

test("the speakers list mutes a player, a team and everyone, and turns voice off", async () => {
  const { page, errors } = await openSpeakers({});
  assert.equal(await page.textContent("#voicetoggle"), "🔊");
  assert.deepEqual(await page.$$eval("#voicelist .vtname", els => els.map(e => e.textContent)), ["RED", "BLU"]);
  assert.equal(await page.$$eval("#voicelist .vrow[data-steamid]", els => els.length), 12);
  const muted = () => page.$$eval("#voicelist [data-mute].muted", els => els.length);
  // By name: the list keeps the HUD's order, which changes as players die.
  const one = `#voicelist [data-mute="${await page.getAttribute("#voicelist [data-mute]", "data-mute")}"]`;
  await page.click(one);
  assert.equal(await muted(), 1);
  await page.click(one);
  assert.equal(await muted(), 0);
  await page.click('#voicelist button[data-team="red"]');
  assert.equal(await page.textContent('#voicelist button[data-team="red"]'), "Unmute team");
  assert.equal(await muted(), 6);
  await page.click('#voicelist [data-act="swap"]');
  assert.equal(await page.textContent('#voicelist button[data-team="blue"]'), "Unmute team");
  assert.equal(await page.textContent('#voicelist button[data-team="red"]'), "Mute team");
  await page.click('#voicelist [data-act="all"]');
  assert.equal(await page.textContent('#voicelist [data-act="all"]'), "Unmute all");
  assert.equal(await muted(), 12);
  await page.click('#voicelist [data-act="all"]');
  assert.equal(await muted(), 0);
  // A volume is kept while the list is drawn anew.
  await page.$eval("#voicelist input[type=range]", (/** @type {HTMLInputElement} */ el) => {
    el.value = "40";
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await page.click('#voicelist button[data-team="red"]');
  assert.equal(
    await page.$eval("#voicelist input[type=range]", (/** @type {HTMLInputElement} */ el) => el.value),
    "40",
  );
  await page.click('#voicelist [data-act="off"]');
  assert.equal(await page.$eval("#voicelist", el => el.hidden), true);
  assert.equal(await page.textContent("#voicetoggle"), "🔇");
  assert.deepEqual(errors, []);
  await page.close();
});

test("comms for Discord: the command to paste is copied, with the delay asked for", async () => {
  /** @type {{ comms?: object }} */
  const site = {};
  const { page, errors, shows } = await openSpeakers(site);
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.click('#voicelist [data-action="go:home"]');
  assert.deepEqual(await page.$$eval("#voicelist .vcopy", els => els.map(e => e.textContent)), ["Discord", "Mumble"]);
  assert.ok(await page.$eval("#voicelist", el => el.classList.contains("wide")));
  await page.click('#voicelist [data-action="go:discord"]');
  await shows("Waiting for bot…");
  assert.equal(await page.getAttribute("#voicelist .vhint a", "href"), "https://discord.test/bot");
  await page.fill('#voicelist [data-action="delay"]', "0");
  await page.press('#voicelist [data-action="delay"]', "Tab");
  await page.click('#voicelist [data-action="copy"]');
  await shows("✓ Copied");
  assert.equal(
    await page.evaluate(() => navigator.clipboard.readText()),
    `/stv join link:${base}?voice=0123456789abcdef delay:0`,
  );
  // The bot came: on to where it is.
  site.comms = { discord: { status: "In General", state: "in" }, mumble: [] };
  await shows("Discord: In General");
  await shows("Nobody there");
  await page.click('#voicelist [data-action="go:"]');
  await shows("🎧 Comms");
  assert.equal(await page.$eval("#voicelist", el => el.classList.contains("wide")), false);
  assert.deepEqual(errors, []);
  await page.close();
});

test("comms for Mumble: a channel is joined, moved and left, and what will not do says why", async () => {
  /** @type {{ comms?: object; refuse?: string }} */
  const site = {};
  const { page, errors, shows, sent } = await openSpeakers(site);
  const field = (/** @type {string} */ name) => `#voicelist [data-mumble="${name}"]`;
  await page.click('#voicelist [data-action="go:home"]');
  await page.click('#voicelist [data-action="go:mumble"]');
  await page.click('#voicelist [data-action="join"]');
  await shows("Invalid server");
  await page.fill(field("server"), "mumble.test:1234");
  await page.click('#voicelist [data-action="join"]');
  await shows("Channel name required");
  await page.fill(field("channel"), "Team A");
  await page.fill(field("password"), "secret");
  site.refuse = "Server full";
  await page.click('#voicelist [data-action="join"]');
  await shows("Server full");
  site.refuse = "";
  site.comms = {
    discord: null,
    mumble: [{ id: 1, host: "mumble.test", port: 1234, channel: "Team A", delay: 90, status: "connecting" }],
  };
  await page.click('#voicelist [data-action="join"]');
  assert.deepEqual(await sent("voice-mumble", 2), {
    host: "mumble.test",
    port: 1234,
    password: "secret",
    channel: "Team A",
    delay: 90,
  });
  await shows("Mumble: connecting");
  assert.ok(await page.$("#voicelist .vdot.wait"));
  const inTeamA = {
    id: 1,
    host: "mumble.test",
    port: 1234,
    channel: "Team A",
    delay: 90,
    status: "In Team A",
    state: "in",
  };
  site.comms = { discord: null, mumble: [inTeamA] };
  await shows("Mumble: In Team A");
  await shows("tv_delay 90 s");
  assert.ok(await page.$("#voicelist .vdot.good"));
  // Another channel is most likely on the same server, its password known.
  await page.click('#voicelist [data-action="go:mumble"]');
  assert.equal(await page.inputValue(field("server")), "mumble.test:1234");
  assert.equal(await page.getAttribute(field("password"), "placeholder"), "unchanged");
  await page.click('#voicelist [data-action="go:home"]');
  await page.click('#voicelist [data-action="go:reconnect:1"]');
  assert.equal(await page.inputValue(field("channel")), "Team A");
  await page.fill(field("channel"), "Team B");
  site.comms = { discord: null, mumble: [{ ...inTeamA, channel: "Team B", status: "In Team B" }] };
  await page.click('#voicelist [data-action="join"]');
  assert.deepEqual(await sent("voice-mumble", 3), {
    id: 1,
    host: "mumble.test",
    port: 1234,
    channel: "Team B",
    delay: 90,
  });
  await shows("Mumble: In Team B");
  await page.click('#voicelist [data-action="go:reconnect:1"]');
  site.comms = { discord: null, mumble: [] };
  await page.click('#voicelist [data-action="leave"]');
  assert.deepEqual(await sent("voice-mumble", 4), { leave: true, id: 1 });
  await until(page, () => document.querySelectorAll("#voicelist .vcopy").length === 2);
  // The only channel, and the bot would not join it: its form again, with why.
  site.comms = {
    discord: null,
    mumble: [
      {
        id: 2,
        host: "mumble.test",
        port: 64738,
        channel: "Gone",
        delay: 30,
        status: "Could not join: no such channel",
      },
    ],
  };
  await shows("Could not join: no such channel");
  await page.click('#voicelist [data-action="go:home"]');
  assert.equal(await page.inputValue(field("server")), "mumble.test");
  assert.equal(await page.inputValue('#voicelist [data-action="delay"]'), "30");
  assert.ok(await page.$("#voicelist .vdot.bad"));
  // The browser tells of what was refused itself.
  assert.deepEqual(
    errors.filter(e => !e.includes("400")),
    [],
  );
  await page.close();
});

test("who is who: those in the channel are put on players, by choice or by Steam ID", async () => {
  const person = { steamid: null, linked: null, heard: true, confirmed: false };
  /** @type {{ comms: object; voices: object[]; refuse?: string }} */
  const site = {
    comms: { discord: { status: "In General", state: "in" }, mumble: [] },
    voices: [
      { ...person, id: "71", name: "player 3 :)" },
      { ...person, id: "72", name: "coach", steamid: "[U:1:1001]" },
      { ...person, id: "73", name: "caster", confirmed: true },
    ],
  };
  const { page, errors, shows, sent } = await openSpeakers(site);
  await page.click('#voicelist [data-action="go:home"]');
  await shows("Who is who");
  assert.equal(await page.textContent("#voicelist .vteam .vhint"), "2/3");
  const row = (/** @type {number} */ n) => `#voicelist .vwho:nth-child(${n} of .vwho)`;
  // A look-alike name comes first; a guess shows as one until confirmed.
  assert.equal(await page.textContent(`${row(1)} option:nth-child(2)`), "Player 3");
  assert.equal(await page.$eval(`${row(2)} select`, el => el.className), "maybe");
  assert.equal(
    await page.$eval(`${row(2)} select`, (/** @type {HTMLSelectElement} */ el) => el.selectedOptions[0].text),
    "Player 1",
  );
  const three = await page.$eval(`${row(1)} option:nth-child(2)`, (/** @type {HTMLOptionElement} */ o) => o.value);
  await page.selectOption(`${row(1)} select`, three);
  assert.deepEqual(await sent("voice-match", 1), { discord: "71", player: Number(three) });
  await page.click(`${row(2)} [data-action="who-ok:72"]`);
  assert.deepEqual(await sent("voice-match", 2), { discord: "72", steamid: "[U:1:1001]" });
  site.voices = [
    site.voices[0],
    { ...person, id: "72", name: "coach", steamid: "[U:1:1001]", confirmed: true },
    site.voices[2],
  ];
  await until(page, () => !document.querySelector('#voicelist [data-action^="who-ok"]'));
  assert.equal(await page.$eval(`${row(2)} select`, el => el.className), "set");
  await page.selectOption(`${row(3)} select`, "steamid");
  await page.fill(`${row(3)} input`, "[U:1:5]");
  await page.click('#voicelist [data-action="who-cancel"]');
  await page.selectOption(`${row(3)} select`, "steamid");
  await page.fill(`${row(3)} input`, "[U:1:99]");
  await page.press(`${row(3)} input`, "Enter");
  assert.deepEqual(await sent("voice-match", 3), { discord: "73", steamid: "[U:1:99]" });
  // One not in the match is kept by their Steam ID.
  site.voices = [
    site.voices[0],
    site.voices[1],
    { ...person, id: "73", name: "caster", steamid: "[U:1:99]", confirmed: true },
  ];
  await until(page, () => [...document.querySelectorAll("#voicelist option")].some(o => o.textContent === "[U:1:99]"));
  site.refuse = "Not a Steam ID";
  await page.selectOption(`${row(1)} select`, "none");
  assert.deepEqual(await sent("voice-match", 4), { discord: "71" });
  await shows("Not a Steam ID");
  // The browser tells of what was refused itself.
  assert.deepEqual(
    errors.filter(e => !e.includes("400")),
    [],
  );
  await page.close();
});

test("demos with comms in them are listed under the speakers, to watch or download", async () => {
  const { page, errors, shows } = await openSpeakers({
    demos: [{ map: "cp_gullywash_f9", mb: 12, href: `${base}demos/1.dem`, watch: `${root}w/d1/` }],
  });
  await shows("With comms");
  assert.equal(await page.textContent("#voicelist a.vwatch"), "▶ cp_gullywash_f9");
  assert.equal(await page.textContent("#voicelist a.vdemo"), "12 MB ↓");
  // Built once asked: the server says so by a cookie of the mark.
  await page.$eval("#voicelist", list =>
    list.addEventListener("click", e => {
      const a = /** @type {Element} */ (e.target).closest("a.vdemo");
      if (a) (window.asked ??= []).push(/** @type {HTMLAnchorElement} */ (a).href);
    }),
  );
  await page.click("#voicelist a.vdemo");
  await shows("building…");
  await page.click("#voicelist a.vdemo");
  const asked = await page.evaluate(() => window.asked);
  const mark = new URL(asked[0]).searchParams.get("built");
  assert.match(mark ?? "", /^[0-9a-z]+$/);
  assert.equal(asked[1], `${base}demos/1.dem`, "not asked for again while it is built");
  await page.evaluate(m => (document.cookie = `built=${m}; Path=/`), mark);
  await shows("12 MB ↓");
  assert.equal(await page.evaluate(() => document.cookie.includes("built=")), false);
  assert.deepEqual(errors, []);
  await page.close();
});

// 20 ms of someone's voice, as stvtf writes it.
const FRAME = "aAxxLkNKtQnFTYqZHNAoDzcHeDUBBonRxudGf/k+Clm/zqvyYQ1a6eNr+BpHPKBKCewIjR5wCTYEg+6a";

test("who talks shows on their HUD card and in the speakers list, and a click there mutes them", async () => {
  const { page, errors } = await openSpeakers({});
  await page.waitForSelector("#hud-red .card");
  // A RED player says something, for as long as `talking` is set.
  const who = await page.evaluate(async frame => {
    const main = /** @type {HTMLScriptElement} */ ([...document.scripts].find(s => s.src.includes("main.js")));
    const voice = await import(main.src.replace("main.js", "core/voice.js"));
    const card = /** @type {HTMLElement} */ (document.querySelector("#hud-red .card"));
    const id = Number(card.dataset.id);
    const { steamid } = window.viewState.players.get(id);
    window.talking = true;
    setInterval(() => {
      if (window.talking) voice.onVoice({ type: "voice", tick: 0, player: id, steamid, rate: 24000, frames: [frame] });
    }, 20);
    return { id, steamid };
  }, FRAME);
  const mark = `#hud-red .card[data-id="${who.id}"] > .vc`;
  await until(page, m => document.querySelector(m)?.matches(".wave:not(.quiet)"), mark);
  await until(page, m => [...document.querySelectorAll(`${m} > i`)].some(i => i.style.height), mark);
  await until(
    page,
    steamid =>
      document.querySelector(`#voicelist .vrow[data-steamid="${steamid}"]`)?.classList.contains("talking") &&
      document.querySelector('#voicelist .vteam[data-team="red"]')?.classList.contains("talking"),
    who.steamid,
  );
  assert.equal(await page.$eval('#voicelist .vteam[data-team="blue"]', el => el.classList.contains("talking")), false);
  // Silent again: a faint 🔊, there to mute them by.
  await page.evaluate(() => (window.talking = false));
  await until(page, m => document.querySelector(m)?.matches(".quiet:not(.wave)"), mark);
  assert.equal(await page.textContent(mark), "🔊");
  await page.click(mark);
  await until(page, m => document.querySelector(m)?.textContent === "🔇", mark);
  assert.equal(await page.$eval("#follow", el => el.hidden), true, "muting follows nobody");
  await page.click(mark);
  await until(page, m => document.querySelector(m)?.textContent === "🔊", mark);
  assert.deepEqual(errors, []);
  await page.close();
});

test("an admin names the players and teams, from a league's names, for everyone in the room", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => document.getElementById("scoreboard").classList.contains("scoring"));
  await until(page, () => ["blue", "red"].every(t => document.getElementById(`hud-${t}`).children.length >= 6));
  const viewer = await open();
  await viewer.page.goto(page.url());
  // A viewer's click on a team's name opens nothing.
  await until(viewer.page, () => !document.getElementById("scoreboard").hidden);
  await viewer.page.click(".sb-team.blue .sb-name", { force: true });
  assert.ok(await viewer.page.$eval("#names", el => el.hidden));
  // What the leagues would say: one knows two of BLU and their team.
  /** @type {any} */
  let asked;
  await page.route("**/leagues", route => {
    asked = route.request().postDataJSON();
    return route.fulfill({
      json: {
        leagues: [
          { league: "ETF2L", players: { "[U:1:1001]": "Uno", "[U:1:1005]": "Cinco" }, teams: { blue: "The Blues" } },
          { league: "RGL", players: { "[U:1:1001]": "One" }, teams: {} },
        ],
      },
    });
  });
  await page.click(".sb-team.blue .sb-name");
  const field = (/** @type {string} */ key) => `#names input[data-name="${key}"]`;
  await until(
    page,
    f => /** @type {HTMLInputElement} */ (document.querySelector(f))?.value === "Uno",
    field("[U:1:1001]"),
  );
  assert.equal(asked.players.length, 12);
  assert.deepEqual(
    asked.players.find((/** @type {any} */ p) => p.steamid === "[U:1:1001]"),
    { steamid: "[U:1:1001]", team: "blue" },
  );
  assert.equal(await page.inputValue(field("team:blue")), "The Blues");
  assert.equal(await page.inputValue(field("team:red")), "");
  assert.equal(await page.getAttribute(field("[U:1:1002]"), "placeholder"), "Player 2");
  // Another league's names, then one's own on top.
  await page.selectOption("#names select", "1");
  assert.equal(await page.inputValue(field("[U:1:1001]")), "One");
  await page.selectOption("#names select", "0");
  await page.fill(field("[U:1:1002]"), "Dos");
  await page.fill(field("team:red"), "Reds");
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/names-panel.png` });
  await page.click("#names .napply");
  await until(page, () => document.getElementById("names").hidden);
  for (const p of [page, viewer.page]) {
    await until(p, () => document.getElementById("hud-blue").textContent.includes("Uno"));
    await until(p, () => document.getElementById("hud-red").textContent.includes("Dos"));
    assert.equal(await p.textContent("#scoreboard .sb-team.blue .sb-name"), "The Blues");
    assert.equal(await p.textContent("#scoreboard .sb-team.red .sb-name"), "Reds");
    assert.equal(await p.$eval("#hud-blue", el => el.textContent.includes("Player 1 ")), false);
  }
  if (process.env.SHOTS) await viewer.page.screenshot({ path: `${process.env.SHOTS}/names-applied.png` });
  // Who comes later sees them too; the panel shows what is set.
  const later = await open();
  await later.page.goto(page.url());
  await until(later.page, () => document.getElementById("hud-blue").textContent.includes("Cinco"));
  await page.click(".sb-team.red .sb-name");
  await until(
    page,
    f => /** @type {HTMLInputElement} */ (document.querySelector(f))?.value === "Dos",
    field("[U:1:1002]"),
  );
  await page.click("#names button:not(.napply)");
  for (const p of [page, viewer.page]) {
    await until(p, () => document.getElementById("hud-red").textContent.includes("Player 2"));
    assert.equal(await p.textContent("#scoreboard .sb-team.blue .sb-name"), "BLU");
  }
  assert.deepEqual([...errors, ...viewer.errors, ...later.errors], []);
  for (const p of [page, viewer.page, later.page]) await p.close();
});

test("an admin clicks a score on the HUD to set it right, for everyone in the room", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => document.getElementById("scoreboard").classList.contains("scoring"));
  const viewer = await open();
  await viewer.page.goto(page.url());
  await until(viewer.page, () => !document.getElementById("scoreboard").hidden);
  // A viewer's click does nothing.
  await viewer.page.click("#score-blue", { force: true });
  assert.equal(await viewer.page.$(".sb-edit"), null);
  await page.click("#score-blue");
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/score-edit.png` });
  await page.fill(".sb-edit", "3");
  await page.press(".sb-edit", "Enter");
  await page.click("#score-red");
  await page.keyboard.press("Escape");
  await page.click("#score-red");
  await page.fill(".sb-edit", "2");
  await page.press(".sb-edit", "Enter");
  for (const p of [page, viewer.page]) {
    await until(p, () => document.getElementById("score-blue").textContent === "3");
    await until(p, () => document.getElementById("score-red").textContent === "2");
  }
  if (process.env.SHOTS) await viewer.page.screenshot({ path: `${process.env.SHOTS}/score-set.png` });
  // Kept through a jump within the game, and for who comes later.
  await page.$eval("#rb-time", el => {
    /** @type {HTMLInputElement} */ (el).value = "0";
    el.dispatchEvent(new Event("change"));
  });
  const later = await open();
  await later.page.goto(page.url());
  await until(later.page, () => document.getElementById("score-blue").textContent === "3");
  await page.click("#score-blue");
  await page.fill(".sb-edit", "100");
  await page.press(".sb-edit", "Enter");
  await until(page, () => document.getElementById("rb-note").textContent === "From 0 to 99.");
  assert.deepEqual(
    [...errors, ...viewer.errors, ...later.errors].filter(e => !e.includes("400")),
    [],
  );
  for (const p of [page, viewer.page, later.page]) await p.close();
});

test("a long status is cut short, not the buttons after it pushed off the page", async () => {
  const { page } = await open();
  // No stream: the status stays as it is put.
  await page.route("**/events*", r => r.abort());
  await page.goto(base);
  await page.waitForSelector("#status");
  const [button, title, status] = await page.evaluate(() => {
    const s = /** @type {HTMLElement} */ (document.getElementById("status"));
    s.textContent = "gave up: disconnected by server: Bad spectator password";
    for (const id of ["roomopen", "copylink"]) /** @type {HTMLElement} */ (document.getElementById(id)).hidden = false;
    const right = (/** @type {string} */ id) =>
      /** @type {HTMLElement} */ (document.getElementById(id)).getBoundingClientRect().right;
    return [right("roomopen"), right("title"), s.scrollWidth > s.clientWidth];
  });
  assert.ok(button <= title, `Watch together ends at ${button}, the panel at ${title}`);
  assert.ok(status, "the status is cut short");
  await page.close();
});

test("a camera is placed on the map and an admin's click puts the room's relay there", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => window.viewState?.framed);
  assert.ok(await page.$eval("#cameras", el => el.hidden));
  await page.click("#settingstoggle");
  await page.check('#settings input[data-layer="cameras"]');
  await page.click("#settingstoggle");
  await page.click('#cam-list button[data-id="new"]');
  // What it would show is said once the map's voxels are in.
  await until(page, () => document.getElementById("cam-shows").textContent !== "");
  await page.fill("#cam-name", "Mid");
  await page.fill("#cam-z", "1400");
  // Its picture, in the corner: more than a sky.
  await until(page, () => {
    const view = /** @type {HTMLCanvasElement} */ (document.getElementById("cam-view")),
      c = Object.assign(document.createElement("canvas"), { width: 64, height: 36 }).getContext("2d");
    if (!view.clientWidth || !view.width || !c) return false;
    c.drawImage(view, 0, 0, 64, 36);
    const px = new Uint32Array(c.getImageData(0, 0, 64, 36).data.buffer);
    return new Set(px).size > 8;
  });
  // Steered from the picture: E lifts it, a drag down looks down, Ctrl+Z takes each back.
  const heights = () =>
    page.evaluate(() =>
      ["cam-z", "cam-aim-z"].map(id => Number(/** @type {HTMLInputElement} */ (document.getElementById(id)).value)),
    );
  const [z, aimZ] = await heights();
  const box = await page.locator("#cam-view").boundingBox();
  assert.ok(box);
  const [mx, my] = [box.x + box.width / 2, box.y + box.height / 2];
  await page.focus("#cam-view");
  await page.keyboard.down("e");
  await page.waitForTimeout(300);
  await page.keyboard.up("e");
  await until(page, z0 => Number(/** @type {HTMLInputElement} */ (document.getElementById("cam-z")).value) > z0, z);
  await page.waitForTimeout(100);
  const [lifted, liftedAim] = await heights();
  assert.equal(liftedAim - aimZ, lifted - z, "what it looks at moves with it");
  await page.mouse.move(mx, my);
  await page.mouse.down();
  await page.mouse.move(mx, my + 30, { steps: 3 });
  await page.mouse.up();
  assert.ok((await heights())[1] < liftedAim, "looks down");
  assert.equal((await heights())[0], lifted);
  await page.keyboard.press("Control+z");
  assert.deepEqual(await heights(), [lifted, liftedAim]);
  await page.keyboard.press("Control+z");
  assert.deepEqual(await heights(), [z, aimZ]);
  // A click aims at what is under it; the slider is its height.
  await page.mouse.click(mx + 40, my + 40);
  await page.locator("#cam-z-slide").fill("1300");
  assert.deepEqual((await heights())[0], 1300);
  await page.fill("#cam-z", "1400");
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/camera-placing.png` });
  await page.click("#cam-save");
  await until(page, () => document.querySelector("#cam-list button")?.textContent === "Mid");
  assert.ok(await page.$eval("#cam-pane", el => el.hidden), "put away with the camera placed");
  const kept = await page.evaluate(map => fetch(`../../cameras/${map}`).then(r => r.json()), MAP);
  assert.equal(kept.cameras.length, 1);
  const { pos, aim } = kept.cameras[0];
  assert.equal(pos[2], 1400);
  // Its marker, clicked: the relay's camera goes there, and back with the director on the next.
  const [x, y] = await page.evaluate(async ([wx, wy]) => {
    const main = [...document.scripts].find(s => s.src.includes("main.js"));
    const view = await import(main.src.replace("main.js", "map/view.js"));
    const r = document.getElementById("c").getBoundingClientRect(),
      [sx, sy] = view.toScreen(wx, wy);
    return [sx + r.left, sy + r.top];
  }, pos);
  for (const sent of [{ pos, aim }, {}]) {
    const [asked] = await Promise.all([page.waitForRequest(r => r.url().endsWith("/camera")), page.mouse.click(x, y)]);
    assert.deepEqual(asked.postDataJSON(), sent);
    assert.equal((await (await asked.response())?.status()) ?? 0, 204);
    await page.waitForTimeout(200);
  }
  // Its name in the list does the same, with no picture of it.
  for (const sent of [{ pos, aim }, {}]) {
    const [asked] = await Promise.all([
      page.waitForRequest(r => r.url().endsWith("/camera")),
      page.click("#cam-list button[data-go]"),
    ]);
    assert.deepEqual(asked.postDataJSON(), sent);
    await until(page, on => (document.querySelector("#cam-list .on") !== null) === on, "pos" in sent);
  }
  assert.ok(await page.$eval("#cam-pane", el => el.hidden));
  // Placed anew by dragging it, then taken away.
  await page.click("#cam-list button[data-id]");
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 40, y + 10);
  await page.mouse.move(x + 80, y + 20);
  await page.mouse.up();
  await page.click("#cam-save");
  await until(page, () => !document.getElementById("cam-list").hidden);
  const moved = await page.evaluate(map => fetch(`../../cameras/${map}`).then(r => r.json()), MAP);
  assert.notDeepEqual(moved.cameras[0].pos.slice(0, 2), pos.slice(0, 2));
  assert.equal(moved.cameras[0].id, kept.cameras[0].id);
  await page.click("#cam-list button[data-id]");
  await page.click("#cam-remove");
  await until(page, () => document.querySelectorAll("#cam-list button").length === 1);
  // A streamer's for the room alone, listed with the map's.
  await page.click('#cam-list button[data-id="new"]');
  await page.fill("#cam-name", "Ours");
  await page.check("#cam-room");
  await Promise.all([
    page.waitForRequest(r => r.method() === "POST" && /\/r\/[0-9a-f]{32}\/cameras$/.test(r.url())),
    page.click("#cam-save"),
  ]);
  await until(page, () => document.querySelector('#cam-list button[data-go="r1"]')?.textContent === "Ours");
  const shared = await page.evaluate(map => fetch(`../../cameras/${map}`).then(r => r.json()), MAP);
  assert.deepEqual(shared.cameras, [], "not the map's");
  await page.click('#cam-list button[data-id="r1"]');
  assert.ok(await page.$eval("#cam-own", el => el.hidden), "one kept stays whose it is");
  await page.click("#cam-remove");
  await until(page, () => document.querySelectorAll("#cam-list button").length === 1);
  assert.deepEqual(errors, []);
  await page.close();
});

test("a camera is given a move, which is kept with it and set off from the list", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => window.viewState?.framed);
  await page.click("#settingstoggle");
  await page.check('#settings input[data-layer="cameras"]');
  await page.click("#settingstoggle");
  await page.click('#cam-list button[data-id="new"]');
  await until(page, () => document.getElementById("cam-shows").textContent !== "");
  await page.fill("#cam-name", "Round");
  // A quarter round what it looks at, in three seconds.
  await page.click('#cam-moves button[data-move="orbit"][data-way="1"]');
  await page.click('#cam-moves button[data-move="orbit"][data-way="1"]');
  await page.fill("#cam-secs", "3");
  await page.dispatchEvent("#cam-secs", "change");
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/camera-move.png` });
  await page.click("#cam-save");
  await until(page, () => document.querySelector("#cam-list button")?.textContent === "Round");
  const kept = (await page.evaluate(map => fetch(`../../cameras/${map}`).then(r => r.json()), MAP)).cameras.at(-1);
  assert.equal(kept.move.way, "arc");
  assert.equal(kept.move.secs, 3);
  assert.ok(Math.abs(kept.move.turn - Math.PI / 2) < 1e-6);
  // Set off from the list: every moment of it, from its spot to its end.
  const [asked] = await Promise.all([
    page.waitForRequest(r => r.url().endsWith("/camera")),
    page.click("#cam-list .crow:last-of-type button[data-play]"),
  ]);
  const sent = asked.postDataJSON();
  assert.deepEqual([sent.pos, sent.aim], [kept.pos, kept.aim]);
  assert.equal(sent.path.length, 91);
  assert.deepEqual(sent.path[0], [0, ...kept.pos, ...kept.aim]);
  assert.deepEqual(sent.path[90].slice(0, 4), [3, ...kept.move.end.pos]);
  assert.equal((await asked.response())?.status(), 204);
  // Its ▶ says it is on its way, counting down, and is itself again once there.
  const play = "#cam-list .crow:last-of-type button[data-play]";
  await until(page, sel => /^[123]s$/.test(document.querySelector(sel)?.textContent ?? ""), play);
  assert.ok(await page.$eval(play, el => el.classList.contains("on")));
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/camera-playing.png` });
  await until(page, sel => document.querySelector(sel)?.textContent === "▶", play);
  assert.ok(!(await page.$eval(play, el => el.classList.contains("on"))));
  // Off on its move it is no longer at its spot: its name puts it back there, and again gives it up.
  const name = "#cam-list .crow:last-of-type button[data-go]";
  const [back] = await Promise.all([page.waitForRequest(r => r.url().endsWith("/camera")), page.click(name)]);
  assert.deepEqual(back.postDataJSON(), { pos: kept.pos, aim: kept.aim });
  await back.response();
  await page.waitForTimeout(200);
  const [off] = await Promise.all([page.waitForRequest(r => r.url().endsWith("/camera")), page.click(name)]);
  assert.equal(off.postData(), "{}");
  // Placed again, it has its move still; told to stay, no more.
  await page.click("#cam-list .crow:last-of-type button[data-id]");
  assert.equal(await page.$eval("#cam-way", el => /** @type {HTMLSelectElement} */ (el).value), "arc");
  await page.click("#cam-still");
  await page.click("#cam-save");
  await until(page, () => document.querySelector("#cam-list .crow:last-of-type button[data-play]") === null);
  const still = (await page.evaluate(map => fetch(`../../cameras/${map}`).then(r => r.json()), MAP)).cameras.at(-1);
  assert.equal(still.move, undefined);
  await page.click("#cam-list .crow:last-of-type button[data-id]");
  await page.click("#cam-remove");
  await until(page, () => document.querySelector("#cam-list .crow") === null);
  assert.deepEqual(errors, []);
  await page.close();
});

test("a flight flown in the picture after a count is a camera's move", async () => {
  const { page, errors } = await open();
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => window.viewState?.framed);
  await page.click("#settingstoggle");
  await page.check('#settings input[data-layer="cameras"]');
  await page.click("#settingstoggle");
  await page.click('#cam-list button[data-id="new"]');
  await until(page, () => document.getElementById("cam-shows").textContent !== "");
  await page.fill("#cam-name", "Flown");
  const began = await page.evaluate(() =>
    ["cam-z", "cam-aim-z"].map(id => Number(/** @type {HTMLInputElement} */ (document.getElementById(id)).value)),
  );
  const heights = () =>
    page.evaluate(() =>
      ["cam-z", "cam-aim-z"].map(id => Number(/** @type {HTMLInputElement} */ (document.getElementById(id)).value)),
    );
  // In a browser that does not let the picture hold the pointer (one inside the game), R begins a
  // flight and ends it; Esc ends it too, and a click in the picture is an aim as ever.
  await page.evaluate(() => {
    window.lockOf = HTMLCanvasElement.prototype.requestPointerLock;
    HTMLCanvasElement.prototype.requestPointerLock = () => Promise.reject(new Error("no"));
  });
  for (const stop of [() => page.keyboard.press("r"), () => page.keyboard.press("Escape")]) {
    await page.focus("#cam-view");
    await page.keyboard.press("r");
    await until(page, () => document.getElementById("cam-badge").classList.contains("rec"));
    await page.focus("#cam-view");
    await page.keyboard.down("w");
    await page.waitForTimeout(900);
    await page.keyboard.up("w");
    await stop();
    await until(page, () => document.getElementById("cam-badge").hidden);
    assert.equal(await page.textContent("#cam-rec"), "● Record (R)");
    assert.deepEqual(await heights(), began);
  }
  await page.evaluate(() => {
    HTMLCanvasElement.prototype.requestPointerLock = window.lockOf;
  });
  // Counted down to, the picture holding the pointer; let go of (Esc) while it counts, it is off.
  await page.click("#cam-rec");
  await until(page, () => /^[123]$/.test(document.getElementById("cam-badge").textContent ?? ""));
  assert.equal(await page.textContent("#cam-rec"), "Cancel (R)");
  await page.evaluate(() => document.exitPointerLock());
  await until(page, () => document.getElementById("cam-badge").hidden);
  assert.equal(await page.textContent("#cam-rec"), "● Record (R)");
  await page.click("#cam-rec");
  await until(page, () => document.getElementById("cam-badge").classList.contains("rec"));
  assert.equal(await page.textContent("#cam-rec"), "■ Stop (R)");
  // Flown forward for a second and a half, looking round with the mouse; a click ends it.
  await page.keyboard.down("w");
  await page.waitForTimeout(700);
  await page.evaluate(() =>
    document.getElementById("cam-view").dispatchEvent(new MouseEvent("mousemove", { movementX: 120, movementY: 10 })),
  );
  await page.waitForTimeout(800);
  await page.keyboard.up("w");
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/camera-flying.png` });
  await page.mouse.down();
  await page.mouse.up();
  await until(page, () => document.getElementById("cam-badge").hidden);
  assert.equal(await page.textContent("#cam-rec"), "● Record (R)");
  assert.equal(await page.evaluate(() => document.pointerLockElement), null);
  // The click that ended it aimed at nothing, and End shows where it was flown to: the flight stays.
  assert.deepEqual(await heights(), began);
  await page.click("#cam-end");
  assert.equal(await page.$eval("#cam-scrub", el => /** @type {HTMLInputElement} */ (el).value), "1000");
  assert.ok(await page.$eval("#cam-moves", el => el.hidden));
  await page.click("#cam-save");
  await until(page, () => document.querySelector("#cam-list button")?.textContent === "Flown");
  const kept = (await page.evaluate(map => fetch(`../../cameras/${map}`).then(r => r.json()), MAP)).cameras.at(-1);
  assert.ok(
    kept.move.path.length > 40 && kept.move.secs > 1.4,
    JSON.stringify([kept.move.path.length, kept.move.secs]),
  );
  const [first, last] = [kept.move.path[0], kept.move.path.at(-1)];
  assert.ok(Math.hypot(last[1] - first[1], last[2] - first[2], last[3] - first[3]) > 300, "it went somewhere");
  // The mouse turned it while the key was held: it looks another way at the end.
  const way = (/** @type {number[]} */ r) => Math.atan2(r[5] - r[2], r[4] - r[1]);
  assert.ok(Math.abs(way(last) - way(first)) > 0.1, `turned by ${way(last) - way(first)}`);
  // Its name puts the relay where the flight begins, looking as it was flown, and is marked there.
  const flownName = "#cam-list .crow:last-of-type button[data-go]";
  const [put] = await Promise.all([page.waitForRequest(r => r.url().endsWith("/camera")), page.click(flownName)]);
  const there = put.postDataJSON();
  assert.deepEqual(there.pos, kept.pos);
  assert.ok(
    there.aim.every(
      (/** @type {number} */ v, /** @type {number} */ i) =>
        Math.abs(v - first[4 + i] - (kept.pos[i] - first[1 + i])) <= 1,
    ),
    JSON.stringify([there.aim, first]),
  );
  await until(page, sel => document.querySelector(sel)?.classList.contains("on"), flownName);
  const [gone] = await Promise.all([page.waitForRequest(r => r.url().endsWith("/camera")), page.click(flownName)]);
  assert.equal(gone.postData(), "{}");
  await page.click("#cam-list .crow:last-of-type button[data-id]");
  await page.click("#cam-remove");
  await until(page, () => document.querySelector("#cam-list .crow") === null);
  assert.deepEqual(errors, []);
  await page.close();
});

test("an admin who is no streamer places a camera for the room alone", async () => {
  const { page, errors } = await open();
  await page.route("**/me", async r => {
    const answer = await r.fetch();
    await r.fulfill({ response: answer, json: { ...(await answer.json()), cameras: false } });
  });
  /** @type {string[]} */
  const shared = [];
  page.on("request", r => /\/cameras\/\w+$/.test(r.url()) && r.method() === "GET" && shared.push(r.url()));
  await page.goto(base);
  await page.click("#roomopen");
  await page.waitForURL(/\/r\/[0-9a-f]{32}\/$/, { timeout: WAIT });
  await until(page, () => window.viewState?.framed);
  await page.click("#settingstoggle");
  await page.check('#settings input[data-layer="cameras"]');
  await page.click("#settingstoggle");
  await page.click('#cam-list button[data-id="new"]');
  await page.fill("#cam-name", "Ours");
  const [asked] = await Promise.all([
    page.waitForRequest(r => r.method() === "POST" && r.url().endsWith("/cameras")),
    page.click("#cam-save"),
  ]);
  assert.match(asked.url(), /\/r\/[0-9a-f]{32}\/cameras$/);
  await until(page, () => document.querySelector("#cam-list button")?.textContent === "Ours");
  assert.deepEqual(shared, [], "the map's are not asked for");
  const kept = await page.evaluate(map => fetch(`../../cameras/${map}`).then(r => r.json()), MAP);
  assert.deepEqual(kept.cameras, [], "not the map's");
  // The relay at one not listed here, as the map's are: a button gives it back.
  const spot = { pos: [100, 200, 900], aim: [0, 0, 0] };
  await page.evaluate(
    ([to, at]) => fetch(to, { method: "POST", headers: { "Content-Type": "application/json" }, body: at }),
    [asked.url().replace(/s$/, ""), JSON.stringify(spot)],
  );
  const [back] = await Promise.all([
    page.waitForRequest(r => r.url().endsWith("/camera") && r.postData() === "{}"),
    page.click('#cam-list button[data-go="back"]'),
  ]);
  assert.equal((await back.response())?.status(), 204);
  await until(page, () => !document.querySelector('#cam-list button[data-go="back"]'));
  await page.click("#cam-list button[data-id]");
  await page.click("#cam-remove");
  await until(page, () => document.querySelectorAll("#cam-list button").length === 1);
  assert.deepEqual(errors, []);
  await page.close();
});
