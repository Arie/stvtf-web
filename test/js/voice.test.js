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
