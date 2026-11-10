import assert from "node:assert/strict";
import { test } from "node:test";
import { clock, moment, nextSpeed, speedLabel } from "../../app/room/playbar.js";

test("a moment reads back what the clock writes", () => {
  for (const ms of [0, 754_000, 3_723_000]) assert.equal(moment(clock(ms)), ms);
  assert.equal(moment("90"), 90_000, "seconds");
  for (const bad of [null, "", "1:2", "12:34:56:78", "x", "-5"]) assert.equal(moment(bad), null, String(bad));
});

test("the speed button steps up through the speeds and round again", () => {
  assert.equal(nextSpeed([1, 2, 4], 1), 2);
  assert.equal(nextSpeed([1, 2, 4], 4), 1, "round again");
  assert.equal(nextSpeed([1, 2, 4], 0.5), 1, "from one it does not have");
  assert.deepEqual([0.25, 0.5, 1, 4].map(speedLabel), ["¼×", "½×", "1×", "4×"]);
});
