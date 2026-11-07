// A room's telestrator: strokes its owner draws, in map coordinates, shown to everyone in the
// room for a few seconds and then faded out.
import { ctx, uiScale } from "./core.js";
import { toScreen } from "./view.js";
/** @typedef {import("../types/stream").DrawLine} DrawLine */

// A stroke shows this long after its last point, then fades out over FADE_MS.
const HOLD_MS = 3000,
  FADE_MS = 1000;
const INK = "#ffd23f",
  EDGE = "#0b0c0e";

/** @type {Map<number, { points: [number, number][]; last: number }>} */
const strokes = new Map();
// Strokes drawn on this page: shown as drawn, and their echo from the room skipped.
/** @type {Set<number>} */
const mine = new Set();

/** @param {DrawLine} m */
export function onDraw(m) {
  if (!mine.has(m.stroke)) addPoints(m);
}

/** @param {{ stroke: number; points: [number, number][] }} m */
export function drawOwn(m) {
  mine.add(m.stroke);
  addPoints(m);
}

/** @param {{ stroke: number; points: [number, number][] }} m */
function addPoints(m) {
  const s = strokes.get(m.stroke) ?? { points: [], last: 0 };
  s.points.push(...m.points);
  s.last = performance.now();
  strokes.set(m.stroke, s);
}

export function drawStrokes(now = performance.now()) {
  if (!strokes.size) return;
  const w = 4 * uiScale();
  ctx.save();
  ctx.lineCap = ctx.lineJoin = "round";
  for (const [id, s] of strokes) {
    const age = now - s.last;
    if (age > HOLD_MS + FADE_MS) {
      strokes.delete(id);
      mine.delete(id);
      continue;
    }
    ctx.globalAlpha = Math.min(1, 1 - (age - HOLD_MS) / FADE_MS);
    ctx.beginPath();
    for (const [i, [x, y]] of s.points.entries()) {
      const [sx, sy] = toScreen(x, y);
      if (i) ctx.lineTo(sx, sy);
      else ctx.moveTo(sx, sy);
    }
    // A lone point is a dot.
    if (s.points.length === 1) ctx.lineTo(...toScreen(...s.points[0]));
    ctx.strokeStyle = EDGE;
    ctx.lineWidth = w + 3;
    ctx.stroke();
    ctx.strokeStyle = INK;
    ctx.lineWidth = w;
    ctx.stroke();
  }
  ctx.restore();
}

/** @param {number} id */
export const pointsOf = id => strokes.get(id)?.points.length ?? 0;
