import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { $, state } from "../../app/core.js";
import { CHAT_TYPES, chat, feedEvent } from "../../app/feed.js";

// The lines Rust checks against stvtf (`the_sample_lines_are_what_stvtf_sends`), so a field the page
// reads that stvtf does not send shows up as a hole in the row.
const samples = readFileSync(new URL("../../testdata/lines.jsonl", import.meta.url), "utf8")
  .trim()
  .split("\n")
  .map(l => JSON.parse(l));
const PLAYERS = {
  3: { id: 3, name: "can of tuna", team: "red" },
  4: { id: 4, name: "silvo", team: "blue" },
  5: { id: 5, name: "medic", team: "red" },
};
const p = id => PLAYERS[id];
// Sent to the page, but it shows nothing of them.
const NO_ROW = new Set(["capture_broken"]);
// What each line must say, which only holds if the page read the right fields.
const SAYS = {
  kill: ["can of tuna", "silvo", "medic", "DOMINATING", "AIRSHOT"],
  flag: ["can of tuna", "captured", "RED intelligence"],
  uber: ["medic", "ÜBER", "Kritzkrieg", "can of tuna"],
  uber_ready: ["medic", "Medi Gun ÜberCharge ready"],
  medic_death: ["silvo", "DROP", "medic"],
  capture_blocked: ["can of tuna", "defended Mid"],
  building_destroyed: ["can of tuna", "destroyed", "silvo", "Sentry"],
  banner: ["can of tuna", "Buff Banner"],
  point_captured: ["BLU captured Last"],
  round_win: ["RED wins the round"],
  scores: ["Match over · BLU 2 – 3 RED"],
  player_leave: ["silvo", "left"],
  team_change: ["can of tuna", "joined RED"],
  chat: ["can of tuna", ": gg"],
  killed_capper: ["can of tuna", "killed capper", "silvo"],
  medic_defended: ["can of tuna", "defended medic", "medic"],
  vote_options: ["Vote: Yes / No"],
  vote_cast: ["can of tuna", "voted Yes"],
  tournament: ["can of tuna", "is ready"],
  announcer: ["1 minute remains"],
  time_remaining: ["1:30 remaining"],
  pass_score: ["can of tuna", "medic", "scored 2 points"],
  pass_stolen: ["can of tuna", "stole the ball from", "silvo"],
  pass_blocked: ["can of tuna", "blocked a pass from", "silvo"],
  ball_carrier_killed: ["can of tuna", "killed the ball carrier", "silvo"],
  robot_killed: ["can of tuna", "destroyed a robot"],
  special_score: ["can of tuna", "scored"],
  team_leader_killed: ["can of tuna", "killed team leader", "silvo"],
  rocket_open: ["BLU opened the rocket"],
  point_locked: ["First locked"],
  sapped: ["can of tuna", "sapped", "silvo", "Dispenser"],
  vote_started: ["can of tuna", "kick silvo (cheating)"],
  voice_command: ["can of tuna", "MEDIC!"],
  server_text: ["Welcome to the server"],
};

test("the samples cover what the feed and chat show", () => {
  const types = new Set(samples.map(s => s.type));
  for (const t of Object.keys(SAYS)) assert.ok(types.has(t), t);
});

for (const ev of samples) {
  test(`${ev.type} from stvtf draws a whole row`, () => {
    state.map = "cp_process_f12";
    const box = CHAT_TYPES.has(ev.type) ? "chat" : "feed";
    $(box).replaceChildren();
    (CHAT_TYPES.has(ev.type) ? chat : feedEvent)(ev, p);
    const html = $(box).children[0]?.innerHTML;
    if (NO_ROW.has(ev.type)) {
      assert.equal(html, undefined);
      return;
    }
    assert.ok(html, "a row is drawn");
    assert.doesNotMatch(html, /undefined|null|NaN|\[object/);
    for (const part of SAYS[ev.type] ?? []) assert.ok(html.includes(part), `${part} in ${html}`);
  });
}
