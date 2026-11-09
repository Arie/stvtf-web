import { env, asset } from "./env.js";
/** @typedef {import("../../types/stream").Player} Player */
/** @typedef {import("../../types/stream").PlacedObjective} PlacedObjective */
// Milliseconds per server tick, from the server line (15 on a 66-tick server, which TF2 runs).
export let msPerTick = 15;
/** @param {number} seconds */
export function setTickInterval(seconds) {
  if (seconds > 0) msPerTick = seconds * 1000;
}
// Playback runs this far behind the newest snapshot beyond the spacing between snapshots, so a
// late one still arrives before it is needed.
const JITTER_MS = 105;
// The tick clock jumps instead of easing when it is off by this much (a stall, a seek).
const CLOCK_JUMP_TICKS = 40;
// The near-black of outlines and lettering on the map, and the yellow of a neutral flag or ball.
export const INK = "#0b0c0e",
  NEUTRAL = "#f2c94c";
/** @type {{ [team: string]: string }} */
export const TEAM = { red: "#d0453f", blue: "#4f8fc0", none: "#9a9a9a", spectator: "#9a9a9a" };
// A team's colour, or `or` for none (or an unknown one).
/** @param {string | number | null | undefined} team */
export const teamColour = (team, or = TEAM.none) => (team != null && TEAM[team]) || or;
/** @type {{ [cls: string]: string }} */
export const CLASS = {
  scout: "Sc",
  soldier: "So",
  pyro: "Py",
  demoman: "De",
  heavy: "He",
  engineer: "En",
  medic: "Me",
  sniper: "Sn",
  spy: "Sp",
  unknown: "?",
};
// logs.tf order, as on serveme; same-class players by name so rows don't swap between snapshots.
const CLASS_ORDER = ["scout", "soldier", "pyro", "demoman", "heavy", "engineer", "medic", "sniper", "spy", "unknown"];
/** @param {string} c */
const classRank = c => {
  const i = CLASS_ORDER.indexOf(c);
  return i < 0 ? CLASS_ORDER.length : i;
};
/** @param {Player} a @param {Player} b */
export const byClass = (a, b) =>
  classRank(a.class) - classRank(b.class) ||
  a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) ||
  a.id - b.id;

// Invisible characters a name or chat line could hide behind: zero-width spaces, direction marks
// and overrides. The zero-width joiner and non-joiner stay: emoji and some scripts need them.
const INVISIBLE = /[\u061c\u180e\u200b\u200e\u200f\u202a-\u202e\u2060-\u2064\u2066-\u2069\ufeff]/g;
const INVISIBLE_ESCAPED = /\\u(061c|180e|200[bef]|202[a-e]|206[0-46-9]|feff)/i;
// A message from the stream, without them. Most have none, and skip the slower parse.
/** @type {(data: string) => import("../../types/stream").StreamMessage} */
export const parseMessage = data =>
  data.search(INVISIBLE) >= 0 || INVISIBLE_ESCAPED.test(data)
    ? JSON.parse(data, (_, v) => (typeof v === "string" ? v.replace(INVISIBLE, "") : v))
    : JSON.parse(data);

/** @type {{ [cls: string]: HTMLImageElement }} */
export const ICONS = {};
// Loaded with CORS (every response allows it), so the players layer can read their pixels.
for (const c of CLASS_ORDER) {
  if (c !== "unknown") {
    ICONS[c] = new Image();
    ICONS[c].crossOrigin = "anonymous";
    ICONS[c].src = asset(`icons/${c}.svg`);
  }
}
/** @param {string} c */
export const icon = c => (ICONS[c]?.complete && ICONS[c].naturalWidth ? ICONS[c] : null);

// Seconds after the last / (/w/<id>/5, /w/<id>/overlay/5) hold the match back by that much, to line
// up with a stream of it. PAGE is the path without them: /w/<id>/ or /w/<id>/overlay/.
const DELAY_PART = /\/([\d.]+)$/;
export const DELAY = env.path.match(DELAY_PART)?.[1] ?? "";
export const DELAY_MS = (Number(DELAY) || 0) * 1000;
export const PAGE = env.path.replace(DELAY_PART, "/");

// /overlay: a fixed, transparent minimap for broadcasts. Options in the query string:
// panel=0..1 (backdrop opacity, none by default), rotate=auto|<degrees>, dot=<px>.
export const OVERLAY = /\/overlay\/?$/.test(PAGE);
export const OPT = new URLSearchParams(env.search);
// A display toggle in the query string, on either view: 1 on, 0 off, anything else the default.
/** @param {string} name */
export const optOn = (name, def = true) => {
  const v = OPT.get(name);
  return v === "1" || (v !== "0" && def);
};
// bg=flat or bg=outline: the map geometry drawn plainly, in place of its usual look.
export const PLAIN_BG = (bg => (bg === "flat" || bg === "outline" ? bg : null))(OPT.get("bg"));
if (OVERLAY) {
  env.el.classList.add("overlay");
  if (OPT.has("panel")) env.el.style.setProperty("--overlay-opacity", OPT.get("panel"));
}
// Marker size in the overlay follows the panel, so a corner minimap in a 1080p stream (~400px)
// stays readable: player dot radius ~12px there. ui=<factor> scales it, dot=<px> fixes it.
const DOT_PX = 12,
  // The dot's radius: the panel's shorter side over this, kept between these.
  DOT_OF_PANEL = 33,
  [DOT_MIN_PX, DOT_MAX_PX] = [9, 20];
export function overlayUnit() {
  if (OPT.has("dot")) return Number(OPT.get("dot")) || DOT_PX;
  const r = canvasSize();
  const dot = Math.min(r.width, r.height) / DOT_OF_PANEL;
  return Math.max(DOT_MIN_PX, Math.min(DOT_MAX_PX, dot)) * (Number(OPT.get("ui")) || 1);
}

// 1080p is the design size; 4K gets the same layout at twice the size. The page's height is asked
// of the layout once a resize (applyScale), not every frame: after the bar's changes, that reflows.
let height = 0;
const DESIGN_HEIGHT = 1080,
  [MIN_UI_SCALE, MAX_UI_SCALE] = [0.75, 2.5];
export function uiScale() {
  return OVERLAY ? 1 : Math.max(MIN_UI_SCALE, Math.min(MAX_UI_SCALE, (height || innerHeight) / DESIGN_HEIGHT));
}
export function applyScale() {
  height = env.el.clientHeight;
  env.el.style.setProperty("--s", String(uiScale()));
}
applyScale();

// What changes the page each frame, after the map is drawn: changed before, the map's text would
// wait for the page's styles.
/** @type {Set<() => void>} */
export const afterDraw = new Set();

// The viewer's elements are all in its page (or the component's template), so they are there.
/** @param {string} id */
export const $ = id => /** @type {HTMLElement} */ (env.root.getElementById(id));
export const canvas = /** @type {HTMLCanvasElement} */ ($("c"));
// A canvas's 2D context. Only null if the canvas already has another kind, which none here has.
/** @param {HTMLCanvasElement} c @param {CanvasRenderingContext2DSettings} [opts] */
export function context2d(c, opts) {
  const g = c.getContext("2d", opts);
  if (!g) throw new Error("no 2D canvas context");
  return g;
}
export const ctx = context2d(canvas);
// The canvas size, kept up to date by an observer: asking the layout for it forces a reflow.
/** @type {{ width: number; height: number } | null} */
let size = null;
new ResizeObserver(([e]) => {
  size = { width: e.contentRect.width, height: e.contentRect.height };
}).observe(canvas);
export const canvasSize = () => size ?? (size = canvas.getBoundingClientRect());

/** @type {import("../../types/page").State} */
export const state = {
  map: null,
  snaps: [],
  events: [],
  effects: [],
  players: new Map(),
  tickOffset: null,
  paused: false,
  lastTick: 0,
  geometry: null,
  geoCanvas: null,
  overview: null,
  marks: [],
  scorches: [],
  trails: [],
  packsTaken: new Map(),
  view: { cx: 0, cy: 0, scale: 0.08, rot: 0 },
  fitted: false,
  // Auto zoom on the fight; the main view turns it off when panned or zoomed by hand.
  autoZoom: !OVERLAY || OPT.get("zoom") !== "map",
  bg: "overview",
  follow: null,
  locked: false,
  handMoved: 0,
  followZoom: null,
  mirror: null,
  // The players last drawn, the score line for the feed, rooms seen spawning, when the HUD last refreshed.
  lastPlayers: [],
  lastScores: null,
  spawnSeen: {},
  lastPanel: 0,
};

// ---------- tick clock: server tick at a local instant, smoothed over snapshot arrivals
// Ticks per local tick's worth of time: a room's playback speed, 0 while paused.
let rate = 1;
function nowTick() {
  return state.tickOffset == null ? 0 : (performance.now() * rate) / msPerTick + state.tickOffset;
}
/** @param {number} r */
export function setClockRate(r) {
  if (state.tickOffset != null) state.tickOffset = nowTick() - (performance.now() * r) / msPerTick;
  rate = r;
  state.paused = r === 0;
}
// How much of each new reading the smoothed snapshot gap and clock offset take.
const GAP_SMOOTHING = 0.1,
  CLOCK_SMOOTHING = 0.03,
  // The clock is never set back by more than this many ticks a snapshot: it slows, never jumps back.
  CLOCK_MAX_BACK = 0.5;
// Ticks between snapshots, smoothed: stvtf's rate cap and the STV's tv_snapshotrate set it.
/** @type {number | null} */
let lastSnapTick = null,
  snapGap = 4;
/** @param {number} tick */
export function observeTick(tick) {
  if (state.tickOffset == null) lastSnapTick = null;
  const gap = lastSnapTick == null ? 0 : tick - lastSnapTick;
  if (gap > 0 && gap < CLOCK_JUMP_TICKS) snapGap += (gap - snapGap) * GAP_SMOOTHING;
  lastSnapTick = tick;
  // Paused, a snapshot only comes from a seek: show it as it is.
  if (rate === 0) {
    state.tickOffset = tick + snapGap + JITTER_MS / msPerTick;
    return;
  }
  const est = tick - (performance.now() * rate) / msPerTick;
  if (state.tickOffset == null || Math.abs(est - state.tickOffset) > CLOCK_JUMP_TICKS) state.tickOffset = est;
  else
    state.tickOffset = Math.max(
      state.tickOffset + (est - state.tickOffset) * CLOCK_SMOOTHING,
      state.tickOffset - CLOCK_MAX_BACK,
    );
}
export function renderTick() {
  return nowTick() - snapGap - JITTER_MS / msPerTick;
}

/** @param {string} text @param {boolean} ok */
export function setStatus(text, ok, why = "") {
  const s = $("status");
  s.textContent = text;
  s.className = ok ? "ok" : "bad";
  s.title = why;
}

// Whether the status says how playback goes: live, paused, a replay, or how far behind SourceTV.
/** @param {string} text */
export const playbackStatus = text => ["live", "paused", "replay"].includes(text) || text.startsWith("−");

// The sidebar's word for a status line from stvtf, given the word it shows now. Signon runs
// connecting, new, prespawn and spawn before data flows, and a server that is starting up or
// changing map can let SourceTV in and drop it several times first: all of that keeps one word
// until the match is live, instead of flicking through the steps. Returns [text, why]; the
// reason a connection failed is the why, for the tooltip.
/** @param {string} shown @param {import("../../types/stream").StatusLine} m */
export function statusText(shown, m) {
  const why = m.reason || "";
  switch (m.state) {
    case "full":
      return ["live", ""];
    case "map_change":
    case "changelevel":
      return ["map changing", ""];
    case "gave_up":
      return [`gave up: ${why}`, why];
    case "disconnected":
    case "connect_failed":
      if (shown === "map changing") return [shown, why];
      return [
        shown === "live" || shown === "no data" || shown === "reconnecting" ? "reconnecting" : "waiting for SourceTV",
        why,
      ];
    default:
      // A step of signon (or a relay coming up): whatever it was waiting for, it still is.
      if (shown === "live" || shown === "no data") return ["reconnecting", ""];
      return [WAITING.has(shown) ? shown : "connecting", ""];
  }
}
const WAITING = new Set(["connecting", "reconnecting", "map changing", "waiting for SourceTV"]);

// Where a moving objective (cart, ball, robot, tank) is between the snapshots around `tick`.
/** @param {PlacedObjective} o @param {number} tick */
export function objPos(o, tick) {
  const s = state.snaps;
  const i = s.findIndex(x => x.tick > tick);
  if (i < 1) return o.pos;
  const a = s[i - 1].objectives?.find(q => q.kind === o.kind && q.id === o.id);
  const b = s[i].objectives?.find(q => q.kind === o.kind && q.id === o.id);
  if (!a?.pos || !b?.pos) return o.pos;
  const t = (tick - s[i - 1].tick) / (s[i].tick - s[i - 1].tick);
  return [a.pos[0] + (b.pos[0] - a.pos[0]) * t, a.pos[1] + (b.pos[1] - a.pos[1]) * t];
}

// A timer's seconds left as TF2's HUD shows them, m:ss.
/** @param {number} secs */
export function clock(secs) {
  const s = Math.max(0, Math.ceil(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
