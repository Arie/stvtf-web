import assert from "node:assert/strict";
import { test } from "node:test";
import { behindLines, closingIn } from "../../app/camera.js";
import {
  byClass,
  msPerTick,
  observeTick,
  optOn,
  parseMessage,
  renderTick,
  setTickInterval,
  state,
  statusText,
} from "../../app/core.js";
import { command, steamid64 } from "../../app/embed.js";
import { layerOn } from "../../app/layers.js";
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

test("behind enemy lines is past more than half of the other team", () => {
  const at = (...ps) => ps.map(([x, y, team]) => ({ id: `${team}${x},${y}`, pos: [x, y], team }));
  const behind = players => behindLines(players).map(p => p.id);
  // Red holds the left, blue the right; one red scout has slipped round to blue's back line.
  const flank = at(
    [0, 0, "red"],
    [200, 300, "red"],
    [8000, 900, "red"],
    [5000, 0, "blue"],
    [5200, 400, "blue"],
    [6000, -300, "blue"],
  );
  assert.deepEqual(behind(flank), ["red8000,900"]);
  assert.deepEqual(behind(at([0, 0, "red"], [5000, 0, "blue"])), [], "two fronts facing each other");
  assert.deepEqual(behind(at([0, 0, "red"], [100, 0, "red"])), [], "one team alone");
});

test("a loner closes in on the fight only by moving", () => {
  const fight = [{ pos: [0, 0] }];
  assert.equal(closingIn([5000, 0], [4000, 0], fight, 300), true, "walking up to the fight");
  assert.equal(closingIn([5000, 0], [5000, 0], fight, 300), false, "standing still");
  assert.equal(closingIn([5000, 0], [4800, 0], fight, 300), false, "not far enough");
  // The fight coming to them is not them closing in.
  assert.equal(closingIn([5000, 0], [5000, 0], [{ pos: [3000, 0] }], 300), false);
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
