import { $, DELAY, DELAY_MS, PAGE, state, observeTick, setStatus, setTickInterval } from "./core.js";
import { killiconsLoaded, CHAT_TYPES, chat, feedEvent } from "./feed.js";
import { hudNodes } from "./hud.js";
import { setFollow } from "./input.js";
import { detailNeeded } from "./layers.js";
import { cartState } from "./objectives.js";
import { loadMap } from "./view.js";

// Ticks going back this far are a new map or a server restart, not jitter.
const TICK_RESET = 200;
// Snapshots kept to interpolate in (3.6 s at stvtf's default of 16.7 a second).
const SNAPS_KEPT = 60;

// ---------- incoming stream
// Next to the page, so a match at /w/<id>/ (and its /w/<id>/overlay) reads its own events.
const EVENTS_URL = PAGE.replace(/(overlay\/?|index\.html)?$/, "") + "events";
// With a delay, everything from the stream is handled that much later, in the order it came
// (timers with the same delay run in order): map changes and the feed so far as well as snapshots.
const later = fn => (DELAY_MS ? setTimeout(fn, DELAY_MS) : fn());
// Every (re)connect starts with the whole feed and chat so far: replace what is shown rather
// than adding it twice (a deploy or a network blip reconnects).
let opened = false;
let es = null;
export function openStream() {
  es?.close();
  const s = es = new EventSource(EVENTS_URL + (detailNeeded() ? "?detail=1" : ""));
  s.onmessage = e => { const m = JSON.parse(e.data); waiting(m); later(() => handle(m)); };
  s.onopen = () => later(() => {
    if (opened) { $("feed").replaceChildren(); $("chat").replaceChildren(); }
    opened = true;
  });
  s.onerror = () => setStatus("stream lost, retrying", false);
}

// When the stream goes quiet (the match ended, or the page was opened afterwards), say how long ago
// the last data came in.
setInterval(() => {
  const age = state.lastData == null ? 0 : (performance.now() - state.lastData) / 1000;
  const t = age < 60 ? `${Math.floor(age)}s` : age < 3600 ? `${Math.floor(age / 60)}m ${Math.floor(age % 60)}s` : `${Math.floor(age / 3600)}h ${Math.floor(age / 60 % 60)}m`;
  $("age").textContent = age < 5 ? "" : `last data ${t} ago`;
  if (age >= 5 && $("status").textContent === "live") setStatus("no data", false);
}, 1000);

// Until the first snapshot shows, say what the page is actually waiting for. The first data only
// comes tv_delay late when SourceTV has just started broadcasting (connected, nothing yet); a match
// under way sends its latest snapshot straight away, so then it is only the stream delay, if any.
function waiting(m) {
  const empty = $("empty");
  if (empty.style.display === "none") return;
  if (m.type === "snapshot") {
    if (DELAY_MS) empty.textContent = `holding the match back ${DELAY}s to line up with the stream…`;
  } else if (m.type === "status") {
    empty.textContent =
      m.state === "full" ? "SourceTV has just started: its first data comes tv_delay after that…"
      : m.reason ? `SourceTV ${m.state.replaceAll("_", " ")}: ${m.reason}`
      : "connecting to SourceTV…";
  }
}

function handle(m) {
  // Feed events from before the page opened: straight into the feed and chat, no map effects.
  if (m.history) {
    const p = id => state.players.get(id) || m.names?.[id];
    if (CHAT_TYPES.has(m.type)) chat(m, p);
    else killiconsLoaded.finally(() => feedEvent(m, p));
    return;
  }
  switch (m.type) {
    case "server": {
      const sameMap = m.map === state.map;
      if (!sameMap) {
        if (state.map) feedEvent({ type: "map", map: m.map }, () => null);
        loadMap(m.map);
      }
      setTickInterval(m.tick_interval);
      // The same map again (a reconnect, a signon restart) keeps who you are following.
      resetMatch(sameMap);
      break;
    }
    case "status":
      if (m.state === "map_change") { resetMatch(); setStatus("map changing", false); }
      else if (m.state === "full") setStatus("live", true);
      else if (m.state === "disconnected" || m.state === "connect_failed") setStatus(m.state + ": " + (m.reason || ""), false);
      else setStatus(m.state, false);
      break;
    case "data_age":
      state.lastData = performance.now() - m.ms;
      break;
    case "snapshot":
      if (m.tick < state.lastTick - TICK_RESET) resetMatch();
      state.lastTick = m.tick;
      state.lastData = performance.now();
      if ($("status").textContent !== "live") setStatus("live", true);
      observeTick(m.tick);
      state.snaps.push(m);
      if (state.snaps.length > SNAPS_KEPT) state.snaps.shift();
      for (const p of m.players) state.players.set(p.id, p);
      $("empty").style.display = "none";
      break;
    case "world":
      break;
    default:
      state.events.push(m);
  }
}

// A map change (or a signon restart) starts from nothing: drop everything from the old map.
function resetMatch(keepFollow = false) {
  state.snaps = []; state.events = []; state.effects = []; state.marks = []; state.scorches = []; state.trails = []; state.packsTaken.clear(); state.tickOffset = null; state.lastTick = 0;
  state.players.clear(); cartState.clear(); state.lastPlayers = []; state.clock = ""; state.lastScores = null; state.spawnSeen = {};
  if (state.follow != null && !keepFollow) setFollow(null);
  $("hud-blue").replaceChildren(); $("hud-red").replaceChildren(); hudNodes.clear(); $("cps").innerHTML = "";
  $("scoreboard").hidden = true;
}
