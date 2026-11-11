import assert from "node:assert/strict";
import { test } from "node:test";

// Enough of WebCodecs and Web Audio for voice to turn on.
const g = /** @type {any} */ (globalThis);
g.AudioDecoder = class {
  state = "configured";
  configure() {}
  decode() {}
  close() {
    this.state = "closed";
  }
};
g.EncodedAudioChunk = class {};
g.AudioContext = class {
  state = "running";
  currentTime = 0;
  destination = {};
  createGain() {
    return { gain: { value: 1 }, connect() {}, disconnect() {} };
  }
};

const v = await import("../../app/core/voice.js");
const line = { type: "voice", tick: 1, player: null, rate: 24000, frames: ["aA=="] };

test("a voice of no player is heard by its name, on the team its line says", () => {
  v.setVoice(true);
  v.onVoice(/** @type {any} */ ({ ...line, steamid: "discord:77", name: "coach", team: "red" }));
  v.onVoice(/** @type {any} */ ({ ...line, steamid: "discord:78", name: "caster" }));
  const heard = v.speakersHeard();
  assert.deepEqual(
    heard.map(s => [s.steamid, s.name, s.talking]),
    [
      ["discord:77", "coach", true],
      ["discord:78", "caster", true],
    ],
  );
  assert.equal(v.teamOf("discord:77"), "red");
  assert.equal(v.teamOf("discord:78"), null);
  v.setTeamMuted("red", true);
  assert.equal(v.isMuted("discord:77"), true, "muted with the team");
  assert.equal(v.isMuted("discord:78"), false);
  v.setTeamMuted("red", false);
});

test("a room's nudge of the voice is kept, and the page told", () => {
  let told = 0;
  v.voiceListeners.add(() => told++);
  v.voiceNudged({ nudge: -1.5 });
  assert.equal(v.voiceNudge(), -1.5);
  assert.equal(told, 1);
});

test("names that look alike are suggested: the same letters, or one within the other", async () => {
  const { alike } = await import("../../app/room/voiceadmin.js");
  assert.equal(alike("Gummy", "gummy"), true);
  assert.equal(alike("[team] gummy :)", "Gummy"), true);
  assert.equal(alike("gummy", "arie"), false);
  assert.equal(alike("a", "arie"), false, "too short to say");
});

test("a voice of no player that fell silent is no longer listed (matched since, or gone)", t => {
  const real = performance.now;
  let now = real.call(performance);
  performance.now = () => now;
  t.after(() => (performance.now = real));
  v.onVoice(/** @type {any} */ ({ ...line, steamid: "discord:79", name: "sub" }));
  v.onVoice(/** @type {any} */ ({ ...line, steamid: "[U:1:9]", player: 9 }));
  now += 119_000;
  assert.ok(v.speakersHeard().some(s => s.steamid === "discord:79"));
  now += 2_000;
  const heard = v.speakersHeard().map(s => s.steamid);
  assert.equal(heard.includes("discord:79"), false);
  assert.equal(heard.includes("[U:1:9]"), true, "a player stays");
});
