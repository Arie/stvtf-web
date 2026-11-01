import assert from "node:assert/strict";
import { test } from "node:test";
import { $ } from "../../app/core.js";
import { mmss, renderBoard } from "../../app/hud.js";

test("the scoreboard escapes names and ranks each team by points", () => {
  const player = (id, team, name, points) => ({ id, team, name, class: "scout", alive: true, ping: 20, score: { points, kills: 1 } });
  renderBoard([player(1, "red", "<img src=x onerror=alert(1)>", 5), player(2, "red", "top", 9), player(3, "blue", "alone", 1)], { score: { red: 2 } });
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
