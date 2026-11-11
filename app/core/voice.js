// The players' voice (`voice` lines, which the page's stream carries once it asks: ?voice=1): each
// speaker's Opus frames decoded with WebCodecs and played through a volume of their own, one
// after the other as they come. Who talks, and the viewer's mutes and volumes (kept in the
// browser, by SteamID, across matches). Off until the viewer turns it on: a browser plays sound
// only after a click. Pausing, seeking or another speed than 1× drops what is queued.
import { OVERLAY, state } from "./core.js";
import { env } from "./env.js";
/** @typedef {import("../../types/stream").VoiceLine} VoiceLine */

// Seconds of sound queued ahead of now, to ride out lines that come in bursts.
const LEAD = 0.1;
// How long a speaker counts as talking after their last frame, in milliseconds.
const TALK_MS = 400;
// How long a voice of no player stays listed after its last frame, in milliseconds.
const FORGET_MS = 120_000;
// The most seconds of a speaker queued ahead (past it, after a stall, frames are dropped), and the
// most speakers (a client that claims a new SteamID each packet makes a new one each time).
const MAX_AHEAD = 1,
  MAX_SPEAKERS = 64;
// What the browser keeps: whether voice is on, and the mutes and volumes.
const ON_KEY = "voice",
  PREFS_KEY = "voice-prefs";

/**
 * @typedef {{
 *   steamid: string; player: number | null; name: string | null; team: string | null; decoder: AudioDecoder | null; gain: GainNode | null;
 *   at: number; n: number; last: number; queued: Set<AudioBufferSourceNode>;
 * }} Speaker
 */

/** @type {AudioContext | null} */
let audio = null;
let on = read(ON_KEY) === "1";
// Not paused, and at 1×.
let playing = true;
/** @type {Map<string, Speaker>} */
const speakers = new Map();
/** @type {{ muted: string[]; volume: Record<string, number> }} */
const prefs = { muted: [], volume: {}, ...parse(read(PREFS_KEY)) };
// Told when who talks, or the mutes, change.
/** @type {Set<() => void>} */
export const voiceListeners = new Set();

/** @param {string} key */
function read(key) {
  try {
    return env.storage ? localStorage.getItem(key) : null;
  } catch {
    return null;
  }
}
/** @param {string} key @param {string} value */
function write(key, value) {
  try {
    if (env.storage) localStorage.setItem(key, value);
  } catch {}
}
/** @param {string | null} s */
function parse(s) {
  try {
    const p = JSON.parse(s ?? "{}");
    return {
      muted: Array.isArray(p.muted) ? p.muted : [],
      volume: p.volume && typeof p.volume === "object" ? p.volume : {},
    };
  } catch {
    return {};
  }
}
const savePrefs = () => write(PREFS_KEY, JSON.stringify(prefs));

// Why this page cannot play voice, if it cannot.
export function voiceUnsupported() {
  if (OVERLAY) return "Not on the overlay";
  if (typeof AudioDecoder === "undefined")
    return "This browser cannot play voice here (it needs WebCodecs, on a secure page)";
  return null;
}

export const voiceOn = () => on && !voiceUnsupported();

// Turns voice on or off; the stream then asks for it, or stops.
/** @param {boolean} want */
export function setVoice(want) {
  on = want;
  write(ON_KEY, want ? "1" : "0");
  if (want && !voiceUnsupported()) start();
  else for (const id of [...speakers.keys()]) forget(id);
  voiceListeners.forEach(f => f());
}

// The audio, once there has been a click: voice turned on in an earlier visit waits for one.
function start() {
  audio ??= new AudioContext();
  if (audio.state === "suspended") audio.resume().catch(() => {});
}
env.root.addEventListener("pointerdown", () => voiceOn() && start(), { capture: true, passive: true });

/** @param {string} id */
function forget(id) {
  const s = speakers.get(id);
  if (!s) return;
  drop(s);
  s.gain?.disconnect();
  speakers.delete(id);
}

// What is queued of `s` stops, and its decoder goes (a new one starts afresh).
/** @param {Speaker} s */
function drop(s) {
  for (const q of s.queued) q.stop();
  s.queued.clear();
  s.at = 0;
  if (s.decoder && s.decoder.state !== "closed") s.decoder.close();
  s.decoder = null;
}

// The speaker of `m`, ready to decode, at the level their mutes say now (a team's changes); none
// past the most speakers, or with a decoder that will not take the line.
/** @param {VoiceLine} m @returns {Speaker | null} */
function speaker(m) {
  let s = speakers.get(m.steamid);
  if (!s) {
    if (speakers.size >= MAX_SPEAKERS) return null;
    s = {
      steamid: m.steamid,
      player: null,
      name: null,
      team: null,
      decoder: null,
      gain: null,
      at: 0,
      n: 0,
      last: 0,
      queued: new Set(),
    };
    speakers.set(m.steamid, s);
  }
  s.player = m.player ?? null;
  // A voice from outside the match that is no player's: its name, and the team it is taken for.
  s.name = m.name ?? null;
  s.team = m.team ?? null;
  if (audio && !s.gain) {
    s.gain = audio.createGain();
    s.gain.connect(audio.destination);
  }
  level(s);
  if (!s.decoder || s.decoder.state === "closed") {
    const self = s;
    const decoder = new AudioDecoder({ output: d => play(self, d), error: () => (self.decoder = null) });
    try {
      decoder.configure({ codec: "opus", sampleRate: m.rate, numberOfChannels: 1 });
    } catch {
      decoder.close();
      return null;
    }
    s.decoder = decoder;
  }
  return s;
}

/** @param {Speaker} s */
function level(s) {
  if (s.gain) s.gain.gain.value = isMuted(s.steamid) ? 0 : volumeOf(s.steamid);
}

/** @param {Speaker} s @param {AudioData} data */
function play(s, data) {
  // Not playing, or so far behind (a stall, a burst) that it would run late: dropped.
  if (!audio || !s.gain || !playing || s.at - audio.currentTime > MAX_AHEAD) return data.close();
  const buffer = audio.createBuffer(1, data.numberOfFrames, data.sampleRate);
  data.copyTo(buffer.getChannelData(0), { planeIndex: 0, format: "f32-planar" });
  data.close();
  const source = audio.createBufferSource();
  source.buffer = buffer;
  source.connect(s.gain);
  const at = Math.max(s.at, audio.currentTime + LEAD);
  source.start(at);
  source.onended = () => s.queued.delete(source);
  s.queued.add(source);
  s.at = at + buffer.duration;
}

/** @param {VoiceLine} m */
export function onVoice(m) {
  if (!voiceOn() || !audio || !playing || !(m.rate > 0)) return;
  const s = speaker(m);
  if (!s) return;
  s.last = performance.now();
  for (const f of m.frames) {
    const data = Uint8Array.from(atob(f), c => c.charCodeAt(0));
    // 20 ms frames: their timestamps need only go up.
    s.decoder?.decode(new EncodedAudioChunk({ type: "key", timestamp: s.n++ * 20_000, data }));
  }
}

// Playback paused, went on, or changed speed: only playing at 1× has voice.
/** @param {{ paused: boolean; speed: number }} m */
export function voicePlayback(m) {
  const now = !m.paused && m.speed === 1;
  if (now === playing) return;
  playing = now;
  if (!now) dropAll();
}

export const voicePlaying = () => playing;

// The page's stream switched (one's own playback, and back to live, which says nothing of
// playback): as playing, at 1×, nothing of the old one queued.
export function streamSwitched() {
  playing = true;
  dropAll();
}

// A seek or a jump: what is queued was of before it.
export function dropAll() {
  for (const s of speakers.values()) drop(s);
}

/** @param {Speaker} s */
const talking = s => performance.now() - s.last < TALK_MS;

// Whether player `id` is talking, muted or not, for their card and their dot.
/** @param {number} id */
export function isTalking(id) {
  for (const s of speakers.values()) if (s.player === id && talking(s)) return true;
  return false;
}

// Everyone heard so far, for the speakers list. A voice of no player that fell silent is forgotten
// (matched to a player since, or gone): their mute and volume are kept.
export function speakersHeard() {
  const now = performance.now();
  for (const s of [...speakers.values()]) if (s.player == null && now - s.last > FORGET_MS) forget(s.steamid);
  return [...speakers.values()].map(s => ({
    steamid: s.steamid,
    player: s.player,
    name: s.name,
    talking: talking(s),
  }));
}

// Teams muted, for this page (another match has other players), and those let through anyway.
/** @type {Set<string>} */
const mutedTeams = new Set();
/** @type {Set<string>} */
const letThrough = new Set();

// The team of whoever has `steamid` in the match; of a voice that is no player's, the team its
// lines say.
/** @param {string} steamid */
export function teamOf(steamid) {
  for (const p of state.players.values()) if (p.steamid === steamid) return p.team;
  return speakers.get(steamid)?.team ?? null;
}

// Whether `steamid` is muted: by the viewer (kept), or with their team (unless let through).
/** @param {string} steamid */
export function isMuted(steamid) {
  if (prefs.muted.includes(steamid)) return true;
  const team = teamOf(steamid);
  return team != null && mutedTeams.has(team) && !letThrough.has(steamid);
}

/** @param {string} team */
export const isTeamMuted = team => mutedTeams.has(team);

// After a change of mutes: every speaker at their level, and the page told.
function remuted() {
  for (const s of speakers.values()) level(s);
  voiceListeners.forEach(f => f());
}

// A click on someone: muted, they are heard (their own mute lifted, or let through their team's);
// heard, their own mute.
/** @param {string} steamid */
export function toggleMuted(steamid) {
  if (!isMuted(steamid)) {
    letThrough.delete(steamid);
    return setMuted(steamid, true);
  }
  if (prefs.muted.includes(steamid)) setMuted(steamid, false);
  if (isMuted(steamid)) letThrough.add(steamid);
  remuted();
}

// Mutes or unmutes `team`, all its players: muting it again takes back those let through.
/** @param {string} team @param {boolean} muted */
export function setTeamMuted(team, muted) {
  if (muted) {
    mutedTeams.add(team);
    for (const id of [...letThrough]) if (teamOf(id) === team) letThrough.delete(id);
  } else mutedTeams.delete(team);
  remuted();
}

// Whether everyone is muted: both teams, and those heard on neither.
export function allMuted() {
  if (!mutedTeams.has("red") || !mutedTeams.has("blue")) return false;
  return [...speakers.keys()].every(id => isMuted(id));
}

// Everyone muted (both teams, and those heard on neither: one's own mutes kept), or nobody (one's
// own mutes of those heard lifted too).
/** @param {boolean} muted */
export function muteEveryone(muted) {
  letThrough.clear();
  if (muted) {
    mutedTeams.add("red").add("blue");
    for (const id of speakers.keys()) if (!isMuted(id)) prefs.muted.push(id);
  } else {
    mutedTeams.clear();
    prefs.muted = prefs.muted.filter(id => !speakers.has(id));
  }
  savePrefs();
  remuted();
}

// The other team heard: one muted, the two swap; neither or both, RED is heard and BLU muted.
export function swapTeams() {
  const hearRed = !(mutedTeams.has("blue") && !mutedTeams.has("red"));
  setTeamMuted("red", !hearRed);
  setTeamMuted("blue", hearRed);
}
/** @param {string} steamid */
export const volumeOf = steamid => prefs.volume[steamid] ?? 1;

/** @param {string} steamid @param {boolean} muted */
export function setMuted(steamid, muted) {
  prefs.muted = prefs.muted.filter(id => id !== steamid);
  if (muted) prefs.muted.push(steamid);
  savePrefs();
  remuted();
}

/** @param {string} steamid @param {number} volume 0 to 1 */
export function setVolume(steamid, volume) {
  if (volume >= 1) delete prefs.volume[steamid];
  else prefs.volume[steamid] = Math.max(0, volume);
  savePrefs();
  const s = speakers.get(steamid);
  if (s) level(s);
}

// Whether `steamid` has talked since the page turned voice on.
/** @param {string} steamid */
export const hasSpoken = steamid => speakers.has(steamid);

// The SteamID of player `id`, while they are a speaker heard.
/** @param {number} id */
export function steamidOf(id) {
  for (const s of speakers.values()) if (s.player === id) return s.steamid;
  return null;
}
