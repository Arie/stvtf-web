import { state } from "./core.js";
import { setFollow } from "./input.js";
import { layerOn, setLayer } from "./layers.js";

// In a site's iframe: what happens goes to the parent page as postMessage, as the map shows it (so
// after any delay), and the parent can follow a player or switch a layer. Every message carries
// `stvtf: 1`. See "Embedding" in the README.
const EMBEDDED = window.parent !== window;
const EVENTS = new Set(["kill", "uber", "point_captured", "capture_broken", "flag", "round_start", "round_win", "chat"]);
const PLAYER_FIELDS = ["victim", "attacker", "assister", "medic", "target", "player", "carrier"];
const STATE_MS = 500;

const post = m => { if (EMBEDDED) window.parent.postMessage({ stvtf: 1, ...m }, "*"); };
const STEAMID64_BASE = 76561197960265728n;
const steamid64 = id => { const n = /^\[U:1:(\d+)\]$/.exec(id || "")?.[1]; return n ? String(STEAMID64_BASE + BigInt(n)) : null; };
const who = p => p && { id: p.id, name: p.name, steamid: p.steamid, steamid64: steamid64(p.steamid), team: p.team, class: p.class };

export function embedMap(map) { post({ type: "map", map }); }
export function embedStatus(s) { post({ type: "status", state: s }); }

export function embedEvent(ev, byId) {
  if (!EMBEDDED || !EVENTS.has(ev.type)) return;
  const ids = [...PLAYER_FIELDS.map(f => ev[f]), ...(ev.cappers || [])].filter(id => id != null);
  const players = Object.fromEntries(ids.map(id => [id, who(byId.get(id) || state.players.get(id))]).filter(([, p]) => p));
  post({ type: "event", event: ev, players });
}

let lastState = 0;
export function embedState(score, players) {
  const now = performance.now();
  if (!EMBEDDED || now - lastState < STATE_MS) return;
  lastState = now;
  post({ type: "state", map: state.map, score, players: players.map(p => ({ ...who(p), alive: p.alive, hp: p.hp, max_hp: p.max_hp })) });
}

window.addEventListener("message", e => {
  const m = e.data;
  if (e.source !== window.parent || m?.stvtf !== 1) return;
  if (m.type === "follow") {
    const p = state.lastPlayers.find(x => m.steamid != null ? x.steamid === m.steamid : m.steamid64 != null ? steamid64(x.steamid) === String(m.steamid64) : x.id === m.id);
    if (!p) setFollow(null);
    else if (p.id !== state.follow) setFollow(p.id);
  } else if (m.type === "layer" && layerOn(m.name) != null) setLayer(m.name, Boolean(m.on));
});

post({ type: "ready" });
