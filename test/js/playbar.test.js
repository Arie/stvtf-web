import assert from "node:assert/strict";
import { test } from "node:test";
import { clock, moment, nextSpeed, playbar, speedLabel, speedNow } from "../../app/room/playbar.js";

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

test("a second click on the speed steps on from the first, before playback has it", () => {
  assert.equal(speedNow(1, null, 0), 1);
  assert.equal(speedNow(1, { speed: 2, since: 0 }, 100), 2, "asked a moment ago");
  assert.equal(speedNow(1, { speed: 2, since: 0 }, 10_000), 1, "never heard: refused, or lost");
});

const line = extra => ({
  type: "playback",
  paused: false,
  speed: 1,
  at: 0,
  length: 9,
  rounds: [],
  live: false,
  ...extra,
});
const maps = [
  { n: 0, map: "cp_a", ready: true },
  { n: 1, map: "cp_b", ready: true },
];
const room = (go = () => {}, live = () => {}) =>
  playbar({ home: new URL("http://stv.test/r/"), send() {}, live, map: go, room: true });
const status = () => document.getElementById("status").textContent;
const say = text => (document.getElementById("status").textContent = text);

test("Live shows on a map that is over, and waits for the next map's play", () => {
  const bar = room();
  bar.allow(true);
  bar.heard(line({ map: 0, over: true, maps: [maps[0]] }));
  assert.equal(bar.live.hidden, false);
  assert.equal(bar.live.disabled, false, "no next map: it follows again");
  bar.heard(line({ map: 0, over: true, maps }));
  assert.equal(bar.live.disabled, false);
  bar.heard(line({ map: 1, over: false, live: false, maps }));
  assert.equal(bar.live.hidden, true);
  const plain = room();
  plain.heard(line({}));
  assert.equal(plain.live.hidden, true, "a demo");
});

test("another map empties the timeline's marks", () => {
  const bar = room();
  bar.heard(line({ map: 0, maps }));
  bar.marks.marks = [{ at: 1, kind: "win", team: "red" }];
  bar.marks.fetched = 1;
  bar.heard(line({ map: 0, maps }));
  assert.equal(bar.marks.marks.length, 1, "the same map keeps them");
  bar.heard(line({ map: 1, maps }));
  assert.equal(bar.marks.marks.length, 0);
  assert.equal(bar.marks.fetched, 0);
});

test("without a map to go to, Live on a map that is over takes the viewer back", () => {
  /** @type {number[]} */
  const back = [];
  const bar = playbar({ home: new URL("http://stv.test/m/"), send() {}, live: n => back.push(n) });
  bar.allow(true);
  bar.heard(line({ map: 0, over: true, maps: [maps[0]] }));
  assert.equal(bar.live.hidden, false);
  assert.equal(bar.live.disabled, false, "no next map, and still back");
  /** @type {any} */ (bar.live).onclick();
  assert.equal(back.length, 1);
});

test("Live on a map that is over goes to the next map, or follows again with none", () => {
  /** @type {number[]} */
  const gone = [];
  const bar = room(n => gone.push(n));
  bar.allow(true);
  bar.heard(line({ map: 4, over: true, maps: [{ n: 4, map: "cp_a", ready: true }] }));
  assert.equal(bar.live.disabled, false);
  assert.equal(bar.live.title, "Follow live");
  bar.live.onclick();
  assert.deepEqual(gone, [4], "the room's own number");
  const both = [
    { n: 4, map: "cp_a", ready: true },
    { n: 7, map: "cp_b", ready: true },
  ];
  bar.heard(line({ map: 4, over: true, maps: both }));
  assert.equal(bar.live.title, "Next map");
  bar.live.onclick();
  assert.deepEqual(gone, [4, 7]);
  bar.allow(false);
  bar.heard(line({ map: 4, over: true, maps: both }));
  assert.equal(bar.live.disabled, true, "not an admin");
});

test("the map's status shows while paused, and gives way when it is not over", () => {
  const bar = room();
  const one = [{ n: 0, map: "cp_a", ready: true }];
  say("live");
  bar.heard(line({ map: 0, over: true, paused: true, maps: one }));
  assert.equal(status(), "Map over");
  bar.heard(line({ map: 0, over: true, paused: true, maps }));
  assert.equal(status(), "Next map ready");
  bar.heard(line({ map: 1, over: false, paused: true, maps }));
  assert.equal(status(), "paused");
  say("Map over");
  bar.heard(line({ map: 1, over: false, paused: false, live: true, maps }));
  assert.equal(status(), "live");
});

test("the clock reads 0:00 at a later map's origin", () => {
  const bar = room();
  bar.heard(line({ map: 1, paused: true, at: 5000, start: 5000, origin: 5000, length: 9000, maps }));
  assert.equal(bar.clockAt(5000), "0:00");
  assert.equal(bar.since(), 0);
  assert.equal(bar.shown.textContent, "0:00 / 0:04");
});
