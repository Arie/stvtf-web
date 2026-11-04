import assert from "node:assert/strict";
import { test } from "node:test";
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
import { closingIn, demostfAlias, fightingPlayers, sameLayout } from "../../app/view.js";

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

test("the fight is the biggest group and any other of at least three", () => {
  const at = (...xs) => xs.map(x => ({ pos: [x, 0] }));
  const fighting = players => fightingPlayers(players).map(p => p.pos[0]);
  // 1500 units links two players, and a chain of links is one group.
  assert.deepEqual(fighting(at(0, 1000, 2000, 9000, 10000, 20000)), [0, 1000, 2000]);
  assert.deepEqual(
    fighting(at(0, 1000, 2000, 3000, 9000, 10000, 11000, 15000, 16000)),
    [0, 1000, 2000, 3000, 9000, 10000, 11000],
  );
  assert.deepEqual(fighting(at(0, 5000)), [0, 5000], "alone, each is as big as any");
  assert.deepEqual(fighting(at(0, 1499, 5000)), [0, 1499]);
  assert.deepEqual(fighting(at(0, 1500, 5000)), [0, 1500, 5000], "just too far to link");
});

test("players fresh from spawn do not make their group the fight", () => {
  const at = (...xs) => xs.map(x => ({ id: x, pos: [x, 0] }));
  const fighting = (players, fresh) => fightingPlayers(players, p => fresh.includes(p.id)).map(p => p.pos[0]);
  // Two still fighting on one side, a wave of four just out of spawn on the other.
  assert.deepEqual(fighting(at(0, 1000, 9000, 9100, 9200, 9300), [9000, 9100, 9200, 9300]), [0, 1000]);
  assert.deepEqual(fighting(at(0, 9000, 9100, 9200), [9000, 9100, 9200]), [0], "even a lone fighter");
  assert.deepEqual(
    fighting(at(0, 100, 200, 1000, 9000, 9100), [1000]),
    [0, 100, 200, 1000],
    "a fresh one in the fight is in it",
  );
  assert.deepEqual(fighting(at(0, 100, 9000), [0, 100, 9000]), [0, 100], "everyone fresh: by size again");
});

test("a loner closes in on the fight only by moving", () => {
  const fight = [{ pos: [0, 0] }];
  assert.equal(closingIn([5000, 0], [4000, 0], fight, 300), true, "walking up to the fight");
  assert.equal(closingIn([5000, 0], [5000, 0], fight, 300), false, "standing still");
  assert.equal(closingIn([5000, 0], [4800, 0], fight, 300), false, "not far enough");
  // The fight coming to them is not them closing in.
  assert.equal(closingIn([5000, 0], [5000, 0], [{ pos: [3000, 0] }], 300), false);
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
