// A room's telestrator: strokes its owner draws, in map coordinates, shown to everyone in the
// room for a few seconds and then faded out.
import { ctx, uiScale } from "./core.js";
import { toScreen } from "./view.js";
/** @typedef {import("../types/stream").DrawLine} DrawLine */

// A stroke shows this long after its last point, then fades out over FADE_MS.
const HOLD_MS = 3000,
  FADE_MS = 1000;
// A colour per admin, by their number in the room: the owner's (1) first.
export const INKS = ["#ffd23f", "#4fd1c5", "#f472b6", "#a3e635", "#fb923c", "#c084fc"];
const EDGE = "#0b0c0e";
// The colours admins picked, by their number.
/** @type {Map<number, string>} */
const picked = new Map();
/** @param {{ id: number; ink?: string }[]} people */
export function setInks(people) {
  for (const p of people) if (p.ink) picked.set(p.id, p.ink);
}
/** @param {number} by */
export const inkOf = by => picked.get(by) ?? INKS[(Math.max(1, by) - 1) % INKS.length];

// Others' strokes show this far behind, so their points come in time to be drawn at the pace
// they were drawn instead of in jumps as each piece arrives.
const BEHIND_MS = 40;

// A stroke's points ([x, y, ms into the stroke]), and when its start shows here.
/** @type {Map<number, { points: [number, number, number][]; start: number; ink: string }>} */
const strokes = new Map();
// Strokes drawn on this page: shown as drawn, and their echo from the room skipped.
/** @type {Set<number>} */
const mine = new Set();

/** @param {DrawLine} m */
export function onDraw(m) {
  if (!mine.has(m.stroke)) addPoints(m, BEHIND_MS);
}

/** @param {{ stroke: number; points: [number, number, number][]; by?: number }} m */
export function drawOwn(m) {
  mine.add(m.stroke);
  addPoints(m, 0);
}

/** @param {{ stroke: number; points: [number, number, number][]; by?: number }} m @param {number} behind */
function addPoints(m, behind) {
  const first = m.points[0]?.[2] ?? 0;
  const s = strokes.get(m.stroke) ?? { points: [], start: performance.now() + behind - first, ink: inkOf(m.by ?? 1) };
  s.points.push(...m.points);
  strokes.set(m.stroke, s);
}

export function drawStrokes(now = performance.now()) {
  if (!strokes.size) return;
  const w = 4 * uiScale();
  ctx.save();
  ctx.lineCap = ctx.lineJoin = "round";
  for (const [id, s] of strokes) {
    // Drawn up to now; it fades from when its last point showed.
    const shown = s.points.filter(p => s.start + p[2] <= now);
    if (!shown.length) continue;
    const age = now - (s.start + shown[shown.length - 1][2]);
    if (age > HOLD_MS + FADE_MS && shown.length === s.points.length) {
      strokes.delete(id);
      mine.delete(id);
      continue;
    }
    ctx.globalAlpha = Math.max(0, Math.min(1, 1 - (age - HOLD_MS) / FADE_MS));
    ctx.beginPath();
    for (const [i, [x, y]] of shown.entries()) {
      const [sx, sy] = toScreen(x, y);
      if (i) ctx.lineTo(sx, sy);
      else ctx.moveTo(sx, sy);
    }
    // A lone point is a dot.
    if (shown.length === 1) ctx.lineTo(...toScreen(shown[0][0], shown[0][1]));
    ctx.strokeStyle = EDGE;
    ctx.lineWidth = w + 3;
    ctx.stroke();
    ctx.strokeStyle = s.ink;
    ctx.lineWidth = w;
    ctx.stroke();
  }
  ctx.restore();
}

/** @param {number} id */
export const pointsOf = id => strokes.get(id)?.points.length ?? 0;
