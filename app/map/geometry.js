import { PLAIN_BG, context2d, INK } from "../core/core.js";
/** @typedef {import("../../types/page").Geometry} Geometry */
/** @typedef {NonNullable<Geometry["floors"]>[number]} Floor */
/** @typedef {NonNullable<Geometry["walls"]>[number]} Wall */
/** @typedef {NonNullable<Geometry["playable"]>} Playable */
// What the drawing steps share: the context, its size, world to canvas x and y, units per pixel
// and the height above which floors are left out.
/** @typedef {{ c: CanvasRenderingContext2D, w: number, h: number, X: (x: number) => number, Y: (y: number) => number, res: number, cut: number }} Drawing */

// Map geometry: at least this many world units per pixel, and a canvas at most this wide;
// src/maps/bsp.rs pads the bounds' top this far above the highest point or pickup (its PAD), floors are
// kept up to this far above that, and a wall counts from this far below its top.
const MIN_UNITS_PER_PIXEL = 4,
  MAX_CANVAS = 4096,
  GEOMETRY_PAD = 1536,
  ROOF_ABOVE = 400,
  WALL_BASE = 64;
// Floor colours by surface kind (src/maps/bsp.rs's material words); anything else, or none, is "other".
const KIND_COLOURS = {
  __proto__: null,
  grass: "#56703d",
  dirt: "#86704f",
  concrete: "#7b7870",
  metal: "#5e6872",
  wood: "#7f5a39",
  tile: "#76665a",
  water: "#2c5a63",
  lava: "#93401f",
  snow: "#c7d1d8",
  other: "#6c6c6c",
};
/** @typedef {Exclude<keyof typeof KIND_COLOURS, "__proto__">} Kind */
// Water shows the floor under it; raised floors cast a short soft shadow (canvas pixels).
const WATER_ALPHA = 0.45,
  SHADOW_COLOUR = "rgba(0,0,0,0.45)",
  SHADOW_BLUR = 6,
  SHADOW_OFFSET = 3;
// bg=flat: floors in these shades, lowest to highest. bg=outline: a line round the playable area in
// this colour with a dark edge, as a share of the canvas's longest side.
const FLAT_SHADES = ["#2a2e33", "#373c42", "#464c53"],
  OUTLINE_COLOUR = "#e4e8ec",
  OUTLINE_EDGE = "#14161a",
  OUTLINE_WIDTH = 1 / 400,
  OUTLINE_EDGE_WIDTH = 1 / 200;
// Gaps up to about twice this share of the canvas's longest side are filled in both.
const GAP_CLOSE = 1 / 150,
  PLAIN_SIDE = 1024;
// Payload track: ties this far apart and this long, rails this far either side (canvas pixels).
const TIE_STEP = 7,
  TIE_LENGTH = 10,
  RAIL_GAP = 3,
  TIE_COLOUR = "#3a2c20",
  RAIL_COLOUR = "#cdc8bc";

// A small tile of subtle marks per kind, made once with seeded noise so a map looks the same
// on every load; kinds without one are plain.
/** @type {{ [kind: string]: HTMLCanvasElement | null }} */
const kindTiles = {};
/** @param {string} k */
function kindTile(k) {
  if (k in kindTiles) return kindTiles[k];
  const tile = Object.hasOwn(TILES, k) ? TILES[k] : null;
  if (!tile) return (kindTiles[k] = null);
  let seed = 7;
  const rnd = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  const t = document.createElement("canvas");
  [t.width, t.height] = tile.size;
  tile.paint(context2d(t), rnd);
  return (kindTiles[k] = t);
}
/** @param {number} a */
const light = a => `rgba(255,255,255,${a})`;
/** @param {number} a */
const dark = a => `rgba(0,0,0,${a})`;
// Each kind's tile: its size and how its marks are painted, given seeded noise.
/** @type {{ [kind: string]: { size: [number, number]; paint: (c: CanvasRenderingContext2D, rnd: () => number) => void } }} */
const TILES = {
  grass: {
    size: [16, 16],
    paint(c, rnd) {
      for (let i = 0; i < 40; i++) {
        c.fillStyle = rnd() < 0.5 ? light(0.1) : dark(0.12);
        c.fillRect(Math.floor(rnd() * 16), Math.floor(rnd() * 16), 1, 1);
      }
    },
  },
  dirt: {
    size: [48, 48],
    paint(c, rnd) {
      for (let i = 0; i < 14; i++) {
        const x = rnd() * 48,
          y = rnd() * 48,
          r = 4 + rnd() * 8,
          tint = rnd() < 0.5 ? "255,255,255" : "0,0,0";
        // Drawn at every wrap so blotches cross the tile's edges without a seam.
        for (const [dx, dy] of [
          [0, 0],
          [-48, 0],
          [0, -48],
          [-48, -48],
          [48, 0],
          [0, 48],
          [48, 48],
          [-48, 48],
          [48, -48],
        ]) {
          const gr = c.createRadialGradient(x + dx, y + dy, 0, x + dx, y + dy, r);
          gr.addColorStop(0, `rgba(${tint},0.09)`);
          gr.addColorStop(1, `rgba(${tint},0)`);
          c.fillStyle = gr;
          c.fillRect(x + dx - r, y + dy - r, 2 * r, 2 * r);
        }
      }
    },
  },
  metal: {
    size: [8, 8],
    paint(c) {
      c.strokeStyle = light(0.09);
      c.lineWidth = 1;
      c.beginPath();
      c.moveTo(-1, 9);
      c.lineTo(9, -1);
      c.moveTo(-1, 1);
      c.lineTo(1, -1);
      c.moveTo(7, 9);
      c.lineTo(9, 7);
      c.stroke();
    },
  },
  wood: {
    size: [32, 10],
    paint(c, rnd) {
      c.fillStyle = dark(0.18);
      c.fillRect(0, 9, 32, 1);
      c.fillRect(Math.floor(rnd() * 32), 0, 1, 9);
      c.fillStyle = light(0.05);
      c.fillRect(0, 0, 32, 1);
    },
  },
  tile: {
    size: [12, 12],
    paint(c) {
      c.fillStyle = dark(0.14);
      c.fillRect(0, 11, 12, 1);
      c.fillRect(11, 0, 1, 11);
    },
  },
  water: {
    size: [24, 12],
    paint(c) {
      c.strokeStyle = light(0.12);
      c.lineWidth = 1;
      c.beginPath();
      for (let x = 0; x <= 24; x++) (x ? c.lineTo : c.moveTo).call(c, x, 6 + 2 * Math.sin((x / 24) * 2 * Math.PI));
      c.stroke();
    },
  },
};

// A hex colour scaled by s, as rgb().
/** @param {string} hex @param {number} s */
function scaled(hex, s) {
  const n = parseInt(hex.slice(1), 16),
    ch = /** @param {number} sh */ sh => Math.min(255, Math.round(((n >> sh) & 255) * s));
  return `rgb(${ch(16)} ${ch(8)} ${ch(0)})`;
}

// A canvas the size of `shape`, holding what draw(context) puts on it.
/** @param {{ width: number, height: number }} shape @param {(c: CanvasRenderingContext2D) => void} draw */
function canvasLike(shape, draw) {
  const o = document.createElement("canvas");
  o.width = shape.width;
  o.height = shape.height;
  draw(context2d(o));
  return o;
}
// Draws a shape at each of 16 points round a circle: with source-over that spreads it outwards.
/** @param {CanvasRenderingContext2D} c @param {HTMLCanvasElement} shape @param {number} r */
function around(c, shape, r) {
  for (let i = 0; i < 16; i++) c.drawImage(shape, Math.cos((i / 8) * Math.PI) * r, Math.sin((i / 8) * Math.PI) * r);
}
// The shape with its cracks and narrow gaps filled: spread outwards, then eaten back as far. In
// passes, because one ring of copies of a thin sliver is a comb rather than a solid disc.
/** @param {HTMLCanvasElement} shape */
function withoutGaps(shape) {
  const r = Math.max(2, Math.max(shape.width, shape.height) * GAP_CLOSE);
  /** @param {HTMLCanvasElement} start @param {GlobalCompositeOperation} mode */
  const passes = (start, mode) =>
    [0.5, 0.25, 0.25].reduce(
      (from, k) =>
        canvasLike(shape, c => {
          c.drawImage(from, 0, 0);
          c.globalCompositeOperation = mode;
          around(c, from, r * k);
        }),
      start,
    );
  return passes(passes(shape, "source-over"), "destination-in");
}
/** @param {HTMLCanvasElement} shape @param {string} colour */
const tinted = (shape, colour) =>
  canvasLike(shape, c => {
    c.drawImage(shape, 0, 0);
    c.globalCompositeOperation = "source-in";
    c.fillStyle = colour;
    c.fillRect(0, 0, shape.width, shape.height);
  });
// A line round a shape: the shape spread outwards in the line's colours, the shape cut out.
/** @param {HTMLCanvasElement} shape */
function outlineOf(shape) {
  const side = Math.max(shape.width, shape.height);
  /** @param {string} colour @param {number} width */
  const ring = (colour, width) => canvasLike(shape, c => around(c, tinted(shape, colour), Math.max(1, side * width)));
  return canvasLike(shape, c => {
    c.drawImage(ring(OUTLINE_EDGE, OUTLINE_EDGE_WIDTH), 0, 0);
    c.drawImage(ring(OUTLINE_COLOUR, OUTLINE_WIDTH), 0, 0);
    c.globalCompositeOperation = "destination-out";
    c.drawImage(shape, 0, 0);
  });
}

// The floors' height range, leaving out the lowest and highest few.
/** @param {Floor[]} floors */
function heightRange(floors) {
  const zs = floors.map(f => f.z);
  const zlo = zs.length ? zs[Math.floor(zs.length * 0.02)] : 0,
    zhi = zs.length ? zs[Math.floor(zs.length * 0.98)] : 1;
  return [zlo, zhi];
}

// A floor's colour at height share t: a flat shade with bg=flat, else its kind's colour, darker low down.
/** @param {Kind} k @param {number} t */
function floorColour(k, t) {
  if (PLAIN_BG) return FLAT_SHADES[Math.min(FLAT_SHADES.length - 1, Math.floor(t * FLAT_SHADES.length))];
  return scaled(KIND_COLOURS[k], 0.7 + 0.4 * t);
}

/** @param {Drawing} at @param {Floor[]} floors */
function drawFloors(at, floors) {
  const { c, cut } = at;
  const [zlo, zhi] = heightRange(floors);
  /** @type {{ [kind: string]: CanvasPattern | null }} */
  const patterns = {};
  c.lineWidth = 1;
  if (!PLAIN_BG) {
    c.shadowBlur = SHADOW_BLUR;
    c.shadowOffsetX = c.shadowOffsetY = SHADOW_OFFSET;
  }
  for (const f of floors) {
    if (f.z > cut) continue;
    const t = Math.max(0, Math.min(1, (f.z - zlo) / (zhi - zlo || 1)));
    drawFloor(at, f, t, patterns);
  }
  c.shadowColor = "transparent";
  c.shadowBlur = c.shadowOffsetX = c.shadowOffsetY = 0;
  c.globalAlpha = 1;
}

// One floor at height share `t`: its colour, shadow and its kind's pattern over it.
/** @param {Drawing} at @param {Floor} f @param {number} t @param {{ [kind: string]: CanvasPattern | null }} patterns */
function drawFloor({ c, X, Y }, f, t, patterns) {
  const k = /** @type {Kind} */ (f.k && f.k in KIND_COLOURS ? f.k : "other"),
    water = k === "water";
  c.fillStyle = c.strokeStyle = floorColour(k, t);
  c.beginPath();
  for (let i = 0; i < f.p.length; i += 2) (i ? c.lineTo : c.moveTo).call(c, X(f.p[i]), Y(f.p[i + 1]));
  c.closePath();
  // Terrain is one smooth surface, so its cells do not shadow each other.
  c.shadowColor = PLAIN_BG || f.d || water ? "transparent" : SHADOW_COLOUR;
  c.globalAlpha = water && !PLAIN_BG ? WATER_ALPHA : 1;
  c.fill();
  c.shadowColor = "transparent";
  // The floor's own colour round its edge, so neighbouring polygons leave no hairline seams.
  c.stroke();
  if (PLAIN_BG) return;
  if (!(k in patterns))
    patterns[k] = kindTile(k) && c.createPattern(/** @type {HTMLCanvasElement} */ (kindTile(k)), "repeat");
  if (patterns[k]) {
    c.fillStyle = patterns[k];
    c.fill();
  }
}

// Walls only where a floor was drawn: the rest are scenery's, out over the background.
/** @param {Drawing} at @param {Wall[]} walls */
function drawWalls({ c, X, Y, cut }, walls) {
  c.strokeStyle = INK;
  c.lineWidth = 1.2;
  c.globalCompositeOperation = "source-atop";
  c.beginPath();
  if (!PLAIN_BG)
    for (const wl of walls) {
      if (wl.z - WALL_BASE > cut) continue;
      c.moveTo(X(wl.p[0]), Y(wl.p[1]));
      c.lineTo(X(wl.p[2]), Y(wl.p[3]));
    }
  c.stroke();
  c.globalCompositeOperation = "source-over";
}

// The playable area's bits as a canvas, white where playable. Rows run from the lowest y up, so
// they are flipped into the canvas.
/** @param {Playable} pa */
function playableMask(pa) {
  const bytes = Uint8Array.from(atob(pa.bits), ch => ch.charCodeAt(0));
  const mc = document.createElement("canvas");
  mc.width = pa.w;
  mc.height = pa.h;
  const mctx = context2d(mc),
    img = mctx.createImageData(pa.w, pa.h);
  for (let cy = 0; cy < pa.h; cy++)
    for (let cx = 0; cx < pa.w; cx++) {
      const i = cy * pa.w + cx;
      if ((bytes[i >> 3] >> (i & 7)) & 1)
        img.data.fill(255, ((pa.h - 1 - cy) * pa.w + cx) * 4, ((pa.h - 1 - cy) * pa.w + cx) * 4 + 4);
    }
  mctx.putImageData(img, 0, 0);
  return mc;
}

// Keeps only the playable area. The mask is optional: one too big for a canvas (a crafted, very
// long map) leaves every floor drawn rather than losing the whole map.
/** @param {Drawing} at @param {Playable | undefined} pa */
function clipToPlayable({ c, X, Y, res }, pa) {
  if (pa?.bits)
    try {
      const mc = playableMask(pa);
      // Smoothing softens the mask's voxel steps into soft edges.
      c.imageSmoothingEnabled = true;
      c.globalCompositeOperation = "destination-in";
      c.drawImage(
        mc,
        X(pa.origin[0]),
        Y(pa.origin[1] + pa.h * pa.cell),
        (pa.w * pa.cell) / res,
        (pa.h * pa.cell) / res,
      );
    } catch (e) {
      console.warn("no playable area", e);
    } finally {
      c.globalCompositeOperation = "source-over";
    }
}

// bg=flat and bg=outline: the floors with their gaps filled in, under the floors or as a line round them.
/** @param {Drawing} at @param {HTMLCanvasElement} oc @param {Geometry["bounds"][0]} lo @param {Geometry["bounds"][1]} hi */
function plainBackground({ c, w, h }, oc, lo, hi) {
  // On a smaller copy: filling the gaps takes many full-canvas draws, and none of it is fine detail.
  const shrink = Math.min(1, PLAIN_SIDE / Math.max(w, h));
  const small = canvasLike({ width: Math.ceil(w * shrink), height: Math.ceil(h * shrink) }, s =>
    s.drawImage(oc, 0, 0, Math.ceil(w * shrink), Math.ceil(h * shrink)),
  );
  const solid = withoutGaps(small);
  if (PLAIN_BG === "outline") return { canvas: outlineOf(solid), lo, hi };
  c.globalCompositeOperation = "destination-over";
  c.drawImage(tinted(solid, FLAT_SHADES[0]), 0, 0, w, h);
  c.globalCompositeOperation = "source-over";
  return { canvas: oc, lo, hi };
}

// The payload track: ties, then the rails over them.
// Each segment is its two ends, x y z and x y z.
/** @param {Drawing} at @param {Geometry["track"]} segments */
function drawTrack({ c, X, Y }, segments) {
  const track = (segments || []).map(([x1, y1, , x2, y2]) => [X(x1), Y(y1), X(x2), Y(y2)]);
  c.strokeStyle = TIE_COLOUR;
  c.lineWidth = 2;
  c.beginPath();
  for (const [x1, y1, x2, y2] of track) {
    const len = Math.hypot(x2 - x1, y2 - y1);
    if (!len) continue;
    const ux = (x2 - x1) / len,
      uy = (y2 - y1) / len,
      nx = (-uy * TIE_LENGTH) / 2,
      ny = (ux * TIE_LENGTH) / 2;
    for (let d = TIE_STEP / 2; d < len; d += TIE_STEP) {
      const x = x1 + ux * d,
        y = y1 + uy * d;
      c.moveTo(x - nx, y - ny);
      c.lineTo(x + nx, y + ny);
    }
  }
  c.stroke();
  c.strokeStyle = RAIL_COLOUR;
  c.lineWidth = 1;
  c.lineCap = "round";
  c.beginPath();
  for (const [x1, y1, x2, y2] of track) {
    const len = Math.hypot(x2 - x1, y2 - y1);
    if (!len) continue;
    const nx = (-(y2 - y1) / len) * RAIL_GAP,
      ny = ((x2 - x1) / len) * RAIL_GAP;
    for (const s of [-1, 1]) {
      c.moveTo(x1 + s * nx, y1 + s * ny);
      c.lineTo(x2 + s * nx, y2 + s * ny);
    }
  }
  c.stroke();
}

// The map's geometry drawn from above onto a canvas, with the world box it covers.
// The server always sends the floors and walls.
/** @param {Geometry & Required<Pick<Geometry, "floors" | "walls">>} g */
export function renderGeometry(g) {
  const [lo, hi] = g.bounds;
  // World units per pixel; huge maps (MGE) get coarser so the canvas stays within browser limits.
  const res = Math.max(MIN_UNITS_PER_PIXEL, Math.max(hi[0] - lo[0], hi[1] - lo[1]) / MAX_CANVAS);
  const w = Math.ceil((hi[0] - lo[0]) / res),
    h = Math.ceil((hi[1] - lo[1]) / res);
  const oc = document.createElement("canvas");
  oc.width = w;
  oc.height = h;
  const c = context2d(oc);
  // Roofs hide what is under them; keep floors up to a little above the highest pickup/point
  // level, which the bounds' top is GEOMETRY_PAD above.
  const cut = hi[2] - GEOMETRY_PAD + ROOF_ABOVE;
  const X = /** @param {number} x */ x => (x - lo[0]) / res,
    Y = /** @param {number} y */ y => (hi[1] - y) / res;
  const at = { c, w, h, X, Y, res, cut };
  drawFloors(at, g.floors);
  drawWalls(at, g.walls);
  clipToPlayable(at, g.playable);
  if (PLAIN_BG) return plainBackground(at, oc, lo, hi);
  drawTrack(at, g.track);
  return { canvas: oc, lo, hi };
}
