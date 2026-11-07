import assert from "node:assert/strict";
import { test } from "node:test";
import { clock, moment } from "../../app/playbar.js";

test("a moment reads back what the clock writes", () => {
  for (const ms of [0, 754_000, 3_723_000]) assert.equal(moment(clock(ms)), ms);
  assert.equal(moment("90"), 90_000, "seconds");
  for (const bad of [null, "", "1:2", "12:34:56:78", "x", "-5"]) assert.equal(moment(bad), null, String(bad));
});
