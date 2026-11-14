import assert from "node:assert/strict";
import { test } from "node:test";
import { LEAST_MS, following, panned, pinched, wheel, within, zoomed } from "../../app/room/zoom.js";

/** @type {[number, number]} */
const whole = [1_000_000, 4_000_000];

test("the timeline zooms about the pointer, and out to the whole again", () => {
  const at = 1_300_000;
  const one = zoomed(null, whole, at, 2);
  assert.deepEqual(one, [1_150_000, 2_650_000], "half of it, the pointer's time where it was");
  const two = zoomed(one, whole, at, 2);
  assert.equal((at - two[0]) / (two[1] - two[0]), 0.1, "still a tenth in");
  assert.equal(zoomed(two, whole, at, 1 / 8), null, "all of it: not zoomed");
  let deep = one;
  for (let i = 0; i < 40; i++) deep = zoomed(deep, whole, at, 2);
  assert.equal(deep[1] - deep[0], LEAST_MS, "no closer than the least");
  assert.equal(zoomed(null, [0, 5000], 100, 4), null, "a timeline shorter than that does not zoom");
});

test("a zoomed timeline pans within the whole, and goes after playback that left it", () => {
  /** @type {[number, number]} */
  const view = [2_000_000, 2_500_000];
  assert.deepEqual(panned(view, whole, 100_000), [2_100_000, 2_600_000]);
  assert.deepEqual(panned(view, whole, -5_000_000), [1_000_000, 1_500_000], "stops at the start");
  assert.deepEqual(panned(view, whole, 5_000_000), [3_500_000, 4_000_000], "and at the end");
  assert.equal(panned(null, whole, 100), null);
  assert.equal(following(view, whole, 2_250_000), view, "in it: left be");
  assert.deepEqual(following(view, whole, 3_000_000), [2_925_000, 3_425_000], "near its left");
  assert.deepEqual(following(view, whole, 3_990_000), [3_500_000, 4_000_000]);
  assert.deepEqual(within([0, 9_000_000], whole), null);
});

test("the wheel zooms, sideways or with shift it pans", () => {
  assert.ok(wheel({ deltaX: 0, deltaY: -100, shiftKey: false }).factor > 1, "up: in");
  assert.ok(wheel({ deltaX: 0, deltaY: 100, shiftKey: false }).factor < 1);
  assert.deepEqual(wheel({ deltaX: 0, deltaY: 300, shiftKey: true }), { factor: 0, pan: 0.5 });
  assert.deepEqual(wheel({ deltaX: -60, deltaY: 3, shiftKey: false }), { factor: 0, pan: -0.1 });
  const lines = wheel({ deltaX: 0, deltaY: -3, shiftKey: false, deltaMode: 1 });
  assert.equal(lines.factor, wheel({ deltaX: 0, deltaY: -100, shiftKey: false }).factor, "a notch in lines");
});

test("a pinch keeps the time between two fingers between them", () => {
  // Fingers at 40% and 60% spread to 20% and 80%: three times closer, about the middle.
  const round = (/** @type {number[] | null} */ v) => v?.map(Math.round);
  assert.deepEqual(round(pinched(whole, whole, [0.4, 0.6], [0.2, 0.8])), [2_000_000, 3_000_000]);
  // Both moved right by a tenth, no further apart: panned back by a tenth.
  assert.deepEqual(round(pinched([2_000_000, 3_000_000], whole, [0.4, 0.6], [0.5, 0.7])), [1_900_000, 2_900_000]);
  assert.equal(pinched([2_000_000, 3_000_000], whole, [0.2, 0.8], [0.45, 0.55]), null, "pinched shut: the whole");
});
