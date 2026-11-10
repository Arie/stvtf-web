// A room's telestrator: admins' strokes in map coordinates. Pen fades after a few seconds;
// marker stays until cleared.
import { INK, TEAM, ctx, uiScale } from "../core/core.js";
import { toScreen } from "../map/view.js";
import { arcsFromMarker, clearArcs, drawArcs } from "./arcs.js";
import { circlesFromMarker, clearCircles, drawCircles } from "./circles.js";
/** @typedef {import("../../types/stream").DrawLine} DrawLine */
/** @typedef {{ stroke: number; points: [number, number, number][]; by?: number; keep?: boolean }} Piece */

// A stroke shows this long after its last point, then fades out over FADE_MS.
const HOLD_MS = 3000,
  FADE_MS = 1000;
// A colour per admin, by their number in the room: the owner's (1) first.
export const INKS = ["#ffd23f", "#4fd1c5", "#f472b6", "#a3e635", "#fb923c", "#c084fc"];
// The colours admins picked, by their number.
/** @type {Map<number, string>} */
const picked = new Map();
/** @param {{ id: number; ink?: string }[]} people */
export function setInks(people) {
  for (const p of people) if (p.ink) picked.set(p.id, p.ink);
}
/** @param {number} by */
export const inkOf = by => picked.get(by) ?? INKS[(Math.max(1, by) - 1) % INKS.length];

// r and b take a team's ink, y one's own back.
/** @type {Record<string, string>} */
const TEAM_INKS = { r: TEAM.red, b: TEAM.blue };
export const INK_KEYS = ["r", "b", "y"];
// The ink a key takes, if any, and one's own to remember.
/** @param {string} key @param {string} now @param {string | null} own */
export function inkByKey(key, now, own) {
  if (!Object.values(TEAM_INKS).includes(now)) own = now;
  const ink = key === "y" ? own : TEAM_INKS[key];
  return { ink: ink && ink !== now ? ink : null, own };
}

/** @param {string} hex */
function luminance(hex) {
  const [r, g, b] = [1, 3, 5].map(i => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
// What reads best on an ink: the dark, or white.
/** @param {string} ink */
export function onInk(ink) {
  const l = luminance(ink) + 0.05;
  return l / (luminance(INK) + 0.05) >= 1.05 / l ? INK : "#ffffff";
}

// Others' strokes show this far behind, so their points come in time to be drawn at the pace
// they were drawn instead of in jumps as each piece arrives.
const BEHIND_MS = 40;

// A stroke's points ([x, y, ms into the stroke]), and when its start shows here.
/** @type {Map<number, { points: [number, number, number][]; start: number; ink: string; keep: boolean }>} */
const strokes = new Map();
// Strokes drawn on this page: shown as drawn, and their echo from the room skipped.
/** @type {Set<number>} */
const mine = new Set();

/** @param {DrawLine} m */
export function onDraw(m) {
  if (!mine.has(m.stroke)) addPoints(m, BEHIND_MS);
}

/** @param {Piece} m */
export function drawOwn(m) {
  mine.add(m.stroke);
  addPoints(m, 0);
}

/** @param {Piece} m @param {number} behind */
function addPoints(m, behind) {
  const first = m.points[0]?.[2] ?? 0;
  const s = strokes.get(m.stroke) ?? {
    points: [],
    start: performance.now() + behind - first,
    ink: inkOf(m.by ?? 1),
    keep: m.keep === true,
  };
  s.points.push(...m.points);
  strokes.set(m.stroke, s);
}

// Marker points the room's map holds.
let keptMax = Infinity;

// The marker as the room has it, on join or reconnect: in place of the page's, but for strokes
// drawn here.
/** @param {import("../../types/stream").MarkerLine} m */
export function onMarker(m) {
  keptMax = m.max ?? Infinity;
  arcsFromMarker(m.strokes);
  circlesFromMarker(m.strokes);
  const kept = new Set(m.strokes.map(k => k.stroke));
  for (const [id, s] of strokes) if (s.keep && !kept.has(id) && !mine.has(id)) strokes.delete(id);
  for (const k of m.strokes)
    if (!mine.has(k.stroke) && !k.arc && !k.circle)
      strokes.set(k.stroke, { points: k.points, start: -Infinity, ink: k.ink ?? inkOf(k.by), keep: true });
}

export const markerFull = () => keptPoints() >= keptMax;

function keptPoints() {
  let n = 0;
  for (const s of strokes.values()) if (s.keep) n += s.points.length;
  return n;
}

// The room took only the first `n` points of this page's stroke.
/** @param {number} stroke @param {number} n */
export function trimOwn(stroke, n) {
  const s = strokes.get(stroke);
  if (!s) return;
  if (n > 0) s.points.length = Math.min(n, s.points.length);
  else strokes.delete(stroke);
}

// An admin wiped the map.
export function onClear() {
  clearArcs();
  clearCircles();
  strokes.clear();
  mine.clear();
}

// A stroke's width at the 1080p size, and what its dark edge adds to it.
export const STROKE_PX = 4,
  EDGE_PX = 3;
export function drawStrokes(now = performance.now()) {
  drawArcs();
  drawCircles();
  if (!strokes.size) return;
  const w = STROKE_PX * uiScale();
  ctx.save();
  ctx.lineCap = ctx.lineJoin = "round";
  for (const [id, s] of strokes) {
    // Drawn up to now; it fades from when its last point showed.
    const shown = s.points.filter(p => s.start + p[2] <= now);
    if (!shown.length) continue;
    const age = now - (s.start + shown[shown.length - 1][2]);
    if (!s.keep && age > HOLD_MS + FADE_MS && shown.length === s.points.length) {
      strokes.delete(id);
      mine.delete(id);
      continue;
    }
    ctx.globalAlpha = s.keep ? 1 : Math.max(0, Math.min(1, 1 - (age - HOLD_MS) / FADE_MS));
    ctx.beginPath();
    for (const [i, [x, y]] of shown.entries()) {
      const [sx, sy] = toScreen(x, y);
      if (i) ctx.lineTo(sx, sy);
      else ctx.moveTo(sx, sy);
    }
    // A lone point is a dot.
    if (shown.length === 1) ctx.lineTo(...toScreen(shown[0][0], shown[0][1]));
    ctx.strokeStyle = INK;
    ctx.lineWidth = w + EDGE_PX;
    ctx.stroke();
    ctx.strokeStyle = s.ink;
    ctx.lineWidth = w;
    ctx.stroke();
  }
  ctx.restore();
}

/** @param {number} id */
export const pointsOf = id => strokes.get(id)?.points.length ?? 0;
