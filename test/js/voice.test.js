import assert from "node:assert/strict";
import { test } from "node:test";
import {
  isMuted,
  isTalking,
  setMuted,
  setVoice,
  setVolume,
  voiceOn,
  voiceUnsupported,
  volumeOf,
} from "../../app/core/voice.js";

test("without WebCodecs voice says why and stays off", () => {
  assert.match(voiceUnsupported() ?? "", /WebCodecs/);
  setVoice(true);
  assert.equal(voiceOn(), false);
});

test("a speaker's mute and volume are kept by SteamID", () => {
  const id = "[U:1:2]";
  assert.equal(isMuted(id), false);
  setMuted(id, true);
  assert.equal(isMuted(id), true);
  setMuted(id, false);
  assert.equal(isMuted(id), false);
  setVolume(id, 0.4);
  assert.equal(volumeOf(id), 0.4);
  setVolume(id, 1);
  assert.equal(volumeOf(id), 1, "full volume is the default again");
});

test("nobody talks before any voice came", () => {
  assert.equal(isTalking(3), false);
});

test("a team's mute covers its players, one can be let through, and swap hears the other team", async () => {
  const { state } = await import("../../app/core/core.js");
  const v = await import("../../app/core/voice.js");
  const red = "[U:1:31]",
    blu = "[U:1:32]";
  state.players = new Map([
    [31, /** @type {any} */ ({ id: 31, steamid: red, team: "red" })],
    [32, /** @type {any} */ ({ id: 32, steamid: blu, team: "blue" })],
  ]);
  v.setTeamMuted("blue", true);
  assert.equal(v.isMuted(blu), true, "by their team");
  assert.equal(v.isMuted(red), false);
  v.toggleMuted(blu);
  assert.equal(v.isMuted(blu), false, "let through");
  v.setTeamMuted("blue", true);
  assert.equal(v.isMuted(blu), true, "muting the team again takes it back");
  v.swapTeams();
  assert.deepEqual([v.isTeamMuted("red"), v.isTeamMuted("blue")], [true, false]);
  assert.deepEqual([v.isMuted(red), v.isMuted(blu)], [true, false]);
  v.swapTeams();
  assert.deepEqual([v.isTeamMuted("red"), v.isTeamMuted("blue")], [false, true]);
  v.setTeamMuted("blue", false);
  v.swapTeams();
  assert.deepEqual([v.isTeamMuted("red"), v.isTeamMuted("blue")], [false, true], "from neither: RED heard first");
  v.setTeamMuted("blue", false);
  v.toggleMuted(red);
  assert.equal(v.isMuted(red), true, "one's own mute, without a team's");
  v.toggleMuted(red);
  assert.equal(v.isMuted(red), false);
});

test("mute all mutes both teams though nobody was heard, and keeps one's own mutes", async () => {
  const { state } = await import("../../app/core/core.js");
  const v = await import("../../app/core/voice.js");
  const red = "[U:1:41]";
  state.players = new Map([[41, /** @type {any} */ ({ id: 41, steamid: red, team: "red" })]]);
  v.setTeamMuted("red", false);
  v.setTeamMuted("blue", true);
  v.setMuted(red, true);
  assert.equal(v.allMuted(), false, "RED is not");
  v.muteEveryone(!v.allMuted());
  assert.deepEqual([v.isTeamMuted("red"), v.isTeamMuted("blue"), v.allMuted()], [true, true, true]);
  v.setTeamMuted("red", false);
  assert.equal(v.isMuted(red), true, "their own mute kept");
  v.setMuted(red, false);
});

test("after mute all nobody is heard, whoever starts talking later, until they are let through", async () => {
  const v = await import("../../app/core/voice.js");
  const { state } = await import("../../app/core/core.js");
  state.players = new Map([[41, /** @type {any} */ ({ id: 41, steamid: "[U:1:41]", team: "red" })]]);
  v.muteEveryone(false);
  v.muteEveryone(true);
  // A caster, a voice on neither team, a player the page knows nothing of yet: none had talked.
  assert.equal(v.isMuted("discord:900"), true);
  assert.equal(v.mutedAs("mumble:cert:abc", null, null), true);
  assert.equal(v.mutedAs("[U:1:77]", 77, null), true);
  assert.equal(v.allMuted(), true);
  // One let through by a click is heard; everyone else stays muted, and the button says so.
  v.toggleMuted("discord:900");
  assert.deepEqual([v.isMuted("discord:900"), v.isMuted("discord:901"), v.allMuted()], [false, true, false]);
  v.muteEveryone(true);
  assert.equal(v.isMuted("discord:900"), true, "muting all again takes it back");
  v.muteEveryone(false);
  assert.deepEqual([v.isMuted("discord:900"), v.isMuted("[U:1:41]")], [false, false]);
});

test("with a team muted, a player whose team the page does not know yet is not heard", async () => {
  const v = await import("../../app/core/voice.js");
  const { state } = await import("../../app/core/core.js");
  state.players = new Map([[41, /** @type {any} */ ({ id: 41, steamid: "[U:1:41]", team: "red" })]]);
  v.muteEveryone(false);
  v.setTeamMuted("blue", true);
  assert.equal(v.mutedAs("[U:1:41]", 41, null), false, "RED is heard");
  assert.equal(v.mutedAs("[U:1:55]", 55, null), true, "a player not in the match's list yet may be BLU");
  assert.equal(v.mutedAs("discord:5", null, "blue"), true, "a voice taken for BLU");
  assert.equal(v.mutedAs("discord:6", null, null), false, "a caster is on neither team");
  state.players.set(55, /** @type {any} */ ({ id: 55, steamid: "[U:1:55]", team: "red" }));
  assert.equal(v.mutedAs("[U:1:55]", 55, null), false, "known to be RED now");
  v.setTeamMuted("blue", false);
});

test("a switch of stream (back to live) plays voice again", async () => {
  const v = await import("../../app/core/voice.js");
  v.voicePlayback({ paused: true, speed: 1 });
  assert.equal(v.voicePlaying(), false);
  v.streamSwitched();
  assert.equal(v.voicePlaying(), true);
});
