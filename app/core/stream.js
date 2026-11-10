import {
  $,
  DELAY,
  DELAY_MS,
  OVERLAY,
  PAGE,
  parseMessage,
  playbackStatus,
  state,
  observeTick,
  setClockRate,
  setStatus,
  setTickInterval,
  statusText,
} from "./core.js";
import { embedMap, embedStatus } from "../embed/embed.js";
import { asset } from "./env.js";
import { killiconsLoaded, CHAT_TYPES, chat, feedEvent } from "../panel/feed.js";
import { hudNodes } from "../panel/hud.js";
import { setFollow } from "./input.js";
import { detailNeeded } from "./layers.js";
import { cartState } from "../map/objectives/payload.js";
import { onClear, onDraw, onMarker } from "../room/telestrator.js";
import { onArc } from "../room/arcs.js";
import { onCircle } from "../room/circles.js";
import { onNote } from "../room/notes.js";
import { loadMap } from "../map/view.js";
/** @typedef {import("../../types/stream").GameEvent} GameEvent */
/** @typedef {import("../../types/stream").StreamMessage} StreamMessage */
/** @typedef {import("../../types/stream").StatusLine} StatusLine */

// Ticks going back this far are a new map or a server restart, not jitter.
const TICK_RESET = 200;
// Snapshots kept to interpolate in (3.6 s at stvtf's default of 16.7 a second).
const SNAPS_KEPT = 60;

// ---------- incoming stream
// Next to the page, so a match at /w/<id>/ (and its /w/<id>/overlay) reads its own events.
export const EVENTS_URL = asset(PAGE.replace(/(overlay\/?|index\.html)?$/, "") + "events");
// With a delay, everything from the stream is handled that much later, in the order it came
// (timers with the same delay run in order): map changes and the feed so far as well as snapshots.
/** @param {() => void} fn */
const later = fn => (DELAY_MS ? setTimeout(fn, DELAY_MS) : fn());
// Every (re)connect starts with the whole feed and chat so far: replace what is shown rather
// than adding it twice (a deploy or a network blip reconnects).
let opened = false;
// How long the page waits to open its stream again once the browser gave up on it, in ms.
const RETRY_MS = 3000;
// What else the page tells the stream it opens (in a room, the viewer's name).
export const streamParams = new URLSearchParams();
/** @type {EventSource | null} */
let es = null;
let eventsUrl = EVENTS_URL;
// Opens the page's stream, or with `from`, switches it there (a viewer's own playback, and back).
/** @param {string} [from] */
export function openStream(from) {
  es?.close();
  if (from) eventsUrl = from;
  const url = new URL(eventsUrl);
  if (detailNeeded()) url.searchParams.set("detail", "1");
  for (const [k, v] of streamParams) url.searchParams.set(k, v);
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
  // The browser retries a lost connection, but gives up on an error response (a restart or a
  // deploy in between): then the page tries again.
  s.onerror = () => {
    setStatus("stream lost, retrying", false);
    if (s.readyState === EventSource.CLOSED) setTimeout(() => es === s && openStream(), RETRY_MS);
  };
}

// How long ago, from seconds: in seconds, minutes and seconds, or hours and minutes.
/** @param {number} age */
function ago(age) {
  if (age < 60) return `${Math.floor(age)}s`;
  if (age < 3600) return `${Math.floor(age / 60)}m ${Math.floor(age % 60)}s`;
  return `${Math.floor(age / 3600)}h ${Math.floor((age / 60) % 60)}m`;
}
// When the stream goes quiet (the match ended, or the page was opened afterwards), say how long ago
// the last data came in: once none came for QUIET_S.
const QUIET_S = 5;
setInterval(() => {
  const age = state.lastData == null ? 0 : (performance.now() - state.lastData) / 1000;
  $("age").textContent = age < QUIET_S ? "" : `last data ${ago(age)} ago`;
  if (age >= QUIET_S && !state.paused && $("status").textContent === "live") setStatus("no data", false);
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
// How often the hold's countdown is redrawn.
const COUNTDOWN_MS = 250;
function countDown() {
  const empty = $("empty");
  if (empty.style.display === "none" || holdUntil == null) return;
  const left = Math.max(1, Math.ceil((holdUntil - performance.now()) / 1000));
  empty.textContent = OVERLAY
    ? `starting in ${left}s…`
    : `holding the match back ${DELAY}s to line up with the stream: ${left}s…`;
  setTimeout(countDown, COUNTDOWN_MS);
}

// What each kind of room or state line does on the page; any other line is a game event.
/** @type {Record<string, (m: unknown) => void>} */
const ON_LINE = {
  server: m => onServer(/** @type {import("../../types/stream").ServerLine} */ (m)),
  status: m => onStatus(/** @type {import("../../types/stream").StatusLine} */ (m)),
  data_age: m => (state.lastData = performance.now() - /** @type {{ ms: number }} */ (m).ms),
  snapshot: m => onSnapshot(/** @type {import("../../types/stream").Snapshot} */ (m)),
  world: () => {},
  seek: () => {
    // A room jumped: everything shown is from before it.
    $("feed").replaceChildren();
    $("chat").replaceChildren();
    resetMatch(true, true);
    sought = true;
  },
  draw: m => onDraw(/** @type {import("../../types/stream").DrawLine} */ (m)),
  marker: m => onMarker(/** @type {import("../../types/stream").MarkerLine} */ (m)),
  arc: m => onArc(/** @type {import("../../types/stream").ArcLine} */ (m)),
  circle: m => onCircle(/** @type {import("../../types/stream").CircleLine} */ (m)),
  note: m => onNote(/** @type {import("../../types/stream").NoteLine} */ (m)),
  clear: () => onClear(),
  view: m => viewListeners.forEach(f => f(/** @type {import("../../types/stream").ViewLine} */ (m))),
  people: m => peopleListeners.forEach(f => f(/** @type {import("../../types/stream").PeopleLine} */ (m))),
  relay: m => relayListeners.forEach(f => f(/** @type {import("../../types/stream").RelayLine} */ (m))),
  playback: m => onPlayback(/** @type {import("../../types/stream").PlaybackLine} */ (m)),
};

/** @param {GameEvent} m Any line of the stream; its `type` tells which. */
function handle(m) {
  // Feed events from before the page opened: straight into the feed and chat, no map effects.
  if (m.history) {
    const p = /** @param {number} id */ id => state.players.get(id) || m.names?.[id];
    if (CHAT_TYPES.has(m.type)) chat(m, p);
    else killiconsLoaded.finally(() => feedEvent(m, p));
    return;
  }
  if (Object.hasOwn(ON_LINE, m.type)) ON_LINE[m.type](m);
  else state.events.push(m);
}

// The server line starts every (re)connect, and follows a map change.
/** @param {import("../../types/stream").ServerLine} m */
function onServer(m) {
  const sameMap = m.map === state.map;
  if (!sameMap) {
    if (state.map) feedEvent({ type: "map", map: m.map, tick: state.lastTick }, () => null);
    loadMap(m.map);
    embedMap(m.map);
  }
  setTickInterval(m.tick_interval);
  // The same map again (a reconnect, a signon restart) keeps who you are following; after a
  // room's jump, also the HUD and points, which the next snapshot updates instead of rebuilding.
  resetMatch(sameMap, sameMap && sought);
  sought = false;
}
// A room just jumped: the server line that follows is the same match, elsewhere in it.
let sought = false;

// A room's controls hear its playback lines.
/** @type {Set<(m: import("../../types/stream").PlaybackLine) => void>} */
export const playbackListeners = new Set();
/** @type {Set<(m: import("../../types/stream").PeopleLine) => void>} */
export const peopleListeners = new Set();
/** @type {Set<(m: import("../../types/stream").ViewLine) => void>} */
export const viewListeners = new Set();
/** @type {Set<(m: import("../../types/stream").RelayLine) => void>} */
export const relayListeners = new Set();
/** @param {import("../../types/stream").PlaybackLine} m */
function onPlayback(m) {
  setClockRate(m.paused ? 0 : m.speed);
  if (m.paused) {
    state.lastData = performance.now();
    setStatus("paused", true);
  } else if ($("status").textContent === "paused") setStatus("live", true);
  for (const f of playbackListeners) f(m);
}

/** @param {import("../../types/stream").StatusLine} m */
function onStatus(m) {
  embedStatus(m.state);
  if (m.state === "map_change") resetMatch();
  const [text, why] = statusText($("status").textContent ?? "", m);
  // A drop while waiting keeps the reason of the last one in the tooltip.
  setStatus(text, text === "live", why || ($("status").textContent === text ? $("status").title : ""));
}

/** @param {import("../../types/stream").Snapshot} snap */
function onSnapshot(snap) {
  if (snap.tick < state.lastTick - TICK_RESET) resetMatch();
  state.lastTick = snap.tick;
  state.lastData = performance.now();
  // A room's playback says itself where it is.
  if (!playbackStatus($("status").textContent ?? "")) setStatus("live", true);
  observeTick(snap.tick);
  state.snaps.push(snap);
  if (state.snaps.length > SNAPS_KEPT) state.snaps.shift();
  for (const p of snap.players) state.players.set(p.id, p);
  $("empty").style.display = "none";
  $("empty").classList.remove("holding");
}

// A map change (or a signon restart) starts from nothing: drop everything from the old map.
function resetMatch(keepFollow = false, keepHud = false) {
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
  if (keepHud) return;
  $("hud-blue").replaceChildren();
  $("hud-red").replaceChildren();
  hudNodes.clear();
  $("cps").innerHTML = "";
  $("scoreboard").hidden = true;
}
