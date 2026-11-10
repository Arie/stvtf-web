import assert from "node:assert/strict";
import { test } from "node:test";
import { $ } from "../../app/core/core.js";
import { chat, esc, feedEvent, killiconsLoaded } from "../../app/panel/feed.js";

const HOSTILE = "<img src=x onerror=alert(1)>";
const PLAYERS = {
  3: { id: 3, name: "can of tuna", team: "red" },
  4: { id: 4, name: "silvo", team: "blue" },
  6: { id: 6, name: HOSTILE, team: `red"><script>alert(1)</script>` },
};
const p = id => PLAYERS[id];
const lastRow = box => $(box).children[0]?.innerHTML;
const feedRow = ev => {
  $("feed").replaceChildren();
  feedEvent(ev, p);
  return lastRow("feed");
};
const chatRow = ev => {
  $("chat").replaceChildren();
  chat(ev, p);
  return lastRow("chat");
};
const safe = html => {
  assert.ok(html, "a row is drawn");
  assert.ok(!html.includes("<img") && !html.includes("<script"), html);
  return html;
};

test("esc leaves no markup", () => {
  assert.equal(esc(`<a href="x">&`), "&lt;a href=&quot;x&quot;&gt;&amp;");
  assert.equal(esc(null), "");
});

test("the feed does not trust what a server sends", () => {
  for (const ev of [
    { type: "kill", victim: 6, attacker: 4, weapon: HOSTILE },
    { type: "kill", victim: 4, attacker: 6, weapon: "unknown" },
    { type: "capture_blocked", blocker: 3, name: HOSTILE, point: 0 },
    { type: "point_captured", team: "red", name: HOSTILE, point: 0 },
    { type: "building_destroyed", owner: 3, attacker: 4, object: HOSTILE },
    { type: "sapped", player: 3, owner: 4, object: HOSTILE },
    { type: "point_locked", name: HOSTILE, point: 0, locked: true },
    { type: "vote_options", options: [HOSTILE, "No"] },
    { type: "vote_cast", player: 3, option: 0 },
    { type: "vote_started", caller: 3, issue: HOSTILE, details: HOSTILE },
    { type: "vote_started", caller: 3, issue: "#TF_vote_kick_player_other", details: HOSTILE },
    { type: "player_leave", name: HOSTILE },
    { type: "map", map: HOSTILE },
  ])
    safe(feedRow(ev));
});

test("chat does not trust what a server sends", () => {
  for (const ev of [
    { type: "chat", player: 6, text: "hi" },
    { type: "chat", player: 3, text: HOSTILE },
    { type: "chat", name: HOSTILE, text: "hi" },
    { type: "chat", text: HOSTILE },
    {
      type: "chat",
      player: 3,
      parts: [
        { c: "team", t: HOSTILE },
        { c: "FF0000", t: HOSTILE },
        { c: null, t: HOSTILE },
      ],
    },
    { type: "server_text", text: HOSTILE },
    { type: "voice_command", player: 6, menu: 0, item: 0 },
  ])
    safe(chatRow(ev));
});

test("a chat line names its sender by id, for a note on them", () => {
  assert.ok(chatRow({ type: "chat", player: 3, text: "hi" }).includes('class="who" data-id="3"'));
});

test("a chat colour is only ever a colour", () => {
  assert.ok(
    chatRow({ type: "chat", player: 3, parts: [{ c: "FF0000", t: "red" }] }).includes(
      '<span style="color:#FF0000">red</span>',
    ),
  );
  const row = chatRow({ type: "chat", player: 3, parts: [{ c: 'FF0000;background:url(//evil)"', t: "x" }] });
  assert.ok(!row.includes("style=") && !row.includes("evil"), row);
});

test("a vote's issue cannot reach Object's own properties", () => {
  const row = feedRow({ type: "vote_started", caller: 3, issue: "constructor", details: "x" });
  assert.ok(row.includes("called a vote: constructor x"), row);
});

test("kills within ten seconds count up a streak, and a feign does not", () => {
  const kill = (tick, more = {}) =>
    feedRow({ type: "kill", tick, attacker: 4, victim: 3, weapon: "scattergun", ...more });
  assert.ok(!kill(1000).includes("2K"));
  assert.ok(kill(1500).includes("2K"));
  assert.ok(!kill(1600, { feign: true }).includes("3K"));
  assert.ok(kill(1700).includes("3K"));
  // 15 ms a tick: 10 s is 667 ticks.
  assert.ok(!kill(1700 + 668).includes("K</span>"));
});

test("an airshot is a projectile hitting someone in the air", () => {
  const air = weapon =>
    feedRow({ type: "kill", tick: 1, attacker: 3, victim: 4, weapon, airborne: true }).includes("AIRSHOT");
  assert.ok(air("quake_rl"));
  assert.ok(!air("scattergun"));
});

test("a known weapon gets its icon, an unknown one its name", async () => {
  await killiconsLoaded;
  assert.ok(
    feedRow({ type: "kill", tick: 1, victim: 4, weapon: "quake_rl", crit: true }).includes(
      'class="killicon killicon-quake_rl crit"',
    ),
  );
  assert.ok(
    feedRow({ type: "kill", tick: 1, victim: 4, weapon: "made_up" }).includes('<span class="w">made_up</span>'),
  );
});

test("a player who left shows as unknown rather than breaking the row", () => {
  assert.ok(
    feedRow({ type: "kill", tick: 1, attacker: 3, victim: 99, weapon: "x" }).includes('<span class="w">?</span>'),
  );
});

test("what is not news draws no row", () => {
  assert.equal(feedRow({ type: "medic_death", medic: 3, attacker: 4, drop: false }), undefined);
  assert.equal(feedRow({ type: "team_change", player: 3, team: "red", old_team: "blue" }), undefined, "a swap");
  assert.equal(feedRow({ type: "team_change", player: 3, team: "red", old_team: "none", disconnect: true }), undefined);
  assert.equal(feedRow({ type: "team_change", player: 3, team: "spectator", old_team: "none" }), undefined);
  assert.equal(feedRow({ type: "scores", red: 1, blue: 0, game_over: false }), undefined);
  assert.equal(feedRow({ type: "made_up" }), undefined);
  assert.equal(chatRow({ type: "server_text", text: "#TF_token" }), undefined, "an untranslated token");
  assert.equal(chatRow({ type: "voice_command", player: 3, menu: 9, item: 9 }), undefined);
});

test("rows name the point, and fall back on its number", () => {
  assert.ok(feedRow({ type: "point_captured", team: "red", name: "Mid", point: 1 }).includes("RED captured Mid"));
  assert.ok(
    feedRow({ type: "point_captured", team: "blue", name: "#cp_token", point: 1 }).includes("BLU captured point 2"),
  );
});

test("a round's end gives the score once it is known", () => {
  feedRow({ type: "scores", red: 2, blue: 1, game_over: false });
  assert.ok(feedRow({ type: "round_win", team: "red" }).includes("RED wins the round · BLU 1 – 2 RED"));
});

test("the feed keeps only the latest rows", () => {
  $("feed").replaceChildren();
  for (let i = 0; i < 70; i++) feedEvent({ type: "player_leave", name: `p${i}` }, p);
  assert.equal($("feed").children.length, 60);
  assert.ok($("feed").children[0].innerHTML.includes("p69"));
  assert.ok($("feed").lastChild.innerHTML.includes("p10"));
});
