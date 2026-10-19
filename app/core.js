"use strict";
const MS_PER_TICK = 15;
const BUFFER_TICKS = 10;
const TEAM = { red: "#d0453f", blue: "#4f8fc0", none: "#9a9a9a", spectator: "#9a9a9a" };
const CLASS = { scout: "Sc", soldier: "So", pyro: "Py", demoman: "De", heavy: "He", engineer: "En", medic: "Me", sniper: "Sn", spy: "Sp", unknown: "?" };
// logs.tf order, as on serveme; same-class players by name so rows don't swap between snapshots.
const CLASS_ORDER = ["scout", "soldier", "pyro", "demoman", "heavy", "engineer", "medic", "sniper", "spy", "unknown"];
const classRank = c => { const i = CLASS_ORDER.indexOf(c); return i < 0 ? CLASS_ORDER.length : i; };
const byClass = (a, b) => classRank(a.class) - classRank(b.class) || a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || a.id - b.id;

const ICONS = {};
for (const c of CLASS_ORDER) { if (c !== "unknown") { ICONS[c] = new Image(); ICONS[c].src = `icons/${c}.svg`; } }
const icon = c => ICONS[c]?.complete && ICONS[c].naturalWidth ? ICONS[c] : null;

// /overlay: a fixed, transparent minimap for broadcasts. Options in the query string:
// names=1, score=1, panel=0..1 (background opacity), rotate=auto|<degrees>, dot=<px>.
const OVERLAY = /\/overlay\/?$/.test(location.pathname);
const OPT = new URLSearchParams(location.search);
if (OVERLAY) {
  document.documentElement.classList.add("overlay");
  if (OPT.has("panel")) document.documentElement.style.setProperty("--overlay-opacity", OPT.get("panel"));
}
// Marker size in the overlay follows the panel, so a corner minimap in a 1080p stream (~400px)
// stays readable: player dot radius ~12px there. ui=<factor> scales it, dot=<px> fixes it.
function overlayUnit() {
  if (OPT.has("dot")) return Number(OPT.get("dot")) || 12;
  const r = canvas.getBoundingClientRect();
  return Math.max(9, Math.min(20, Math.min(r.width, r.height) / 33)) * (Number(OPT.get("ui")) || 1);
}

// 1080p is the design size; 4K gets the same layout at twice the size.
function uiScale() { return OVERLAY ? 1 : Math.max(0.75, Math.min(2.5, innerHeight / 1080)); }
function applyScale() { document.documentElement.style.setProperty("--s", uiScale()); }
applyScale();
// A resize (or hiding the events) only refits the whole-map framing; auto zoom glides to it.
addEventListener("resize", () => { applyScale(); if (state.framed) frameMap(true); });

const canvas = document.getElementById("c");
const ctx = canvas.getContext("2d");
const $ = id => document.getElementById(id);

const state = {
  map: null, snaps: [], events: [], effects: [], players: new Map(),
  tickOffset: null, lastTick: 0, geometry: null, geoCanvas: null, overview: null,
  marks: [], scorches: [], trails: [], packsTaken: new Map(),
  view: { cx: 0, cy: 0, scale: 0.08, rot: 0 }, fitted: false,
  // Auto zoom on the fight; the main view turns it off when panned or zoomed by hand.
  autoZoom: !OVERLAY || OPT.get("zoom") !== "map", bg: "overview", follow: null,
};

// ---------- tick clock: server tick at a local instant, smoothed over snapshot arrivals
function nowTick() {
  return state.tickOffset == null ? 0 : performance.now() / MS_PER_TICK + state.tickOffset;
}
function observeTick(tick) {
  const est = tick - performance.now() / MS_PER_TICK;
  if (state.tickOffset == null || Math.abs(est - state.tickOffset) > 40) state.tickOffset = est;
  else state.tickOffset = Math.max(state.tickOffset * 0.97 + est * 0.03, state.tickOffset - 0.5);
}
function renderTick() { return nowTick() - BUFFER_TICKS; }

function setStatus(text, ok) { const s = $("status"); s.textContent = text; s.className = ok ? "ok" : "bad"; }
