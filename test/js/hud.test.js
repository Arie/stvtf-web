import assert from "node:assert/strict";
import { test } from "node:test";
import { $ } from "../../app/core/core.js";
import { mmss, renderBoard } from "../../app/panel/hud.js";
import { renderStopwatch, stopwatchParts } from "../../app/panel/stopwatch.js";

test("the scoreboard escapes names and ranks each team by points", () => {
  const player = (id, team, name, points) => ({
    id,
    team,
    name,
    class: "scout",
    alive: true,
    ping: 20,
    score: { points, kills: 1 },
  });
  renderBoard(
    [player(1, "red", "<img src=x onerror=alert(1)>", 5), player(2, "red", "top", 9), player(3, "blue", "alone", 1)],
    { score: { red: 2 } },
  );
  const board = $("board").innerHTML;
  assert.ok(!board.includes("<img src=x") && board.includes("&lt;img src=x"), board);
  assert.ok(board.indexOf("top") < board.indexOf("&lt;img"), "more points first");
  assert.ok(board.indexOf("alone") < board.indexOf("top"), "BLU on the left");
  assert.ok(board.includes("<b>RED</b><span>2</span><i>2 players</i>"), board);
  assert.ok(board.includes("<b>BLU</b><span>0</span><i>1 players</i>"), board);
});

test("times read as minutes and seconds", () => {
  assert.equal(mmss(0), "0:00");
  assert.equal(mmss(90.9), "1:30");
  assert.equal(mmss(600), "10:00");
});

test("the stopwatch reads like TF2's panel to a spectator", () => {
  const sw = { attackers: "blue", setting: true, state: "unset", time: 0, paused: true, points: 0 };
  assert.deepEqual(stopwatchParts(sw, 5), { label: "CAPTURE TIME NOT YET SET", time: "", points: "" });
  const set = { ...sw, state: "running", time: 92.8, points: 1 };
  assert.deepEqual(stopwatchParts(set, 5), { label: "", time: "1:33", points: "1 POINT" }, "stands still while set");
  const beat = { ...set, setting: false, paused: false, time: 100, points: 3 };
  assert.deepEqual(stopwatchParts(beat, 10.5), { label: "RED'S TIME", time: "1:30", points: "3 POINTS" });
  const over = { ...beat, state: "overtime", time: 0, paused: true, points: 2 };
  assert.equal(stopwatchParts(over, 0).label, "ATTACKERS NEED 2 POINTS TO WIN");
});

test("the stopwatch shows only in stopwatch", () => {
  const sw = { attackers: "red", setting: false, state: "running", time: 61, paused: false, points: 1 };
  renderStopwatch({ tick: 100, stopwatch: sw }, 100 + 67);
  assert.equal($("stopwatch").hidden, false);
  assert.equal($("stopwatch").innerHTML, "<span>BLU'S TIME</span><b>1:00</b><i>1 POINT</i>", "run on to the tick");
  renderStopwatch({ tick: 200 }, 200);
  assert.equal($("stopwatch").hidden, true);
});
