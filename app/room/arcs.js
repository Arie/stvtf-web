// A room's jump arcs: two clicks, a jump from the low spot onto the high one, kept until cleared.
// Seen from above an arc is straight; its height shows as shading, dark at the launch to bright
// at the peak, in the drawer's ink, as the jump trails do (map/overkill.js).
import { ctx, uiScale } from "../core/core.js";
import { toScreen } from "../map/view.js";
/** @typedef {import("../../types/stream").ArcLine} ArcLine */

// Lightness at the launch and at the peak, as the jump trails'.
const DARK = 24,
  BRIGHT = 74;
// A line's width at the 1080p size, what its dark edge adds, and the arrowhead's length and angle.
const ARC_PX = 4,
  EDGE_PX = 3,
  HEAD_PX = 14,
  HEAD_ANGLE = Math.PI / 6;
const EDGE = "#0b0c0e";

/** @type {Map<number, { ink: string; points: [number, number, number][] }>} */
const arcs = new Map();

/** @param {ArcLine} m */
export function onArc(m) {
  arcs.set(m.stroke, { ink: m.ink, points: m.points });
}

// The marker's arcs, in place of those the page had.
/** @param {{ stroke: number; ink?: string; arc?: [number, number, number][] }[]} strokes */
export function arcsFromMarker(strokes) {
  arcs.clear();
  for (const k of strokes) if (k.arc && k.ink) arcs.set(k.stroke, { ink: k.ink, points: k.arc });
}

export const clearArcs = () => arcs.clear();

// The first click of an arc being drawn on this page, shown as a dot until the second.
/** @type {[number, number] | null} */
let pending = null;
/** @param {[number, number] | null} at */
export const setPending = at => (pending = at);
export const arcCount = () => arcs.size;

// `ink` (#rrggbb) at `height` (0 the launch, 1 the peak): its hue and saturation, lightness DARK to BRIGHT.
/** @param {string} ink @param {number} height */
export function arcShade(ink, height) {
  const [r, g, b] = [1, 3, 5].map(i => parseInt(ink.slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b),
    min = Math.min(r, g, b),
    c = max - min,
    l = (max + min) / 2;
  const s = c ? c / (1 - Math.abs(2 * l - 1)) : 0;
  const light = DARK + (BRIGHT - DARK) * Math.max(0, Math.min(1, height));
  return `hsl(${Math.round(sixth(r, g, b, max, c) * 60)} ${Math.round(s * 100)}% ${Math.round(light)}%)`;
}

// The hue in sixths of the wheel, from whichever channel is strongest.
/** @param {number} r @param {number} g @param {number} b @param {number} max @param {number} c */
function sixth(r, g, b, max, c) {
  if (!c) return 0;
  if (max === r) return ((g - b) / c + 6) % 6;
  if (max === g) return (b - r) / c + 2;
  return (r - g) / c + 4;
}

export function drawArcs() {
  if (!arcs.size && !pending) return;
  const w = ARC_PX * uiScale();
  ctx.save();
  ctx.lineCap = ctx.lineJoin = "round";
  if (pending) {
    const [x, y] = toScreen(...pending);
    ctx.beginPath();
    ctx.arc(x, y, w * 1.5, 0, 2 * Math.PI);
    ctx.fillStyle = EDGE;
    ctx.fill();
  }
  for (const { ink, points } of arcs.values()) drawArc(ink, points, w);
  ctx.restore();
}

/** @param {string} ink @param {[number, number, number][]} points @param {number} w */
function drawArc(ink, points, w) {
  const z0 = points[0][2],
    peak = Math.max(...points.map(p => p[2]));
  const at = points.map(p => toScreen(p[0], p[1]));
  ctx.beginPath();
  at.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
  ctx.strokeStyle = EDGE;
  ctx.lineWidth = w + EDGE_PX;
  ctx.stroke();
  ctx.lineWidth = w;
  for (let i = 1; i < at.length; i++) {
    ctx.beginPath();
    ctx.moveTo(...at[i - 1]);
    ctx.lineTo(...at[i]);
    ctx.strokeStyle = arcShade(ink, peak > z0 ? (points[i][2] - z0) / (peak - z0) : 0);
    ctx.stroke();
  }
  // The arrowhead, pointing from the launch to the landing.
  const [x0, y0] = at[0],
    [x1, y1] = at[at.length - 1],
    dir = Math.atan2(y1 - y0, x1 - x0),
    len = HEAD_PX * uiScale();
  ctx.beginPath();
  for (const side of [-1, 1]) {
    ctx.moveTo(x1, y1);
    ctx.lineTo(x1 - len * Math.cos(dir + side * HEAD_ANGLE), y1 - len * Math.sin(dir + side * HEAD_ANGLE));
  }
  ctx.strokeStyle = arcShade(ink, 1);
  ctx.stroke();
}
