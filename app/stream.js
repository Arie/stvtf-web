import {
  $,
  DELAY,
  DELAY_MS,
  OVERLAY,
  PAGE,
  parseMessage,
  state,
  observeTick,
  setClockRate,
  setStatus,
  setTickInterval,
  statusText,
} from "./core.js";
import { embedMap, embedStatus } from "./embed.js";
import { asset } from "./env.js";
import { killiconsLoaded, CHAT_TYPES, chat, feedEvent } from "./feed.js";
import { hudNodes } from "./hud.js";
import { setFollow } from "./input.js";
import { detailNeeded } from "./layers.js";
import { cartState } from "./payload.js";
import { loadMap } from "./view.js";
/** @typedef {import("../types/stream").GameEvent} GameEvent */
/** @typedef {import("../types/stream").StreamMessage} StreamMessage */
/** @typedef {import("../types/stream").StatusLine} StatusLine */

// Ticks going back this far are a new map or a server restart, not jitter.
const TICK_RESET = 200;
// Snapshots kept to interpolate in (3.6 s at stvtf's default of 16.7 a second).
const SNAPS_KEPT = 60;

// ---------- incoming stream
// Next to the page, so a match at /w/<id>/ (and its /w/<id>/overlay) reads its own events.
const EVENTS_URL = asset(PAGE.replace(/(overlay\/?|index\.html)?$/, "") + "events");
// With a delay, everything from the stream is handled that much later, in the order it came
// (timers with the same delay run in order): map changes and the feed so far as well as snapshots.
/** @param {() => void} fn */
const later = fn => (DELAY_MS ? setTimeout(fn, DELAY_MS) : fn());
// Every (re)connect starts with the whole feed and chat so far: replace what is shown rather
// than adding it twice (a deploy or a network blip reconnects).
let opened = false;
/** @type {EventSource | null} */
let es = null;
export function openStream() {
  es?.close();
  const url = new URL(EVENTS_URL);
  if (detailNeeded()) url.searchParams.set("detail", "1");
  const s = (es = new EventSource(url));
  s.onmessage = e => {
    const m = parseMessage(e.data);
    waiting(m);
    // handle() reads every kind of line by its `type`, as loosely as an event.
    later(() => handle(/** @type {GameEvent} */ (m)));
  };
  s.onopen = () =>
    later(() => {
      if (opened) {
        $("feed").replaceChildren();
        $("chat").replaceChildren();
      }
      opened = true;
    });
  s.onerror = () => setStatus("stream lost, retrying", false);
}

// How long ago, from seconds: in seconds, minutes and seconds, or hours and minutes.
/** @param {number} age */
function ago(age) {
  if (age < 60) return `${Math.floor(age)}s`;
  if (age < 3600) return `${Math.floor(age / 60)}m ${Math.floor(age % 60)}s`;
  return `${Math.floor(age / 3600)}h ${Math.floor((age / 60) % 60)}m`;
}
// When the stream goes quiet (the match ended, or the page was opened afterwards), say how long ago
// the last data came in.
setInterval(() => {
  const age = state.lastData == null ? 0 : (performance.now() - state.lastData) / 1000;
  $("age").textContent = age < 5 ? "" : `last data ${ago(age)} ago`;
  if (age >= 5 && !state.paused && $("status").textContent === "live") setStatus("no data", false);
}, 1000);

// Until the first snapshot shows, say what the page is actually waiting for. The first data only
// comes tv_delay late when SourceTV has just started broadcasting (connected, nothing yet); a match
// under way sends its latest snapshot straight away, so then it is only the stream delay, if any.
// A delay counts down to when that snapshot shows; it is all the overlay says while it waits, so a
// delayed overlay is not an empty panel until then.
/** @type {number | null} */
let holdUntil = null;
/** @param {StreamMessage} m */
function waiting(m) {
  const empty = $("empty");
  if (empty.style.display === "none") return;
  if (m.type === "snapshot") {
    if (DELAY_MS && holdUntil == null) {
      holdUntil = performance.now() + DELAY_MS;
      empty.classList.add("holding");
      countDown();
    }
  } else if (m.type === "status" && holdUntil == null) {
    empty.textContent = waitingFor(/** @type {StatusLine} */ (m));
  }
}
// What a status line says SourceTV is up to, while there is no data yet.
/** @param {StatusLine} m */
function waitingFor(m) {
  if (m.state === "full") return "SourceTV has just started: its first data comes tv_delay after that…";
  return m.reason ? `SourceTV ${m.state.replaceAll("_", " ")}: ${m.reason}` : "connecting to SourceTV…";
}
function countDown() {
  const empty = $("empty");
  if (empty.style.display === "none" || holdUntil == null) return;
  const left = Math.max(1, Math.ceil((holdUntil - performance.now()) / 1000));
  empty.textContent = OVERLAY
    ? `starting in ${left}s…`
    : `holding the match back ${DELAY}s to line up with the stream: ${left}s…`;
  setTimeout(countDown, 250);
}

/** @param {GameEvent} m Any line of the stream; its `type` tells which. */
function handle(m) {
  // Feed events from before the page opened: straight into the feed and chat, no map effects.
  if (m.history) {
    const p = /** @param {number} id */ id => state.players.get(id) || m.names?.[id];
    if (CHAT_TYPES.has(m.type)) chat(m, p);
    else killiconsLoaded.finally(() => feedEvent(m, p));
    return;
  }
  switch (m.type) {
    case "server":
      onServer(/** @type {import("../types/stream").ServerLine} */ (/** @type {unknown} */ (m)));
      break;
    case "status":
      onStatus(/** @type {import("../types/stream").StatusLine} */ (/** @type {unknown} */ (m)));
      break;
    case "data_age":
      state.lastData = performance.now() - m.ms;
      break;
    case "snapshot":
      onSnapshot(/** @type {import("../types/stream").Snapshot} */ (m));
      break;
    case "world":
      break;
    case "seek":
      // A room jumped: everything shown is from before it.
      $("feed").replaceChildren();
      $("chat").replaceChildren();
      resetMatch(true);
      break;
    case "playback":
      onPlayback(/** @type {import("../types/stream").PlaybackLine} */ (/** @type {unknown} */ (m)));
      break;
    default:
      state.events.push(m);
  }
}

// The server line starts every (re)connect, and follows a map change.
/** @param {import("../types/stream").ServerLine} m */
function onServer(m) {
  const sameMap = m.map === state.map;
  if (!sameMap) {
    if (state.map) feedEvent({ type: "map", map: m.map, tick: state.lastTick }, () => null);
    loadMap(m.map);
    embedMap(m.map);
  }
  setTickInterval(m.tick_interval);
  // The same map again (a reconnect, a signon restart) keeps who you are following.
  resetMatch(sameMap);
}

// A room's controls hear its playback lines.
/** @type {Set<(m: import("../types/stream").PlaybackLine) => void>} */
export const playbackListeners = new Set();
/** @param {import("../types/stream").PlaybackLine} m */
function onPlayback(m) {
  setClockRate(m.paused ? 0 : m.speed);
  if (m.paused) {
    state.lastData = performance.now();
    setStatus("paused", true);
  } else if ($("status").textContent === "paused") setStatus("live", true);
  for (const f of playbackListeners) f(m);
}

/** @param {import("../types/stream").StatusLine} m */
function onStatus(m) {
  embedStatus(m.state);
  if (m.state === "map_change") resetMatch();
  const [text, why] = statusText($("status").textContent ?? "", m);
  // A drop while waiting keeps the reason of the last one in the tooltip.
  setStatus(text, text === "live", why || ($("status").textContent === text ? $("status").title : ""));
}

/** @param {import("../types/stream").Snapshot} snap */
function onSnapshot(snap) {
  if (snap.tick < state.lastTick - TICK_RESET) resetMatch();
  state.lastTick = snap.tick;
  state.lastData = performance.now();
  if ($("status").textContent !== "live") setStatus("live", true);
  observeTick(snap.tick);
  state.snaps.push(snap);
  if (state.snaps.length > SNAPS_KEPT) state.snaps.shift();
  for (const p of snap.players) state.players.set(p.id, p);
  $("empty").style.display = "none";
  $("empty").classList.remove("holding");
}

// A map change (or a signon restart) starts from nothing: drop everything from the old map.
function resetMatch(keepFollow = false) {
  state.snaps = [];
  state.events = [];
  state.effects = [];
  state.marks = [];
  state.scorches = [];
  state.trails = [];
  state.packsTaken.clear();
  state.tickOffset = null;
  state.lastTick = 0;
  state.players.clear();
  cartState.clear();
  state.lastPlayers = [];
  state.clock = "";
  state.lastScores = null;
  state.spawnSeen = {};
  if (state.follow != null && !keepFollow) setFollow(null);
  $("hud-blue").replaceChildren();
  $("hud-red").replaceChildren();
  hudNodes.clear();
  $("cps").innerHTML = "";
  $("scoreboard").hidden = true;
}
