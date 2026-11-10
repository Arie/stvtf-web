// The players' voice (`voice` lines, which the page's stream carries once it asks: ?voice=1): each
// speaker's Opus frames decoded with WebCodecs and played through a volume of their own, one
// after the other as they come. Who talks, and the viewer's mutes and volumes (kept in the
// browser, by SteamID, across matches). Off until the viewer turns it on: a browser plays sound
// only after a click. Pausing, seeking or another speed than 1× drops what is queued.
import { OVERLAY } from "./core.js";
import { env } from "./env.js";
/** @typedef {import("../../types/stream").VoiceLine} VoiceLine */

// Seconds of sound queued ahead of now, to ride out lines that come in bursts.
const LEAD = 0.1;
// How long a speaker counts as talking after their last frame, in milliseconds.
const TALK_MS = 400;
// What the browser keeps: whether voice is on, and the mutes and volumes.
const ON_KEY = "voice",
  PREFS_KEY = "voice-prefs";

/**
 * @typedef {{
 *   steamid: string; player: number | null; decoder: AudioDecoder | null; gain: GainNode | null;
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
  if (s.decoder?.state !== "closed") s.decoder?.close();
  speakers.delete(id);
}

// What is queued of `s` stops, and its decoder starts afresh.
/** @param {Speaker} s */
function drop(s) {
  for (const q of s.queued) q.stop();
  s.queued.clear();
  s.at = 0;
  if (s.decoder?.state === "configured") s.decoder.reset();
  s.decoder = null;
}

/** @param {VoiceLine} m */
function speaker(m) {
  let s = speakers.get(m.steamid);
  if (!s) {
    s = { steamid: m.steamid, player: null, decoder: null, gain: null, at: 0, n: 0, last: 0, queued: new Set() };
    speakers.set(m.steamid, s);
  }
  s.player = m.player ?? null;
  if (audio && !s.gain) {
    s.gain = audio.createGain();
    s.gain.connect(audio.destination);
    level(s);
  }
  if (!s.decoder || s.decoder.state === "closed") {
    const self = s;
    s.decoder = new AudioDecoder({ output: d => play(self, d), error: () => (self.decoder = null) });
    s.decoder.configure({ codec: "opus", sampleRate: m.rate, numberOfChannels: 1 });
  }
  return s;
}

/** @param {Speaker} s */
function level(s) {
  if (s.gain) s.gain.gain.value = isMuted(s.steamid) ? 0 : volumeOf(s.steamid);
}

/** @param {Speaker} s @param {AudioData} data */
function play(s, data) {
  if (!audio || !s.gain || !playing) return data.close();
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
  if (!voiceOn() || !audio || !playing) return;
  const s = speaker(m);
  const was = talking(s);
  s.last = performance.now();
  for (const f of m.frames) {
    const data = Uint8Array.from(atob(f), c => c.charCodeAt(0));
    // 20 ms frames: their timestamps need only go up.
    s.decoder?.decode(new EncodedAudioChunk({ type: "key", timestamp: s.n++ * 20_000, data }));
  }
  if (!was) voiceListeners.forEach(f => f());
}

// Playback paused, went on, or changed speed: only playing at 1× has voice.
/** @param {{ paused: boolean; speed: number }} m */
export function voicePlayback(m) {
  const now = !m.paused && m.speed === 1;
  if (now === playing) return;
  playing = now;
  if (!now) dropAll();
}

// A seek or a jump: what is queued was of before it.
export function dropAll() {
  for (const s of speakers.values()) drop(s);
}

/** @param {Speaker} s */
const talking = s => performance.now() - s.last < TALK_MS;

// Whether player `id` is talking (and not muted), for their card and their dot.
/** @param {number} id */
export function isTalking(id) {
  for (const s of speakers.values()) if (s.player === id && talking(s) && !isMuted(s.steamid)) return true;
  return false;
}

// Everyone heard so far, for the speakers list.
export function speakersHeard() {
  return [...speakers.values()].map(s => ({ steamid: s.steamid, player: s.player, talking: talking(s) }));
}

/** @param {string} steamid */
export const isMuted = steamid => prefs.muted.includes(steamid);
/** @param {string} steamid */
export const volumeOf = steamid => prefs.volume[steamid] ?? 1;

/** @param {string} steamid @param {boolean} muted */
export function setMuted(steamid, muted) {
  prefs.muted = prefs.muted.filter(id => id !== steamid);
  if (muted) prefs.muted.push(steamid);
  savePrefs();
  const s = speakers.get(steamid);
  if (s) level(s);
  voiceListeners.forEach(f => f());
}

/** @param {string} steamid @param {number} volume 0 to 1 */
export function setVolume(steamid, volume) {
  if (volume >= 1) delete prefs.volume[steamid];
  else prefs.volume[steamid] = Math.max(0, volume);
  savePrefs();
  const s = speakers.get(steamid);
  if (s) level(s);
}

// The SteamID of player `id`, while they are a speaker heard.
/** @param {number} id */
export function steamidOf(id) {
  for (const s of speakers.values()) if (s.player === id) return s.steamid;
  return null;
}
