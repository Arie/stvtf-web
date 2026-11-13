import assert from "node:assert/strict";
import { test } from "node:test";
import { mapNames, mapState } from "../../app/room/maps.js";

test("the bar says the map is over, then that the next is ready", () => {
  const maps = [
    { n: 0, map: "cp_a", ready: true },
    { n: 1, map: "cp_b", ready: false },
  ];
  const p = {
    type: "playback",
    paused: false,
    speed: 1,
    at: 9,
    length: 9,
    rounds: [],
    live: false,
    map: 0,
    over: true,
    maps,
  };
  assert.deepEqual(mapState(p), { status: "Map over", next: null });
  maps[1].ready = true;
  assert.deepEqual(mapState(p), { status: "Next map ready", next: 1 });
  assert.deepEqual(mapState({ ...p, map: 1, over: false, live: true }), { status: "", next: null });
  assert.deepEqual(
    mapState({ ...p, maps: undefined, map: undefined, over: undefined }),
    { status: "", next: null },
    "a demo: no maps",
  );
});

test("the next map is the newest ready one after the room's, numbers not indexes", () => {
  const maps = [
    { n: 2, map: "a", ready: true },
    { n: 5, map: "b", ready: true },
    { n: 6, map: "c", ready: true },
  ];
  const p = {
    type: "playback",
    paused: false,
    speed: 1,
    at: 0,
    length: 0,
    rounds: [],
    live: false,
    map: 2,
    over: true,
    maps,
  };
  assert.equal(mapState(p).next, 6);
  assert.equal(mapState({ ...p, map: 5 }).next, 6);
  assert.equal(mapState({ ...p, map: 6 }).next, null);
});

test("the same map twice is told apart, the first staying bare", () => {
  assert.deepEqual(mapNames(["koth_a", "koth_b", "koth_a", "koth_a"]), ["koth_a", "koth_b", "koth_a 2", "koth_a 3"]);
  assert.deepEqual(mapNames(["cp_a", "cp_b"]), ["cp_a", "cp_b"]);
  assert.deepEqual(mapNames([]), []);
});
