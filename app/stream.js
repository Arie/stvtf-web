"use strict";
// ---------- incoming stream
// Next to the page, so a match at /w/<id>/ (and its /w/<id>/overlay) reads its own events.
const EVENTS_URL = location.pathname.replace(/(overlay|index\.html)?$/, "") + "events";
// Every (re)connect starts with the whole feed and chat so far: replace what is shown rather
// than adding it twice (a deploy or a network blip reconnects).
let opened = false;
let es = null;
function openStream() {
  es?.close();
  const s = es = new EventSource(EVENTS_URL + (detailNeeded() ? "?detail=1" : ""));
  s.onmessage = e => handle(JSON.parse(e.data));
  s.onopen = () => {
    if (opened) { $("feed").replaceChildren(); $("chat").replaceChildren(); }
    opened = true;
  };
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
      if (m.tick < state.lastTick - 200) resetMatch();
      state.lastTick = m.tick;
      state.lastData = performance.now();
      if ($("status").textContent !== "live") setStatus("live", true);
      observeTick(m.tick);
      state.snaps.push(m);
      if (state.snaps.length > 60) state.snaps.shift();
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
  state.players.clear(); cartState.clear(); lastPlayers = []; state.clock = ""; lastScores = null; spawnSeen = {};
  if (state.follow != null && !keepFollow) setFollow(null);
  $("hud-blue").replaceChildren(); $("hud-red").replaceChildren(); hudNodes.clear(); $("cps").innerHTML = "";
  $("scoreboard").hidden = true;
}
