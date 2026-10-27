import { env, asset } from "./env.js";
// Milliseconds per server tick, from the server line (15 on a 66-tick server, which TF2 runs).
export let msPerTick = 15;
export function setTickInterval(seconds) { if (seconds > 0) msPerTick = seconds * 1000; }
// Playback runs this far behind the newest snapshot beyond the spacing between snapshots, so a
// late one still arrives before it is needed.
const JITTER_MS = 105;
// The tick clock jumps instead of easing when it is off by this much (a stall, a seek).
const CLOCK_JUMP_TICKS = 40;
export const TEAM = { red: "#d0453f", blue: "#4f8fc0", none: "#9a9a9a", spectator: "#9a9a9a" };
export const CLASS = { scout: "Sc", soldier: "So", pyro: "Py", demoman: "De", heavy: "He", engineer: "En", medic: "Me", sniper: "Sn", spy: "Sp", unknown: "?" };
// logs.tf order, as on serveme; same-class players by name so rows don't swap between snapshots.
const CLASS_ORDER = ["scout", "soldier", "pyro", "demoman", "heavy", "engineer", "medic", "sniper", "spy", "unknown"];
const classRank = c => { const i = CLASS_ORDER.indexOf(c); return i < 0 ? CLASS_ORDER.length : i; };
export const byClass = (a, b) => classRank(a.class) - classRank(b.class) || a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || a.id - b.id;

export const ICONS = {};
for (const c of CLASS_ORDER) { if (c !== "unknown") { ICONS[c] = new Image(); ICONS[c].src = asset(`icons/${c}.svg`); } }
export const icon = c => ICONS[c]?.complete && ICONS[c].naturalWidth ? ICONS[c] : null;

// Seconds after the last / (/w/<id>/5, /w/<id>/overlay/5) hold the match back by that much, to line
// up with a stream of it. PAGE is the path without them: /w/<id>/ or /w/<id>/overlay/.
const DELAY_PART = /\/([\d.]+)$/;
export const DELAY = env.path.match(DELAY_PART)?.[1] ?? "";
export const DELAY_MS = (Number(DELAY) || 0) * 1000;
export const PAGE = env.path.replace(DELAY_PART, "/");

// /overlay: a fixed, transparent minimap for broadcasts. Options in the query string:
// names=1, score=1, panel=0..1 (backdrop opacity, none by default), rotate=auto|<degrees>, dot=<px>.
export const OVERLAY = /\/overlay\/?$/.test(PAGE);
export const OPT = new URLSearchParams(env.search);
if (OVERLAY) {
  env.el.classList.add("overlay");
  if (OPT.has("panel")) env.el.style.setProperty("--overlay-opacity", OPT.get("panel"));
}
// Marker size in the overlay follows the panel, so a corner minimap in a 1080p stream (~400px)
// stays readable: player dot radius ~12px there. ui=<factor> scales it, dot=<px> fixes it.
export function overlayUnit() {
  if (OPT.has("dot")) return Number(OPT.get("dot")) || 12;
  const r = canvasSize();
  return Math.max(9, Math.min(20, Math.min(r.width, r.height) / 33)) * (Number(OPT.get("ui")) || 1);
}

// 1080p is the design size; 4K gets the same layout at twice the size.
export function uiScale() { return OVERLAY ? 1 : Math.max(0.75, Math.min(2.5, (env.el.clientHeight || innerHeight) / 1080)); }
export function applyScale() { env.el.style.setProperty("--s", uiScale()); }
applyScale();

export const $ = id => env.root.getElementById(id);
export const canvas = $("c");
export const ctx = canvas.getContext("2d");
// The canvas size, kept up to date by an observer: asking the layout for it forces a reflow.
let size = null;
new ResizeObserver(([e]) => { size = { width: e.contentRect.width, height: e.contentRect.height }; }).observe(canvas);
export const canvasSize = () => size ?? (size = canvas.getBoundingClientRect());

export const state = {
  map: null, snaps: [], events: [], effects: [], players: new Map(),
  tickOffset: null, lastTick: 0, geometry: null, geoCanvas: null, overview: null,
  marks: [], scorches: [], trails: [], packsTaken: new Map(),
  view: { cx: 0, cy: 0, scale: 0.08, rot: 0 }, fitted: false,
  // Auto zoom on the fight; the main view turns it off when panned or zoomed by hand.
  autoZoom: !OVERLAY || OPT.get("zoom") !== "map", bg: "overview", follow: null,
  // The players last drawn, the score line for the feed, rooms seen spawning, when the HUD last refreshed.
  lastPlayers: [], lastScores: null, spawnSeen: {}, lastPanel: 0,
};

// ---------- tick clock: server tick at a local instant, smoothed over snapshot arrivals
function nowTick() {
  return state.tickOffset == null ? 0 : performance.now() / msPerTick + state.tickOffset;
}
// Ticks between snapshots, smoothed: stvtf's rate cap and the STV's tv_snapshotrate set it.
let lastSnapTick = null, snapGap = 4;
export function observeTick(tick) {
  if (state.tickOffset == null) lastSnapTick = null;
  const gap = lastSnapTick == null ? 0 : tick - lastSnapTick;
  if (gap > 0 && gap < CLOCK_JUMP_TICKS) snapGap = snapGap * 0.9 + gap * 0.1;
  lastSnapTick = tick;
  const est = tick - performance.now() / msPerTick;
  if (state.tickOffset == null || Math.abs(est - state.tickOffset) > CLOCK_JUMP_TICKS) state.tickOffset = est;
  else state.tickOffset = Math.max(state.tickOffset * 0.97 + est * 0.03, state.tickOffset - 0.5);
}
export function renderTick() { return nowTick() - snapGap - JITTER_MS / msPerTick; }

export function setStatus(text, ok) { const s = $("status"); s.textContent = text; s.className = ok ? "ok" : "bad"; }
