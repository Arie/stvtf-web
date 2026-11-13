import assert from "node:assert/strict";
import { test } from "node:test";
import { timeline } from "../../app/room/timeline.js";

test("a marks fetch belongs to one visit of a map: A, B, A again drops the first visit's late answer", async t => {
  const real = globalThis.fetch;
  t.after(() => (globalThis.fetch = real));
  /** @type {((r: unknown) => void)[]} */
  const waiting = [];
  globalThis.fetch = () => new Promise(res => waiting.push(r => res({ json: async () => r })));
  const tl = timeline({
    track: document.getElementById("t"),
    home: new URL("http://stv.test/r/"),
    clock: String,
    length: () => 9,
    jump: null,
  });
  const heard = load => tl.heard({ rounds: [], marks: 1, load });
  const answer = (i, at) => waiting[i]({ marks: [{ at, kind: "win", team: "red" }], all: 1 });
  heard(0);
  heard(1);
  heard(0);
  assert.equal(waiting.length, 3);
  answer(0, 1);
  answer(2, 2);
  await new Promise(r => setImmediate(r));
  answer(1, 3);
  await new Promise(r => setImmediate(r));
  assert.deepEqual(
    tl.marks.map(m => m.at),
    [2],
    "only the visit that is current adds",
  );
});
