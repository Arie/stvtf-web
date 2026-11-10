// A room's circles: one click each, a ring on the floor there in the drawer's ink, kept until
// cleared, at the size TF2 shows it (src/rooms/room/circle.rs).
import { INK, ctx, uiScale } from "../core/core.js";
import { toScreen } from "../map/view.js";
import { EDGE_PX, STROKE_PX } from "./telestrator.js";
/** @typedef {import("../../types/stream").CircleLine} CircleLine */

// Its radius, in map units.
const RADIUS = 38;

/** @type {Map<number, { at: [number, number, number]; ink: string }>} */
const circles = new Map();

/** @param {CircleLine} m */
export const onCircle = m => circles.set(m.stroke, { at: m.at, ink: m.ink });

// The marker's circles, in place of those the page had.
/** @param {{ stroke: number; ink?: string; circle?: [number, number, number] }[]} strokes */
export function circlesFromMarker(strokes) {
  circles.clear();
  for (const k of strokes) if (k.circle && k.ink) circles.set(k.stroke, { at: k.circle, ink: k.ink });
}

export const clearCircles = () => circles.clear();
export const circleCount = () => circles.size;

export function drawCircles() {
  if (!circles.size) return;
  const w = STROKE_PX * uiScale();
  ctx.save();
  for (const { at, ink } of circles.values()) {
    const [x, y] = toScreen(at[0], at[1]),
      [ex, ey] = toScreen(at[0] + RADIUS, at[1]);
    ctx.beginPath();
    ctx.arc(x, y, Math.hypot(ex - x, ey - y), 0, 2 * Math.PI);
    ctx.strokeStyle = INK;
    ctx.lineWidth = w + EDGE_PX;
    ctx.stroke();
    ctx.strokeStyle = ink;
    ctx.lineWidth = w;
    ctx.stroke();
  }
  ctx.restore();
}
