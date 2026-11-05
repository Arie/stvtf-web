import { $, OVERLAY, OPT, overlayUnit, canvas, uiScale, ctx, context2d, state, canvasSize } from "./core.js";
import { env, asset } from "./env.js";
import { renderGeometry } from "./geometry.js";
/** @typedef {import("../types/page").Overview} Overview */
/** @typedef {import("../types/page").Geometry} Geometry */
/** @typedef {{ x: number, y: number }} Point */

// The HUD's player columns (shown on screens wider than this) and the scoreboard, in CSS pixels.
const HUD_MIN_WIDTH = 800,
  HUD_COLUMN = 268,
  SCOREBOARD_HEIGHT = 140;
// World units kept around the spawns and points when framing the map.
const FRAME_PAD = 600;

// ---------- map backgrounds
// demos.tf's bounds that do not line up with their image, fitted to the map's cart track and walls.
const BOUNDS_FIXES = {
  pl_upward: { boundary_min: { x: -2761, y: -4412 }, boundary_max: { x: 2961, y: 2112 } },
  koth_ashville_rc1: { boundary_min: { x: -1491, y: -3903 }, boundary_max: { x: 1433, y: 3907 } },
};
/** @type {Promise<{ [map: string]: Overview["b"] }> | null} */
let boundaries = null;
const demostfBoundaries = () =>
  (boundaries ??= fetch(asset("overview/boundaries.json"))
    .then(r => r.json())
    .then(j => ({ ...j, ...BOUNDS_FIXES }))
    .catch(() => ({})));
// demos.tf's own lookup: exact name, then names with version suffixes trimmed.
/** @param {string} map @param {{ [map: string]: Overview["b"] }} known */
export function demostfAlias(map, known) {
  /** @param {string} m */
  const trim = m => {
    while (m.lastIndexOf("_") > m.indexOf("_")) m = m.slice(0, m.lastIndexOf("_"));
    return m;
  };
  if (map.startsWith("cp_gullywash_f") && !map.startsWith("cp_gullywash_final")) return "cp_gullywash_f6";
  if (known[map]) return map;
  const t = trim(map);
  if (known[t]) return t;
  for (const k of Object.keys(known)) if (trim(k) === map) return k;
  for (const k of Object.keys(known)) if (trim(k) === t) return k;
  return null;
}

// Whether an overview of another version of the map covers this version's own bounds (at least
// half of it does). Versions can move the whole map: koth_clearcut's overview is nowhere near
// koth_clearcut_b18, while the overviews that line up cover all or nearly all of it.
/** @param {Overview["b"]} b @param {Geometry | null} g */
export function sameLayout(b, g) {
  if (!g?.bounds) return true;
  const [[x0, y0], [x1, y1]] = g.bounds,
    lo = b.boundary_min,
    hi = b.boundary_max;
  const w = Math.min(x1, hi.x) - Math.max(x0, lo.x),
    h = Math.min(y1, hi.y) - Math.max(y0, lo.y);
  return w > 0 && h > 0 && w * h >= 0.5 * (hi.x - lo.x) * (hi.y - lo.y);
}

/** @param {string} map */
export async function loadMap(map) {
  state.map = map;
  state.geometry = null;
  state.geoCanvas = null;
  state.overview = null;
  state.fitted = false;
  state.framed = false;
  state.spawns = null;
  state.mapData = false;
  $("map").textContent = map;
  const known = await demostfBoundaries();
  const alias = demostfAlias(map, known);
  const geometry = fetch(asset(`map/${encodeURIComponent(map)}`)).then(r => r.json());
  if (alias) {
    const img = new Image();
    img.onload = async () => {
      if (alias !== map && !sameLayout(known[alias], await geometry.catch(() => null))) return;
      if (state.map !== map) return;
      state.overview = { img: OVERLAY ? withoutBlack(img) : flattened(img), b: known[alias] };
      $("bgtoggle").hidden = false;
      if (!state.fitted) fitTo(known[alias].boundary_min, known[alias].boundary_max);
    };
    img.src = asset(`overview/${alias}.webp`);
  }
  $("bgtoggle").hidden = true;
  try {
    const g = await geometry;
    if (state.map !== map) return;
    state.geometry = g;
    state.spawns = g.spawns || null;
    state.geoCanvas = renderGeometry(g);
    if (!state.fitted && g.bounds)
      fitTo({ x: g.bounds[0][0], y: g.bounds[0][1] }, { x: g.bounds[1][0], y: g.bounds[1][1] });
  } catch (e) {
    console.warn("no geometry", e);
  }
  if (state.map === map) state.mapData = true;
}

// ---------- view
// Whether the main view shows the player HUD or the scoreboard (h and s hide them).
/** @param {string} what */
const shown = what =>
  !OVERLAY && !env.el.classList.contains("nohud") && !(what === "score" && env.el.classList.contains("noscore"));

/** @param {Point} min @param {Point} max */
function fitTo(min, max) {
  const r = canvas.getBoundingClientRect(),
    v = state.view;
  v.cx = (min.x + max.x) / 2;
  v.cy = (min.y + max.y) / 2;
  // The box's extent once rotated into the view.
  const c = Math.abs(Math.cos(v.rot)),
    s = Math.abs(Math.sin(v.rot));
  const w = max.x - min.x,
    h = max.y - min.y,
    rw = w * c + h * s,
    rh = w * s + h * c;
  // Keep the map clear of the HUD columns on both sides.
  const hud = shown("hud") && r.width > HUD_MIN_WIDTH ? 2 * HUD_COLUMN * uiScale() : 0;
  v.scale = Math.min((r.width - hud) / rw, r.height / rh) * (OVERLAY ? 0.96 : 0.95);
  state.fitted = true;
}

// Frame what matters (spawns and points) rather than the whole image, which is mostly empty.
// Both views turn the map so blue's spawns are on the left and red's on the right, as on the HUD
// (rotate=teams, the default; upright when a team has no spawns). rotate=auto stays upright, or
// turns a quarter when that fills the panel clearly better; rotate=<degrees> turns it that far.
// The part of the canvas the map has to itself: the main view keeps clear of the HUD columns
// and the scoreboard; the overlay uses its whole panel. Framings are centred in it.
export function viewArea() {
  const r = canvasSize();
  const side = shown("hud") && r.width > HUD_MIN_WIDTH ? HUD_COLUMN * uiScale() : 0,
    top = shown("score") ? SCOREBOARD_HEIGHT * uiScale() : 0;
  return { w: r.width - 2 * side, h: r.height - top, top };
}
// The world offset of a point `dx`, `dy` pixels right of and below the canvas centre, in a view
// at `scale` turned by `rot`.
/** @param {number} dx @param {number} dy @param {number} scale @param {number} [rot] @returns {[number, number]} */
export function screenOffset(dx, dy, scale, rot = state.view.rot) {
  const a = dx / scale,
    b = -dy / scale,
    c = Math.cos(rot),
    s = Math.sin(rot);
  return [a * c + b * s, -a * s + b * c];
}
// Framings are kept as the world point at the centre of the free area: the canvas centre sits
// half the scoreboard above it on screen (the overlay has no scoreboard).
/** @param {{ cx: number, cy: number }} focus @param {number} scale @returns {{ cx: number, cy: number }} */
export function viewCentre(focus, scale) {
  const [dx, dy] = screenOffset(0, -viewArea().top / 2, scale);
  return { cx: focus.cx + dx, cy: focus.cy + dy };
}
// The framing point of the view `v`: the world point at the centre of its free area.
/** @param {{ cx: number, cy: number, scale: number }} v */
export function focusOf(v) {
  const [dx, dy] = screenOffset(0, viewArea().top / 2, v.scale);
  return { cx: v.cx + dx, cy: v.cy + dy };
}

// The quarter turn that puts the middle of blue's spawn points left of red's, or null without both.
/** @param {{ pos: number[], team: string }[]} points */
export function teamsTurn(points) {
  /** @param {string} team */
  const middle = team => {
    const t = points.filter(p => p.team === team);
    return t.length ? [0, 1].map(k => t.reduce((a, p) => a + p.pos[k], 0) / t.length) : null;
  };
  const blue = middle("blue"),
    red = middle("red");
  if (!blue || !red) return null;
  const quarter = Math.PI / 2;
  // A turn by `rot` shows the world direction -rot as the screen's +x.
  return Math.round(-Math.atan2(red[1] - blue[1], red[0] - blue[0]) / quarter) * quarter || 0;
}

export function frameMap(keepView = false) {
  /** @type {number[][]} */
  const pts = [];
  for (const p of state.spawns?.points || []) pts.push(p.pos);
  for (const o of state.snaps.at(-1)?.objectives || []) if (o.pos) pts.push(o.pos);
  if (pts.length < 3) return false;
  const v = state.view,
    pad = FRAME_PAD,
    { w, h } = viewArea();
  // Centre and scale for a rotation, measuring the points along the rotated axes.
  /** @param {number} rot */
  const fit = rot => {
    const c = Math.cos(rot),
      sn = Math.sin(rot);
    const us = pts.map(p => p[0] * c - p[1] * sn),
      vs = pts.map(p => p[0] * sn + p[1] * c);
    const u0 = Math.min(...us) - pad,
      u1 = Math.max(...us) + pad,
      v0 = Math.min(...vs) - pad,
      v1 = Math.max(...vs) + pad;
    const uc = (u0 + u1) / 2,
      vc = (v0 + v1) / 2;
    const scale = Math.min(w / (u1 - u0), h / (v1 - v0)) * 0.96;
    return { rot, cx: uc * c + vc * sn, cy: -uc * sn + vc * c, scale };
  };
  const want = OPT.get("rotate") ?? "teams";
  let best;
  if (want === "teams") {
    best = fit(teamsTurn(state.spawns?.points || []) ?? 0);
  } else if (want === "auto") {
    const upright = fit(0),
      turned = fit(Math.PI / 2);
    best = turned.scale > upright.scale * 1.1 ? turned : upright;
  } else {
    best = fit(((Number(want) || 0) * Math.PI) / 180);
  }
  if (!keepView || best.rot !== v.rot)
    Object.assign(v, { rot: best.rot, ...viewCentre(best, best.scale), scale: best.scale });
  state.full = { cx: best.cx, cy: best.cy, scale: best.scale };
  state.camTarget = null;
  state.fitted = state.framed = true;
  return true;
}
// The view can be rotated (the overlay turns a map so its long axis fits the panel).
/** @param {number} x @param {number} y @returns {[number, number]} */
export function toScreen(x, y) {
  const r = canvasSize(),
    v = state.view;
  const dx = x - v.cx,
    dy = y - v.cy,
    c = Math.cos(v.rot),
    s = Math.sin(v.rot);
  return [(dx * c - dy * s) * v.scale + r.width / 2, -(dx * s + dy * c) * v.scale + r.height / 2];
}
// A world yaw (degrees, counter-clockwise from +x) as a canvas angle.
/** @param {number} deg */
export function screenAngle(deg) {
  return -((deg * Math.PI) / 180 + state.view.rot);
}

// ---------- drawing
// Text drawn once per string, font and colour, then copied: setting a canvas font and drawing
// text are slow enough to show every frame. `x` of a sprite is the left of its text.
const TEXT_PAD = 3;
/** @type {Map<string, { c: HTMLCanvasElement, w: number, h: number }>} */
const textSprites = new Map();
/** @param {string} text @param {string} font @param {string} fill @param {string | null} outline */
function textSprite(text, font, fill, outline) {
  const dpr = window.devicePixelRatio || 1,
    key = `${text}\n${font}\n${fill}\n${outline}\n${dpr}`;
  let sp = textSprites.get(key);
  if (!sp) {
    if (textSprites.size > 128) textSprites.clear();
    const c = document.createElement("canvas"),
      g = context2d(c);
    g.font = font;
    const m = g.measureText(text),
      size = parseFloat(font.match(/(\d+(?:\.\d+)?)px/)?.[1] || "12");
    const w = Math.ceil(m.width) + TEXT_PAD * 2,
      h = Math.ceil(size * 1.3) + TEXT_PAD * 2;
    c.width = Math.ceil(w * dpr);
    c.height = Math.ceil(h * dpr);
    g.scale(dpr, dpr);
    g.font = font;
    g.textBaseline = "top";
    g.lineJoin = "round";
    if (outline) {
      g.lineWidth = 3;
      g.strokeStyle = outline;
      g.strokeText(text, TEXT_PAD, TEXT_PAD);
    }
    g.fillStyle = fill;
    g.fillText(text, TEXT_PAD, TEXT_PAD);
    textSprites.set(key, (sp = { c, w, h }));
  }
  return sp;
}
// A short label (a class, a building's letter, a count) centred on a marker at x, y, in `size`
// pixels; half a pixel low, which looks centred.
/** @param {string} text @param {number} x @param {number} y @param {number} size */
export function label(text, x, y, size, { colour = "#fff", weight = "800" } = {}) {
  ctx.fillStyle = colour;
  ctx.font = `${weight} ${Math.round(size)}px system-ui`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(text, x, y + 0.5);
}
// Draws a text sprite in `look`'s font, fill and outline (or none), with the top of its text at
// y, centred on x or starting at x.
/**
 * @param {string} text @param {{ font: string, fill: string, outline: string | null }} look
 * @param {number} x @param {number} y @param {boolean} centre
 */
export function drawText(text, look, x, y, centre) {
  const { c, w, h } = textSprite(text, look.font, look.fill, look.outline);
  ctx.drawImage(c, centre ? x - w / 2 : x - TEXT_PAD, y - TEXT_PAD, w, h);
}

/** @param {number} units @param {number} min */
export function px(units, min) {
  return Math.max(min * uiScale(), units * state.view.scale);
}
export const playerRad = () => (OVERLAY ? overlayUnit() : px(30, 11));

// The overview images have an opaque black surround; on a rotated, transparent overlay that
// would show as a tilted black box, so make the near-black pixels transparent.
/** @param {HTMLImageElement} img */
function withoutBlack(img) {
  const c = document.createElement("canvas");
  c.width = img.naturalWidth;
  c.height = img.naturalHeight;
  const g = context2d(c);
  g.drawImage(img, 0, 0);
  try {
    const d = g.getImageData(0, 0, c.width, c.height),
      rgba = d.data;
    for (let i = 0; i < rgba.length; i += 4) {
      const lum = Math.max(rgba[i], rgba[i + 1], rgba[i + 2]);
      if (lum < 14) rgba[i + 3] = 0;
      else if (lum < 28) rgba[i + 3] = Math.round((255 * (lum - 14)) / 14);
    }
    g.putImageData(d, 0, 0);
  } catch {
    return img;
  }
  return c;
}

// The overview at 85% over the page background, drawn once: a plain copy each frame is much
// cheaper than blending an image element.
/** @param {HTMLImageElement} img */
function flattened(img) {
  const c = document.createElement("canvas");
  c.width = img.naturalWidth;
  c.height = img.naturalHeight;
  const g = context2d(c);
  g.fillStyle = "#101214";
  g.fillRect(0, 0, c.width, c.height);
  g.globalAlpha = 0.85;
  g.drawImage(img, 0, 0);
  return c;
}

// The map picture is an element under the canvas, moved with a CSS transform: the compositor
// scales and turns it, so the canvas does not redraw a full-screen image every frame.
const bgBox = $("bg");
// bgopacity=0..1: how strongly the background shows, whichever it is.
const BG_OPACITY = (v => (Number.isFinite(v) ? String(Math.max(0, Math.min(1, v))) : null))(
  Number(OPT.get("bgopacity") || NaN),
);
/** @type {HTMLImageElement | HTMLCanvasElement | null} */
let bgShown = null,
  bgTransform = "";
/** @param {{ width: number, height: number }} r */
export function drawBackground(r) {
  ctx.clearRect(0, 0, r.width, r.height);
  /** @type {HTMLImageElement | HTMLCanvasElement | null} */
  let pic = null;
  /** @type {number[] | undefined} */
  let lo, hi;
  // The overview also stands in once it is clear this map has no geometry.
  if (state.overview && (state.bg === "overview" || (state.mapData && !state.geoCanvas))) {
    const { img, b } = state.overview;
    pic = img;
    lo = [b.boundary_min.x, b.boundary_min.y];
    hi = [b.boundary_max.x, b.boundary_max.y];
  } else if (state.geoCanvas) {
    ({ canvas: pic, lo, hi } = state.geoCanvas);
  }
  if (pic !== bgShown) {
    bgBox.replaceChildren(...(pic ? [pic] : []));
    if (pic) pic.style.opacity = BG_OPACITY ?? (OVERLAY && pic !== state.geoCanvas?.canvas ? "0.95" : "");
    bgShown = pic;
    bgTransform = "";
  }
  if (!pic || !lo || !hi) return;
  const [cx, cy] = toScreen((lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2);
  const w = (hi[0] - lo[0]) * state.view.scale,
    h = (hi[1] - lo[1]) * state.view.scale;
  const [pw, ph] = "naturalWidth" in pic ? [pic.naturalWidth, pic.naturalHeight] : [pic.width, pic.height];
  const t = `translate(${cx}px, ${cy}px) rotate(${-state.view.rot}rad) scale(${w / pw}, ${h / ph}) translate(-50%, -50%)`;
  if (t !== bgTransform) {
    pic.style.transform = t;
    bgTransform = t;
  }
}
