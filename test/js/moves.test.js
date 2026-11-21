import assert from "node:assert/strict";
import { test } from "node:test";

import { at, flight, fromKept, orbited, rowsOf, toKept, turnTo } from "../../app/room/moves.js";

/** @type {import("../../app/room/moves.js").Spot} */
const from = { pos: [900, 0, 900], aim: [0, 0, 300] };

test("a straight move goes from its spot to its end, slow off and slow to a stop", () => {
  /** @type {import("../../app/room/moves.js").Move} */
  const move = { end: { pos: [900, 800, 900], aim: [0, 0, 300] }, secs: 8, way: "straight" };
  assert.deepEqual(at(from, move, 0).pos, [900, 0, 900]);
  assert.deepEqual(at(from, move, 4).pos, [900, 400, 900]);
  assert.deepEqual(at(from, move, 99).pos, [900, 800, 900]);
  assert.ok(at(from, move, 1).pos[1] < 100, "slow off");
});

test("an arc keeps its distance round what it looks at, the way round it was turned", () => {
  const end = orbited(orbited(from, 1), 1),
    far = (/** @type {typeof from} */ s) => Math.hypot(s.pos[0] - s.aim[0], s.pos[1] - s.aim[1]);
  assert.ok(Math.abs(end.pos[0]) < 1e-6 && Math.abs(end.pos[1] - 900) < 1e-6, "a quarter round");
  /** @type {import("../../app/room/moves.js").Move} */
  const move = { end, secs: 4, way: "arc", turn: turnTo(from, end) };
  assert.ok(Math.abs(far(at(from, move, 2)) - 900) < 1e-6);
  // The long way round, when that is how it was turned.
  const long = turnTo(from, end, -1.5 * Math.PI);
  assert.ok(Math.abs(long + 1.5 * Math.PI) < 1e-9);
  assert.ok(at(from, { ...move, turn: long }, 2).pos[1] < 0);
});

test("the relay is given each moment of a move, a thirtieth of a second apart", () => {
  /** @type {import("../../app/room/moves.js").Move} */
  const move = { end: { pos: [900, 800, 900], aim: [0, 0, 300] }, secs: 2, way: "straight" };
  const rows = rowsOf(from, move);
  assert.equal(rows.length, 61);
  assert.deepEqual(rows[0], [0, 900, 0, 900, 0, 0, 300]);
  assert.deepEqual(rows[60], [2, 900, 800, 900, 0, 0, 300]);
  assert.equal(new Set(rows.map(r => r[0])).size, 61, "each its own moment");
  assert.deepEqual(rowsOf(from), [[0, 900, 0, 900, 0, 0, 300]]);
});

test("a flight is kept as its moments and comes back as it was", () => {
  const raw = Array.from({ length: 40 }, (_, i) => ({
    t: i / 20,
    pos: /** @type {[number, number, number]} */ ([i * 10, 0, 900]),
    aim: /** @type {[number, number, number]} */ ([0, 500, 300]),
  }));
  const flown = flight(raw, 0.25);
  assert.ok(flown?.path && flown.path.length > 50);
  const kept = toKept(flown);
  assert.ok(!("raw" in kept) && kept.path?.[0].length === 7);
  const back = fromKept(JSON.parse(JSON.stringify(kept)));
  assert.equal(back.path?.length, flown.path.length);
  assert.ok(Math.abs(at(from, back, 1).pos[0] - at(from, flown, 1).pos[0]) < 0.1);
  assert.equal(flight(raw.slice(0, 5), 0.25), null, "too short to be one");
});

test("a flight goes from where its camera is, moved since it was flown", () => {
  const raw = Array.from({ length: 40 }, (_, i) => ({
    t: i / 20,
    pos: /** @type {[number, number, number]} */ ([i * 10, 0, 900]),
    aim: /** @type {[number, number, number]} */ ([0, 500, 300]),
  }));
  const flown = flight(raw, 0.25);
  assert.ok(flown?.path);
  /** @type {import("../../app/room/moves.js").Spot} */
  const moved = { pos: [100, 200, 950], aim: [100, 700, 350] };
  const near = (/** @type {number[]} */ a, /** @type {number[]} */ b) => a.every((v, i) => Math.abs(v - b[i]) < 1e-6);
  const begins = at(moved, flown, 0);
  assert.ok(near(begins.pos, moved.pos) && near(begins.aim, moved.aim), "it begins where the camera is");
  const [there, here] = [at({ pos: raw[0].pos, aim: raw[0].aim }, flown, 1), at(moved, flown, 1)];
  assert.ok(Math.abs(here.pos[1] - there.pos[1] - 200) < 1e-6 && Math.abs(here.aim[2] - there.aim[2] - 50) < 1e-6);
  assert.deepEqual(rowsOf(moved, flown)[0], [0, 100, 200, 950, 100, 700, 350]);
});
