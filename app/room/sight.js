// What blocks sight on a map (`/sight/<map>`, src/maps/bsp/reach.rs): a bit per voxel that
// something solid fills. Tells what a camera at a spot would show of the ground.
import { state } from "../core/core.js";
import { asset } from "../core/env.js";
/** @typedef {[number, number, number]} Vec3 */
/** @typedef {{ pos: Vec3; aim: Vec3 }} Spot */
/** @typedef {{ bits: Uint8Array; dims: Vec3; origin: Vec3; vox: number; floors: Map<number, number[]> }} Grid */

// TF2 at 16:9: half of what it shows left to right and top to bottom, as tangents.
const TAN_H = Math.tan((53.13 * Math.PI) / 180),
  TAN_V = Math.tan((36.87 * Math.PI) / 180);
// How far round the aim the ground is looked at, and a player's chest above a floor, in map units.
const RANGE = 3200,
  CHEST = 48;
// Ground that shows, and ground in the picture but hidden: red, green, blue, alpha.
const SHOWN = [124, 255, 138, 110],
  HIDDEN = [255, 107, 90, 70];

/** @param {string} map @returns {Promise<Grid | null>} */
export async function loadSight(map) {
  const r = await fetch(asset(`sight/${encodeURIComponent(map)}`)).catch(() => null);
  if (!r?.ok) return null;
  const body = new Uint8Array(await r.arrayBuffer()),
    head = body.indexOf(10);
  const { dims, origin, vox } = JSON.parse(new TextDecoder().decode(body.subarray(0, head)));
  return { bits: body.subarray(head + 1), dims, origin, vox, floors: new Map() };
}
/** @param {Grid} g @param {number} i @param {number} j @param {number} k */
function solid(g, i, j, k) {
  const [nx, ny, nz] = g.dims;
  if (i < 0 || j < 0 || k < 0 || i >= nx || j >= ny || k >= nz) return false;
  const n = (i * ny + j) * nz + k;
  return ((g.bits[n >> 3] >> (n & 7)) & 1) === 1;
}
/** @param {Grid} g @param {number} v @param {number} axis */
const cell = (g, v, axis) => Math.floor((v - g.origin[axis]) / g.vox);
/** @param {Grid} g @param {number[]} p */
const solidAt = (g, p) => solid(g, cell(g, p[0], 0), cell(g, p[1], 1), cell(g, p[2], 2));
// The floors of a column: where space lies on something solid.
/** @param {Grid} g @param {number} i @param {number} j */
function floorsOf(g, i, j) {
  const key = i * g.dims[1] + j;
  let f = g.floors.get(key);
  if (!f) {
    f = [];
    for (let k = 1; k < g.dims[2]; k++)
      if (!solid(g, i, j, k) && solid(g, i, j, k - 1)) f.push(g.origin[2] + k * g.vox);
    g.floors.set(key, f);
  }
  return f;
}
// Whether nothing solid lies between `a` and `b`, in steps of a third of a voxel.
/** @param {Grid} g @param {number[]} a @param {number[]} b */
function clear(g, a, b) {
  const d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
    steps = Math.ceil(Math.hypot(d[0], d[1], d[2]) / (g.vox / 3));
  for (let s = 1; s < steps; s++) {
    const t = s / steps;
    if (solidAt(g, [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t])) return false;
  }
  return true;
}
// A camera that shows nothing of its spot: in something solid, out over the void, or with
// something between it and what it looks at.
/** @param {Grid} g @param {Spot} c */
export function blind(g, c) {
  const overGround = floorsOf(g, cell(g, c.pos[0], 0), cell(g, c.pos[1], 1)).some(z => z <= c.pos[2]);
  return solidAt(g, c.pos) || !overGround || !clear(g, c.pos, [c.aim[0], c.aim[1], c.aim[2] + CHEST]);
}
// The highest floor under `z` at a spot, for an aim put there.
/** @param {Grid} g @param {number} x @param {number} y @param {number} z */
export function floorUnder(g, x, y, z) {
  const under = floorsOf(g, cell(g, x, 0), cell(g, y, 1)).filter(f => f < z);
  return under.length ? Math.round(under[under.length - 1]) : null;
}

const sub = (/** @type {number[]} */ a, /** @type {number[]} */ b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]],
  dot = (/** @type {number[]} */ a, /** @type {number[]} */ b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2],
  cross = (/** @type {number[]} */ a, /** @type {number[]} */ b) => [
    a[1] * b[2] - a[2] * b[1],
    a[2] * b[0] - a[0] * b[2],
    a[0] * b[1] - a[1] * b[0],
  ],
  unit = (/** @type {number[]} */ a) => a.map(v => v / (Math.hypot(a[0], a[1], a[2]) || 1));

// Whether a column's ground is in the camera's picture (null if not), and shows.
/** @param {Grid} g @param {Spot} c @param {number[][]} axes @param {number} i @param {number} j */
function columnShows(g, c, [f, r, u], i, j) {
  const x = g.origin[0] + (i + 0.5) * g.vox,
    y = g.origin[1] + (j + 0.5) * g.vox;
  let framed = null;
  for (const z of floorsOf(g, i, j)) {
    const p = [x, y, z + CHEST],
      d = sub(p, c.pos),
      depth = dot(d, f);
    if (depth <= 16 || Math.abs(dot(d, r)) > depth * TAN_H || Math.abs(dot(d, u)) > depth * TAN_V) continue;
    if (clear(g, c.pos, p)) return true;
    framed = false;
  }
  return framed;
}
// How much of each capture area in the picture shows, as said under the camera's name.
/** @param {Grid} g @param {Set<number>} shown */
function pointsShown(g, shown) {
  const said = (state.geometry?.cap_areas ?? []).flatMap(area => {
    let all = 0,
      seen = 0;
    for (let i = cell(g, area.min[0], 0); i <= cell(g, area.max[0], 0); i++)
      for (let j = cell(g, area.min[1], 1); j <= cell(g, area.max[1], 1); j++) {
        all++;
        if (shown.has(i * g.dims[1] + j)) seen++;
      }
    return seen ? [`Point ${area.point + 1}: ${Math.round((100 * seen) / all)}%`] : [];
  });
  return said.join(" · ") || "No point";
}
// What a camera would show of the ground round its aim: an image of it, a pixel per column and
// rows running with the map's y, where it lies on the map, and what to say of it.
/** @param {Grid} g @param {Spot} c */
export function lookFrom(g, c) {
  const n = Math.ceil(RANGE / g.vox),
    [ci, cj] = [cell(g, c.aim[0], 0), cell(g, c.aim[1], 1)],
    f = unit(sub(c.aim, c.pos)),
    // Straight down any right will do.
    r = Math.abs(f[2]) > 0.999 ? [0, -1, 0] : unit(cross(f, [0, 0, 1])),
    lost = solidAt(g, c.pos);
  const image = document.createElement("canvas");
  image.width = image.height = 2 * n;
  const pen = /** @type {CanvasRenderingContext2D} */ (image.getContext("2d")),
    data = pen.createImageData(2 * n, 2 * n),
    shown = new Set();
  for (let a = 0; a < 2 * n; a++)
    for (let b = 0; b < 2 * n; b++) {
      const [i, j] = [ci - n + a, cj - n + b],
        shows = columnShows(g, c, [f, r, cross(r, f)], i, j);
      if (shows === null) continue;
      if (shows && !lost) shown.add(i * g.dims[1] + j);
      data.data.set(shows && !lost ? SHOWN : HIDDEN, (b * 2 * n + a) * 4);
    }
  pen.putImageData(data, 0, 0);
  return {
    image,
    x: g.origin[0] + (ci - n) * g.vox,
    y: g.origin[1] + (cj - n) * g.vox,
    size: 2 * n * g.vox,
    says: lost ? "In a wall" : pointsShown(g, shown),
  };
}
