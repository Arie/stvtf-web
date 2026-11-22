// The players' voice (`voice` lines, which the page's stream carries once it asks: ?voice=1). Each
// speaker's Opus frames are decoded with WebCodecs and played in order through that speaker's own
// volume. Also tracks who is talking, and the viewer's mutes and volumes (kept in the browser by
// SteamID, across matches). Off until the viewer turns it on: a browser plays sound only after a
// click. Pausing, seeking or a speed other than 1× drops what is queued.
import { OVERLAY, state } from "./core.js";
import { env } from "./env.js";
/** @typedef {import("../../types/stream").VoiceLine} VoiceLine */

// Seconds of sound queued ahead of now, to ride out lines that come in bursts.
const LEAD = 0.1;
// How long a speaker counts as talking after their last frame, in milliseconds.
const TALK_MS = 400;
// How long a voice that belongs to no player stays listed after its last frame, in milliseconds.
const FORGET_MS = 120_000;
// The most seconds of a speaker queued ahead (past it, after a stall, frames are dropped), and the
// most speakers (a client that claims a new SteamID each packet makes a new one each time).
const MAX_AHEAD = 1,
  MAX_SPEAKERS = 64;
// The RMS levels (dB of full scale) at which a speaker's loudness is 0 and 1, and how long a level
// stays shown after it played, in seconds.
const QUIET_DB = -42,
  LOUD_DB = -14,
  LOUD_HOLD = 0.12;
// localStorage keys: whether voice is on, and the mutes and volumes.
const ON_KEY = "voice",
  PREFS_KEY = "voice-prefs";

/**
 * @typedef {{
 *   steamid: string; player: number | null; name: string | null; team: string | null; decoder: AudioDecoder | null; gain: GainNode | null;
 *   at: number; n: number; last: number; queued: Set<AudioBufferSourceNode>; loud: { at: number; end: number; level: number }[];
 * }} Speaker
 */

/** @type {AudioContext | null} */
let audio = null;
let on = read(ON_KEY) === "1";
// Whether the viewer ever chose voice on or off.
let chosen = read(ON_KEY) != null;
// Not paused, and at 1×.
let playing = true;
/** @type {Map<string, Speaker>} */
const speakers = new Map();
// While paused: who was talking when playback stopped, and how loud (null: none of their sound had
// played yet). They stay shown that way until playback resumes or jumps.
/** @type {Map<string, number | null> | null} */
let held = null;
// When it paused, by the page's clock: the waveforms stand still at that moment.
let heldAt = 0;

// The time the waveforms are drawn at: now, or while paused, the moment playback stopped.
/** @param {number} t */
export const waveTime = t => (held ? heldAt : t);
/** @type {{ muted: string[]; volume: Record<string, number> }} */
const prefs = { muted: [], volume: {}, ...parse(read(PREFS_KEY)) };
// Called when who is talking or the mutes change.
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
  chosen = true;
  write(ON_KEY, want ? "1" : "0");
  if (want && !voiceUnsupported()) start();
  else for (const id of [...speakers.keys()]) forget(id);
  voiceListeners.forEach(f => f());
}

// Starts the audio. A browser plays no sound before a click or a key press, so voice turned on in an
// earlier visit waits for one; the listeners are told when the audio starts.
function start() {
  if (!audio) {
    audio = new AudioContext();
    audio.onstatechange = () => voiceListeners.forEach(f => f());
  }
  if (audio.state === "suspended") audio.resume().catch(() => {});
}
for (const first of ["pointerdown", "keydown"])
  env.root.addEventListener(
    first,
    () => {
      asked = voiceMissed() ? { both: missed.has("red") && missed.has("blue") } : null;
      if (voiceOn()) start();
    },
    { capture: true, passive: true },
  );

// Whether voice is on but nothing can be heard yet: the page waits for its first click.
export const voiceWaiting = () => voiceOn() && audio?.state !== "running";

// Teams that spoke while the viewer could not hear them ("" for unknown): the voice button then asks
// for a click.
/** @type {Set<string>} */
const missed = new Set();
const unheard = () => (voiceOn() ? voiceWaiting() : !chosen && !voiceUnsupported());
export const voiceMissed = () => unheard() && missed.size > 0;

/** @param {string} team */
function spokeUnheard(team) {
  if (!unheard() || missed.has(team)) return;
  missed.add(team);
  voiceListeners.forEach(f => f());
}

// When each player last talked, from `talking` lines.
/** @type {Map<number, number>} */
const said = new Map();

/** @param {import("../../types/stream").TalkingLine} m */
export function onTalking(m) {
  spokeUnheard(m.team ?? "");
  if (m.player != null) said.set(m.player, performance.now());
}

// Whether the voice button was asking for a click at the last click or key press, and whether both
// teams had spoken by then.
/** @type {{ both: boolean } | null} */
let asked = null;
export const voiceAsked = () => asked;

/** @param {string} id */
function forget(id) {
  const s = speakers.get(id);
  if (!s) return;
  drop(s);
  s.gain?.disconnect();
  speakers.delete(id);
}

// Stops what is queued for `s` and closes its decoder (the next line starts a new one).
/** @param {Speaker} s */
function drop(s) {
  for (const q of s.queued) q.stop();
  s.queued.clear();
  s.loud = [];
  s.at = 0;
  if (s.decoder && s.decoder.state !== "closed") s.decoder.close();
  s.decoder = null;
}

// The speaker of `m`, ready to decode, with their volume set from the mutes as they are now (a team
// mute can change it). Null past MAX_SPEAKERS, or when a decoder cannot be configured for the line.
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
      loud: [],
    };
    speakers.set(m.steamid, s);
  }
  s.player = m.player ?? null;
  // For a voice from outside the match that belongs to no player: its name, and the team it counts as.
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

// When the next sound starts, with sound queued until `queued` and the time `now`: right after the
// queue while any is left (a line late by less than LEAD leaves no gap), else LEAD ahead of now.
/** @param {number} queued @param {number} now */
export const startAt = (queued, now) => (queued > now ? queued : now + LEAD);

/** @param {Speaker} s @param {AudioData} data */
function play(s, data) {
  // Not playing, or so far behind (a stall, a burst) that it would run late: dropped.
  if (!audio || !s.gain || !playing || s.at - audio.currentTime > MAX_AHEAD) return data.close();
  const buffer = audio.createBuffer(1, data.numberOfFrames, data.sampleRate);
  const samples = buffer.getChannelData(0);
  data.copyTo(samples, { planeIndex: 0, format: "f32-planar" });
  data.close();
  // Measured before the buffer is given away to play: Firefox detaches its samples then.
  const loud = loudness(samples);
  const source = audio.createBufferSource();
  source.buffer = buffer;
  source.connect(s.gain);
  const at = startAt(s.at, audio.currentTime);
  source.start(at);
  source.onended = () => s.queued.delete(source);
  s.queued.add(source);
  s.at = at + buffer.duration;
  // How loud it is, for when it plays; what played a while ago is forgotten.
  const now = audio.currentTime;
  s.loud = s.loud.filter(l => l.end > now - LOUD_HOLD);
  s.loud.push({ at, end: s.at, level: loud });
}

// How loud `samples` are, 0 (a murmur) to 1 (a shout): their RMS level in decibels, between QUIET_DB and LOUD_DB.
/** @param {Float32Array} samples */
export function loudness(samples) {
  let sum = 0;
  for (const v of samples) sum += v * v;
  const db = 10 * Math.log10(sum / Math.max(1, samples.length) + 1e-12);
  return Math.min(1, Math.max(0, (db - QUIET_DB) / (LOUD_DB - QUIET_DB)));
}

// How loud `s` is at `now`, counting what is queued to play too when `ahead`; null with nothing
// known of it.
/** @param {Speaker} s @param {number} now @param {boolean} ahead */
function loudAt(s, now, ahead) {
  let loud = null;
  for (const l of s.loud) if ((ahead || l.at <= now) && l.end > now - LOUD_HOLD) loud = Math.max(loud ?? 0, l.level);
  return loud;
}

// How loud player `id` is as heard now, 0 to 1 (the loudest within LOUD_HOLD, so it does not
// flicker between syllables); null with nothing known of it (not talking, or nothing played).
// While paused, as loud as they were when playback stopped.
/** @param {number} id */
export function loudnessOf(id) {
  if (!audio) return null;
  const now = audio.currentTime;
  let loud = null;
  for (const s of speakers.values()) {
    if (s.player !== id) continue;
    const l = held ? (held.get(s.steamid) ?? null) : loudAt(s, now, false);
    if (l != null) loud = Math.max(loud ?? 0, l);
  }
  return loud;
}

/** @param {VoiceLine} m */
export function onVoice(m) {
  if (m.frames.length) spokeUnheard(teamOf(m.steamid) ?? m.team ?? "");
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
  // On a pause, those talking stay shown as they were, counting the sound that was about to play.
  if (!m.paused) held = null;
  else if (!held) {
    const at = audio?.currentTime ?? 0;
    held = new Map([...speakers.values()].filter(talking).map(s => [s.steamid, loudAt(s, at, true)]));
    heldAt = performance.now();
  }
  const now = !m.paused && m.speed === 1;
  if (now === playing) return;
  playing = now;
  if (!now) for (const s of speakers.values()) drop(s);
}

export const voicePlaying = () => playing;

// The page's stream switched (to the viewer's own playback, or back to live, which sends no playback
// line): assume it plays at 1×, and drop what the old stream queued.
export function streamSwitched() {
  playing = true;
  dropAll();
}

// A seek or a jump: what is queued is from before it, and so is the list of who was talking in a pause.
export function dropAll() {
  if (held) held = new Map();
  for (const s of speakers.values()) drop(s);
}

// Whether `s` is talking; while paused, whether they were when playback stopped.
/** @param {Speaker} s */
function talking(s) {
  return held ? held.has(s.steamid) : performance.now() - s.last < TALK_MS;
}

// Whether player `id` is talking, muted or not, for their card and their dot.
/** @param {number} id */
export function isTalking(id) {
  for (const s of speakers.values()) if (s.player === id && talking(s)) return true;
  return !held && performance.now() - (said.get(id) ?? -Infinity) < TALK_MS;
}

// Everyone heard so far, for the speakers list. A voice that belongs to no player is forgotten after
// FORGET_MS of silence (it was matched to a player since, or is gone); its mute and volume are kept.
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

// Mutes of everyone or of a team, for this page (another match has other players), and the players
// let through anyway. The mutes are kept in sessionStorage, so a reload does not let anyone be heard again.
const MUTES_KEY = `stvtf.voice.mutes:${globalThis.location?.pathname ?? ""}`;
/** @type {Set<string>} */
const mutedTeams = new Set();
/** @type {Set<string>} */
const letThrough = new Set();
let everyone = false;
try {
  const kept = JSON.parse((env.storage && sessionStorage.getItem(MUTES_KEY)) || "{}");
  everyone = kept.everyone === true;
  for (const team of Array.isArray(kept.teams) ? kept.teams : []) mutedTeams.add(String(team));
} catch {}
function saveMutes() {
  try {
    if (env.storage) sessionStorage.setItem(MUTES_KEY, JSON.stringify({ everyone, teams: [...mutedTeams] }));
  } catch {}
}

// The team of whoever has `steamid` in the match; for a voice that belongs to no player, the team
// its lines give.
/** @param {string} steamid */
export function teamOf(steamid) {
  for (const p of state.players.values()) if (p.steamid === steamid) return p.team;
  return speakers.get(steamid)?.team ?? null;
}

// Whether `steamid` is muted: by the viewer (kept), with everyone, or with their team (unless
// let through).
/** @param {string} steamid */
export function isMuted(steamid) {
  const s = speakers.get(steamid);
  return mutedAs(steamid, s?.player ?? null, s?.team ?? null);
}

// The same, for a voice as its line describes it: by the player's id, else by the team the line
// gives. With everyone muted, nobody is heard, whoever they are and whenever they first talk. With a
// team muted, a player the match's list does not have yet is muted too: they may be on that team.
/** @param {string} steamid @param {number | null} player @param {string | null} lineTeam */
export function mutedAs(steamid, player, lineTeam) {
  if (prefs.muted.includes(steamid)) return true;
  if (letThrough.has(steamid)) return false;
  if (everyone) return true;
  if (!mutedTeams.size) return false;
  const known = player == null ? null : state.players.get(player);
  if (player != null && !known) return true;
  const team = known?.team ?? teamOf(steamid) ?? lineTeam;
  return team != null && mutedTeams.has(team);
}

/** @param {string} team */
export const isTeamMuted = team => mutedTeams.has(team);

// After the mutes change: sets every speaker's volume and tells the listeners.
function remuted() {
  for (const s of speakers.values()) level(s);
  voiceListeners.forEach(f => f());
}

// A click on someone: if muted, they are heard again (their own mute is lifted, or they are let
// through their team's mute); if heard, they are muted.
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
// Unmuting one after everyone was muted leaves the other team muted, and no longer everyone.
/** @param {string} team @param {boolean} muted */
export function setTeamMuted(team, muted) {
  if (muted) {
    mutedTeams.add(team);
    for (const id of [...letThrough]) if (teamOf(id) === team) letThrough.delete(id);
  } else {
    mutedTeams.delete(team);
    everyone = false;
  }
  saveMutes();
  remuted();
}

// Whether everyone is muted, with nobody let through.
export function allMuted() {
  return everyone && !letThrough.size;
}

// Mutes everyone (those heard and whoever talks later, on a team or not; the viewer's own mutes are
// kept), or nobody (the viewer's own mutes of speakers heard are lifted too).
/** @param {boolean} muted */
export function muteEveryone(muted) {
  letThrough.clear();
  everyone = muted;
  if (muted) {
    mutedTeams.add("red").add("blue");
  } else {
    mutedTeams.clear();
    prefs.muted = prefs.muted.filter(id => !speakers.has(id));
  }
  savePrefs();
  saveMutes();
  remuted();
}

// Hears the other team: with one muted, the two swap; with neither or both muted, RED is heard and BLU muted.
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
