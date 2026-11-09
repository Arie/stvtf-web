import { state } from "../core/core.js";
import { env, asset } from "../core/env.js";
import { KILLICONS, killDetails } from "../panel/feed.js";
import { setFollow } from "../core/input.js";
import { layerOn, setLayer } from "../core/layers.js";
/** @typedef {import("../../types/stream").GameEvent} GameEvent */
/** @typedef {import("../../types/stream").Player} Player */
/**
 * What the parent page can ask for.
 * @typedef {{ type: "follow"; steamid?: string | null; steamid64?: string | number | null; id?: number }
 *   | { type: "layer"; name: string; on?: unknown }} EmbedCommand
 */

// On another site, in an iframe or as <stv-map>: what happens goes to that page as the map shows it
// (so after any delay), as postMessage or as the element's events, and the page can follow a
// player or switch a layer. Every message carries `stvtf: 1`. See docs/embedding.md.
export const EMBEDDED = Boolean(env.emit) || window.parent !== window;
// What the page's own feed and chat show; not the effects drawn on the map.
const EFFECTS = new Set([
  "shot",
  "explosion",
  "airblast",
  "sentry_shot",
  "damage",
  "spawn",
  "heal",
  "impact",
  "mark",
  "blast_jump",
  "pickup",
  "building_upgrade",
  "building_repair",
  "time_added",
]);
const PLAYER_FIELDS = [
  "victim",
  "attacker",
  "assister",
  "medic",
  "target",
  "player",
  "carrier",
  "owner",
  "blocker",
  "killer",
];
const STATE_MS = 500;

/** @param {{ type: string; [field: string]: unknown }} m */
const post = m => {
  if (env.emit) env.emit(/** @type {{ type: string }} */ ({ stvtf: 1, ...m }));
  else if (EMBEDDED) window.parent.postMessage({ stvtf: 1, ...m }, "*");
};
const STEAMID64_BASE = 76561197960265728n;
/** @param {string | null | undefined} id */
export const steamid64 = id => {
  const n = /^\[U:1:(\d+)\]$/.exec(id || "")?.[1];
  return n ? String(STEAMID64_BASE + BigInt(n)) : null;
};
// Absolute, since the parent page loads them from its own origin.
const SPRITE = asset("killicons/v1/killicons.webp");
// Item images from the backpack, by item definition index.
/** @type {{ [item: string]: string }} */
let itemImages = {};
if (EMBEDDED)
  fetch(asset("backpack/items.json"))
    .then(r => r.json())
    .then(j => {
      itemImages = j;
    })
    .catch(() => {});
/** @param {Player["weapon"]} w */
const weapon = w =>
  w && {
    item: w.item,
    clip: w.clip ?? null,
    image: itemImages[w.item] ? asset(`backpack/${itemImages[w.item]}.webp`) : null,
  };
/** @param {Player | null | undefined} p */
const who = p =>
  p && { id: p.id, name: p.name, steamid: p.steamid, steamid64: steamid64(p.steamid), team: p.team, class: p.class };

/** @param {string} map */
export function embedMap(map) {
  post({ type: "map", map });
}
/** @param {string} s */
export function embedStatus(s) {
  post({ type: "status", state: s });
}

/** @param {GameEvent} ev @param {Map<number, Player>} byId */
export function embedEvent(ev, byId) {
  if (!EMBEDDED || EFFECTS.has(ev.type)) return;
  const ids = [...PLAYER_FIELDS.map(f => ev[f]), ...(ev.cappers || [])].filter(id => id != null);
  const pl = /** @param {number} id */ id => byId.get(id) || state.players.get(id);
  const players = Object.fromEntries(ids.map(id => [id, who(pl(id))]).filter(([, p]) => p));
  if (ev.type === "kill") {
    const k = killDetails(ev, pl),
      box = k.icon && KILLICONS.get(k.icon);
    ev = { ...ev, icon: k.icon, icon_box: box ? { url: SPRITE, ...box } : null, streak: k.streak, airshot: k.airshot };
  }
  post({ type: "event", event: ev, players });
}

let lastState = 0;
/** @param {{ red: number; blue: number } | null} score @param {Player[]} players */
export function embedState(score, players) {
  const now = performance.now();
  if (!EMBEDDED || now - lastState < STATE_MS) return;
  lastState = now;
  post({
    type: "state",
    map: state.map,
    score,
    players: players.map(p => ({
      ...who(p),
      alive: p.alive,
      hp: p.hp,
      max_hp: p.max_hp,
      weapon: p.alive ? weapon(p.weapon) : null,
    })),
  });
}

// Whether a player is the one a command names: by Steam ID, 64-bit Steam ID, or id.
/** @param {Player} x @param {EmbedCommand & { type: "follow" }} m */
function isPlayer(x, m) {
  if (m.steamid != null) return x.steamid === m.steamid;
  return m.steamid64 != null ? steamid64(x.steamid) === String(m.steamid64) : x.id === m.id;
}
// What the parent page asks for: `follow` a player, switch a `layer`.
/** @param {EmbedCommand} m */
export function command(m) {
  if (m.type === "follow") {
    const p = state.lastPlayers.find(x => isPlayer(x, m));
    if (!p) setFollow(null);
    else if (p.id !== state.follow) setFollow(p.id);
  } else if (m.type === "layer" && layerOn(m.name) != null) setLayer(m.name, Boolean(m.on));
}
window.addEventListener("message", e => {
  if (!env.component && e.source === window.parent && e.data?.stvtf === 1) command(e.data);
});

post({ type: "ready" });
