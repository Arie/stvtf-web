import assert from "node:assert/strict";
import { test } from "node:test";
import {
  byClass,
  msPerTick,
  observeTick,
  optOn,
  parseMessage,
  renderTick,
  setClockRate,
  setTickInterval,
  state,
  statusText,
} from "../../app/core.js";
import { command, steamid64 } from "../../app/embed.js";
import { layerOn } from "../../app/layers.js";
import { labelCorner } from "../../app/spawns.js";
import { demostfAlias, sameLayout, screenOffset, teamsTurn, toScreen } from "../../app/view.js";

test("players sort by class, then name whatever its case, then id", () => {
  const rows = [
    { id: 1, class: "medic", name: "a" },
    { id: 2, class: "scout", name: "b" },
    { id: 3, class: "scout", name: "A" },
    { id: 5, class: "soldier", name: "x" },
    { id: 4, class: "soldier", name: "x" },
    { id: 6, class: "unknown", name: "a" },
    { id: 7, class: "made_up", name: "a" },
  ];
  assert.deepEqual(
    rows.sort(byClass).map(r => r.id),
    [3, 2, 4, 5, 1, 6, 7],
  );
});

test("the map's overview may be of another version of it", () => {
  const known = { cp_process_f12: {}, koth_product: {}, cp_gullywash_final1: {} };
  for (const [map, want] of [
    ["cp_process_f12", "cp_process_f12"],
    ["cp_process", "cp_process_f12"],
    ["cp_process_f13", "cp_process_f12"],
    ["koth_product_rcx", "koth_product"],
    ["cp_gullywash_final1", "cp_gullywash_final1"],
    ["cp_gullywash_f9", "cp_gullywash_f6"],
    ["cp_nope", null],
  ])
    assert.equal(demostfAlias(map, known), want, map);
});

test("an overview is used only if it covers the map's layout", () => {
  const overview = { boundary_min: { x: 0, y: 0 }, boundary_max: { x: 100, y: 100 } };
  const covering = (x1, y1, x0 = 0, y0 = 0) =>
    sameLayout(overview, {
      bounds: [
        [x0, y0],
        [x1, y1],
      ],
    });
  assert.equal(covering(100, 100), true);
  assert.equal(covering(100, 50), true, "exactly half");
  assert.equal(covering(100, 49), false);
  assert.equal(covering(300, 300, 200, 200), false, "nowhere near");
  assert.equal(sameLayout(overview, null), true, "no geometry yet");
});

test("the map turns so blue's spawns are on the left and red's on the right", () => {
  const spawns = (blue, red) => [
    { pos: blue, team: "blue" },
    { pos: [blue[0] + 100, blue[1]], team: "blue" },
    { pos: red, team: "red" },
  ];
  const blueLeft = (pts, rot) => {
    Object.assign(state.view, { cx: 0, cy: 0, scale: 1, rot });
    const [bx] = toScreen(...pts[0].pos),
      [rx] = toScreen(...pts[2].pos);
    return bx < rx;
  };
  // Gullywash: blue in the east, red in the west. Granary: blue north, red south.
  for (const [name, pts] of [
    ["gullywash", spawns([3385, -1270], [-2170, 652])],
    ["granary", spawns([-887, 3024], [-2186, -2968])],
    ["already left to right", spawns([-3000, 0], [3000, 200])],
  ]) {
    const rot = teamsTurn(pts);
    assert.equal(Math.round(rot / (Math.PI / 2)) * (Math.PI / 2), rot, `${name}: a quarter turn`);
    assert.ok(blueLeft(pts, rot), `${name}: blue on the left at ${rot}`);
  }
  assert.equal(teamsTurn(spawns([-3000, 0], [3000, 0])), 0);
  assert.equal(teamsTurn([{ pos: [0, 0], team: "red" }]), null, "one team only");
});

test("a spawn room's label keeps its corner while the view pans", () => {
  const r = { min: [-500, -300], max: [700, 400] };
  for (const rot of [0, Math.PI / 2, Math.PI, -Math.PI / 2]) {
    const picked = new Set();
    for (let cx = -3000; cx < 3000; cx += 0.731) {
      Object.assign(state.view, { cx, cy: 123.4, scale: 0.3, rot });
      const corners = [
        [r.min[0], r.min[1]],
        [r.max[0], r.min[1]],
        [r.max[0], r.max[1]],
        [r.min[0], r.max[1]],
      ].map(([x, y]) => toScreen(x, y));
      picked.add(corners.indexOf(labelCorner(corners)));
    }
    assert.equal(picked.size, 1, `one corner at ${rot}`);
  }
});

test("a screen offset is the same map offset whichever way the view is turned", () => {
  const was = { ...state.view };
  for (const rot of [0, Math.PI / 2, Math.PI, -Math.PI / 2, 0.3]) {
    Object.assign(state.view, { cx: 100, cy: 50, scale: 0.5, rot });
    const [centreX, centreY] = toScreen(100, 50);
    const [dx, dy] = screenOffset(30, -20, 0.5);
    const [x, y] = toScreen(100 + dx, 50 + dy);
    assert.ok(
      Math.abs(x - centreX - 30) < 1e-9 && Math.abs(y - centreY + 20) < 1e-9,
      `rot ${rot}: ${x - centreX}, ${y - centreY}`,
    );
  }
  Object.assign(state.view, was);
});

test("the tick clock jumps on a stall and otherwise eases", () => {
  // Behind the newest snapshot by the spacing (4 ticks) and the jitter margin (105 ms, 7 ticks).
  observeTick(1000);
  assert.ok(Math.abs(renderTick() - 989) < 1, `${renderTick()}`);
  observeTick(5000);
  assert.ok(Math.abs(renderTick() - 4989) < 1, `${renderTick()}`);
  observeTick(5010);
  const eased = renderTick();
  assert.ok(eased > 4988 && eased < 4990, `${eased} moves a little, not ten ticks`);
});

/** @param {number} ms */
const spin = ms => {
  const until = performance.now() + ms;
  while (performance.now() < until);
};

test("the tick clock runs at a room's playback rate, without a jump", () => {
  observeTick(1000);
  const before = renderTick();
  setClockRate(2);
  assert.ok(Math.abs(renderTick() - before) < 0.5, "no jump");
  spin(60);
  // 60 ms at twice 15 ms a tick.
  assert.ok(Math.abs(renderTick() - before - 8) < 1, `${renderTick() - before}`);
  setClockRate(1);
  assert.equal(state.paused, false);
});

test("a paused clock stands still and shows a sought snapshot as it is", () => {
  observeTick(2000);
  setClockRate(0);
  assert.equal(state.paused, true);
  const still = renderTick();
  spin(20);
  assert.equal(renderTick(), still);
  observeTick(9000);
  assert.ok(Math.abs(renderTick() - 9000) < 1e-9, `${renderTick()}`);
  setClockRate(1);
});

test("a tick interval comes only from a server that gives one", () => {
  setTickInterval(0);
  assert.equal(msPerTick, 15);
  setTickInterval(1 / 128);
  assert.equal(msPerTick, 7.8125);
  setTickInterval(0.015);
});

test("a toggle without an option in the address is its default", () => {
  assert.equal(optOn("names"), true);
  assert.equal(optOn("names", false), false);
});

test("the Steam ids an embedding page gets are real ones", () => {
  assert.equal(steamid64("[U:1:231502]"), "76561197960497230");
  assert.equal(steamid64("[U:1:1]"), "76561197960265729");
  for (const id of ["BOT", "", null, "[U:1:x]", "[A:1:1]"]) assert.equal(steamid64(id), null, id);
});

test("an embedding page can follow a player by any id, and stop", () => {
  state.lastPlayers = [
    { id: 3, steamid: "[U:1:231502]" },
    { id: 4, steamid: "[U:1:1]" },
  ];
  command({ type: "follow", steamid: "[U:1:231502]" });
  assert.equal(state.follow, 3);
  command({ type: "follow", steamid64: "76561197960265729" });
  assert.equal(state.follow, 4);
  command({ type: "follow", id: 3 });
  assert.equal(state.follow, 3);
  command({ type: "follow", id: 99 });
  assert.equal(state.follow, null);
});

test("an embedding page can switch known layers only", () => {
  command({ type: "layer", name: "caps", on: false });
  assert.equal(layerOn("caps"), false);
  command({ type: "layer", name: "caps", on: true });
  assert.equal(layerOn("caps"), true);
  command({ type: "layer", name: "made_up", on: true });
  assert.equal(layerOn("made_up"), undefined);
});

test("the sidebar keeps one word while SourceTV signs on, however often it is dropped", () => {
  const run = (shown, ...lines) =>
    lines.map(m => (shown = statusText(shown, typeof m === "string" ? { state: m } : m)[0]));
  const drop = { state: "disconnected", reason: "timed out" };
  // A server starting up: in and dropped during signon, then failing to connect, then live.
  assert.deepEqual(
    run(
      "waiting for data",
      "connecting",
      "new",
      "prespawn",
      "spawn",
      drop,
      "connecting",
      { state: "connect_failed", reason: "no answer" },
      "connecting",
      "new",
      "spawn",
      "full",
    ),
    [
      "connecting",
      "connecting",
      "connecting",
      "connecting",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "live",
    ],
  );
  // A map change, signing on again, dropped once on the way.
  assert.deepEqual(run("live", "changelevel", "map_change", "new", "prespawn", drop, "connecting", "spawn", "full"), [
    "map changing",
    "map changing",
    "map changing",
    "map changing",
    "map changing",
    "map changing",
    "map changing",
    "live",
  ]);
  // Dropped while live.
  assert.deepEqual(run("no data", drop, "connecting", "new", "full"), [
    "reconnecting",
    "reconnecting",
    "reconnecting",
    "live",
  ]);
  assert.deepEqual(
    statusText("connecting", drop),
    ["waiting for SourceTV", "timed out"],
    "the reason goes in the tooltip",
  );
  assert.deepEqual(statusText("connecting", { state: "gave_up", reason: "wrong password" }), [
    "gave up: wrong password",
    "wrong password",
  ]);
});

test("stream messages lose invisible characters but keep emoji joiners", () => {
  const raw = JSON.stringify({
    type: "chat",
    name: "a\u202eb\u200bc",
    text: "\u{1F469}\u200d\u{1F4BB}",
    players: [{ name: "\u2066x\u2069" }],
  });
  assert.deepEqual(parseMessage(raw), {
    type: "chat",
    name: "abc",
    text: "\u{1F469}\u200d\u{1F4BB}",
    players: [{ name: "x" }],
  });
  assert.deepEqual(parseMessage(raw), parseMessage(raw));
  assert.equal(parseMessage('{"name":"a\\u202Eb"}').name, "ab");
  assert.equal(parseMessage('{"name":"plain"}').name, "plain");
});
