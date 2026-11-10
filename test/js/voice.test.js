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
