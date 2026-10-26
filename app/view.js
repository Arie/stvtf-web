import { $, OVERLAY, OPT, overlayUnit, canvas, uiScale, ctx, state, canvasSize } from "./core.js";
import { env, asset } from "./env.js";

// Map geometry: at least this many world units per pixel, and a canvas at most this wide;
// bsp.rs pads the bounds' top this far above the highest point or pickup (its PAD), floors are
// kept up to this far above that, and a wall counts from this far below its top.
const MIN_UNITS_PER_PIXEL = 4, MAX_CANVAS = 4096, GEOMETRY_PAD = 1536, ROOF_ABOVE = 400, WALL_BASE = 64;
// Floor colours by surface kind (bsp.rs's material words); anything else, or none, is "other".
const KIND_COLOURS = { __proto__: null, grass: "#56703d", dirt: "#86704f", concrete: "#7b7870", metal: "#5e6872",
  wood: "#7f5a39", tile: "#76665a", water: "#2c5a63", lava: "#93401f", snow: "#c7d1d8", other: "#6c6c6c" };
// Water shows the floor under it; raised floors cast a short soft shadow (canvas pixels).
const WATER_ALPHA = 0.45, SHADOW_COLOUR = "rgba(0,0,0,0.45)", SHADOW_BLUR = 6, SHADOW_OFFSET = 3;
// Payload track: ties this far apart and this long, rails this far either side (canvas pixels).
const TIE_STEP = 7, TIE_LENGTH = 10, RAIL_GAP = 3, TIE_COLOUR = "#3a2c20", RAIL_COLOUR = "#cdc8bc";
// The HUD's player columns (shown on screens wider than this) and the scoreboard, in CSS pixels.
const HUD_MIN_WIDTH = 800, HUD_COLUMN = 268, SCOREBOARD_HEIGHT = 140;
// World units kept around the spawns and points when framing the map.
const FRAME_PAD = 600;

// ---------- map backgrounds
let boundaries = null;
async function demostfBoundaries() {
  if (!boundaries) {
    try { boundaries = await (await fetch(asset("overview/boundaries.json"))).json(); }
    catch { boundaries = {}; }
  }
  return boundaries;
}
// demos.tf's own lookup: exact name, then names with version suffixes trimmed.
function demostfAlias(map, known) {
  const trim = m => { while (m.lastIndexOf("_") > m.indexOf("_")) m = m.slice(0, m.lastIndexOf("_")); return m; };
  if (map.startsWith("cp_gullywash_f") && !map.startsWith("cp_gullywash_final")) return "cp_gullywash_f6";
  if (known[map]) return map;
  const t = trim(map);
  if (known[t]) return t;
  for (const k of Object.keys(known)) if (trim(k) === map) return k;
  for (const k of Object.keys(known)) if (trim(k) === t) return k;
  return null;
}

export async function loadMap(map) {
  state.map = map; state.geometry = null; state.geoCanvas = null; state.overview = null; state.fitted = false; state.framed = false;
  state.spawns = null; state.mapData = false;
  $("map").textContent = map;
  const known = await demostfBoundaries();
  const alias = demostfAlias(map, known);
  if (alias) {
    const img = new Image();
    img.onload = () => {
      state.overview = { img: OVERLAY ? withoutBlack(img) : flattened(img), b: known[alias] };
      $("bgtoggle").hidden = false;
      if (!state.fitted) fitTo(known[alias].boundary_min, known[alias].boundary_max);
    };
    img.src = asset(`overview/${alias}.webp`);
  }
  $("bgtoggle").hidden = true;
  try {
    const g = await (await fetch(asset(`map/${encodeURIComponent(map)}`))).json();
    if (state.map !== map) return;
    state.geometry = g;
    state.spawns = g.spawns || null;
    state.geoCanvas = renderGeometry(g);
    if (!state.fitted && g.bounds) fitTo({ x: g.bounds[0][0], y: g.bounds[0][1] }, { x: g.bounds[1][0], y: g.bounds[1][1] });
  } catch (e) { console.warn("no geometry", e); }
  if (state.map === map) state.mapData = true;
}

// A small tile of subtle marks per kind, made once with seeded noise so a map looks the same
// on every load; kinds without one are plain.
const kindTiles = {};
function kindTile(k) {
  if (k in kindTiles) return kindTiles[k];
  let seed = 7;
  const rnd = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  const size = { grass: [16, 16], dirt: [48, 48], metal: [8, 8], wood: [32, 10], tile: [12, 12], water: [24, 12] }[k];
  if (!size) return (kindTiles[k] = null);
  const t = document.createElement("canvas"); [t.width, t.height] = size;
  const c = t.getContext("2d"), light = a => `rgba(255,255,255,${a})`, dark = a => `rgba(0,0,0,${a})`;
  if (k === "grass") {
    for (let i = 0; i < 40; i++) { c.fillStyle = rnd() < 0.5 ? light(0.1) : dark(0.12); c.fillRect(Math.floor(rnd() * 16), Math.floor(rnd() * 16), 1, 1); }
  } else if (k === "dirt") {
    for (let i = 0; i < 14; i++) {
      const x = rnd() * 48, y = rnd() * 48, r = 4 + rnd() * 8, tint = rnd() < 0.5 ? "255,255,255" : "0,0,0";
      // Drawn at every wrap so blotches cross the tile's edges without a seam.
      for (const [dx, dy] of [[0, 0], [-48, 0], [0, -48], [-48, -48], [48, 0], [0, 48], [48, 48], [-48, 48], [48, -48]]) {
        const gr = c.createRadialGradient(x + dx, y + dy, 0, x + dx, y + dy, r);
        gr.addColorStop(0, `rgba(${tint},0.09)`); gr.addColorStop(1, `rgba(${tint},0)`);
        c.fillStyle = gr; c.fillRect(x + dx - r, y + dy - r, 2 * r, 2 * r);
      }
    }
  } else if (k === "metal") {
    c.strokeStyle = light(0.09); c.lineWidth = 1;
    c.beginPath(); c.moveTo(-1, 9); c.lineTo(9, -1); c.moveTo(-1, 1); c.lineTo(1, -1); c.moveTo(7, 9); c.lineTo(9, 7); c.stroke();
  } else if (k === "wood") {
    c.fillStyle = dark(0.18); c.fillRect(0, 9, 32, 1); c.fillRect(Math.floor(rnd() * 32), 0, 1, 9);
    c.fillStyle = light(0.05); c.fillRect(0, 0, 32, 1);
  } else if (k === "tile") {
    c.fillStyle = dark(0.14); c.fillRect(0, 11, 12, 1); c.fillRect(11, 0, 1, 11);
  } else if (k === "water") {
    c.strokeStyle = light(0.12); c.lineWidth = 1;
    c.beginPath();
    for (let x = 0; x <= 24; x++) (x ? c.lineTo : c.moveTo).call(c, x, 6 + 2 * Math.sin(x / 24 * 2 * Math.PI));
    c.stroke();
  }
  return (kindTiles[k] = t);
}

// A hex colour scaled by s, as rgb().
function scaled(hex, s) {
  const n = parseInt(hex.slice(1), 16), ch = sh => Math.min(255, Math.round((n >> sh & 255) * s));
  return `rgb(${ch(16)} ${ch(8)} ${ch(0)})`;
}

function renderGeometry(g) {
  const [lo, hi] = g.bounds;
  // World units per pixel; huge maps (MGE) get coarser so the canvas stays within browser limits.
  const res = Math.max(MIN_UNITS_PER_PIXEL, Math.max(hi[0] - lo[0], hi[1] - lo[1]) / MAX_CANVAS);
  const w = Math.ceil((hi[0] - lo[0]) / res), h = Math.ceil((hi[1] - lo[1]) / res);
  const oc = document.createElement("canvas"); oc.width = w; oc.height = h;
  const c = oc.getContext("2d");
  const zs = g.floors.map(f => f.z);
  const zlo = zs.length ? zs[Math.floor(zs.length * 0.02)] : 0, zhi = zs.length ? zs[Math.floor(zs.length * 0.98)] : 1;
  // Roofs hide what is under them; keep floors up to a little above the highest pickup/point
  // level, which the bounds' top is GEOMETRY_PAD above.
  const cut = hi[2] - GEOMETRY_PAD + ROOF_ABOVE;
  const X = x => (x - lo[0]) / res, Y = y => (hi[1] - y) / res;
  const patterns = {};
  c.lineWidth = 1; c.shadowBlur = SHADOW_BLUR; c.shadowOffsetX = c.shadowOffsetY = SHADOW_OFFSET;
  for (const f of g.floors) {
    if (f.z > cut) continue;
    const t = Math.max(0, Math.min(1, (f.z - zlo) / (zhi - zlo || 1)));
    const k = f.k in KIND_COLOURS ? f.k : "other", water = k === "water";
    c.fillStyle = c.strokeStyle = scaled(KIND_COLOURS[k], 0.70 + 0.40 * t);
    c.beginPath();
    for (let i = 0; i < f.p.length; i += 2) (i ? c.lineTo : c.moveTo).call(c, X(f.p[i]), Y(f.p[i + 1]));
    c.closePath();
    // Terrain is one smooth surface, so its cells do not shadow each other.
    c.shadowColor = f.d || water ? "transparent" : SHADOW_COLOUR;
    c.globalAlpha = water ? WATER_ALPHA : 1;
    c.fill();
    c.shadowColor = "transparent";
    // The floor's own colour round its edge, so neighbouring polygons leave no hairline seams.
    c.stroke();
    if (!(k in patterns)) patterns[k] = kindTile(k) && c.createPattern(kindTile(k), "repeat");
    if (patterns[k]) { c.fillStyle = patterns[k]; c.fill(); }
  }
  c.shadowColor = "transparent"; c.shadowBlur = c.shadowOffsetX = c.shadowOffsetY = 0; c.globalAlpha = 1;
  // Walls only where a floor was drawn: the rest are scenery's, out over the background.
  c.strokeStyle = "#0b0c0e"; c.lineWidth = 1.2; c.globalCompositeOperation = "source-atop";
  c.beginPath();
  for (const wl of g.walls) {
    if (wl.z - WALL_BASE > cut) continue;
    c.moveTo(X(wl.p[0]), Y(wl.p[1])); c.lineTo(X(wl.p[2]), Y(wl.p[3]));
  }
  c.stroke();
  c.globalCompositeOperation = "source-over";
  const pa = g.playable;
  // The mask is optional: one too big for a canvas (a crafted, very long map) leaves every floor
  // drawn rather than losing the whole map.
  if (pa?.bits) try {
    // Rows run from the lowest y up, so they are flipped into the canvas.
    const bytes = Uint8Array.from(atob(pa.bits), ch => ch.charCodeAt(0));
    const mc = document.createElement("canvas"); mc.width = pa.w; mc.height = pa.h;
    const mctx = mc.getContext("2d"), img = mctx.createImageData(pa.w, pa.h);
    for (let cy = 0; cy < pa.h; cy++) for (let cx = 0; cx < pa.w; cx++) {
      const i = cy * pa.w + cx;
      if (bytes[i >> 3] >> (i & 7) & 1) img.data.fill(255, ((pa.h - 1 - cy) * pa.w + cx) * 4, ((pa.h - 1 - cy) * pa.w + cx) * 4 + 4);
    }
    mctx.putImageData(img, 0, 0);
    // Smoothing softens the mask's voxel steps into soft edges.
    c.imageSmoothingEnabled = true;
    c.globalCompositeOperation = "destination-in";
    c.drawImage(mc, X(pa.origin[0]), Y(pa.origin[1] + pa.h * pa.cell), pa.w * pa.cell / res, pa.h * pa.cell / res);
  } catch (e) { console.warn("no playable area", e); } finally { c.globalCompositeOperation = "source-over"; }
  const track = (g.track || []).map(([x1, y1, , x2, y2]) => [X(x1), Y(y1), X(x2), Y(y2)]);
  c.strokeStyle = TIE_COLOUR; c.lineWidth = 2;
  c.beginPath();
  for (const [x1, y1, x2, y2] of track) {
    const len = Math.hypot(x2 - x1, y2 - y1);
    if (!len) continue;
    const ux = (x2 - x1) / len, uy = (y2 - y1) / len, nx = -uy * TIE_LENGTH / 2, ny = ux * TIE_LENGTH / 2;
    for (let d = TIE_STEP / 2; d < len; d += TIE_STEP) {
      const x = x1 + ux * d, y = y1 + uy * d;
      c.moveTo(x - nx, y - ny); c.lineTo(x + nx, y + ny);
    }
  }
  c.stroke();
  c.strokeStyle = RAIL_COLOUR; c.lineWidth = 1; c.lineCap = "round";
  c.beginPath();
  for (const [x1, y1, x2, y2] of track) {
    const len = Math.hypot(x2 - x1, y2 - y1);
    if (!len) continue;
    const nx = -(y2 - y1) / len * RAIL_GAP, ny = (x2 - x1) / len * RAIL_GAP;
    for (const s of [-1, 1]) { c.moveTo(x1 + s * nx, y1 + s * ny); c.lineTo(x2 + s * nx, y2 + s * ny); }
  }
  c.stroke();
  return { canvas: oc, lo, hi };
}

// ---------- view
// Whether the main view shows the player HUD or the scoreboard (h and s hide them).
const shown = what => !OVERLAY && !env.el.classList.contains("nohud") &&
  !(what === "score" && env.el.classList.contains("noscore"));

function fitTo(min, max) {
  const r = canvas.getBoundingClientRect(), v = state.view;
  v.cx = (min.x + max.x) / 2; v.cy = (min.y + max.y) / 2;
  // The box's extent once rotated into the view.
  const c = Math.abs(Math.cos(v.rot)), s = Math.abs(Math.sin(v.rot));
  const w = max.x - min.x, h = max.y - min.y, rw = w * c + h * s, rh = w * s + h * c;
  // Keep the map clear of the HUD columns on both sides.
  const hud = shown("hud") && r.width > HUD_MIN_WIDTH ? 2 * HUD_COLUMN * uiScale() : 0;
  v.scale = Math.min((r.width - hud) / rw, r.height / rh) * (OVERLAY ? 0.96 : 0.95);
  state.fitted = true;
}

// Frame what matters (spawns and points) rather than the whole image, which is mostly empty.
// The main view stays upright and clear of the HUD columns and scoreboard. The overlay with
// rotate=auto stays upright, or turns a quarter when that fills the panel clearly better.
// The part of the canvas the map has to itself: the main view keeps clear of the HUD columns
// and the scoreboard; the overlay uses its whole panel. Framings are centred in it.
function viewArea() {
  const r = canvasSize();
  const side = shown("hud") && r.width > HUD_MIN_WIDTH ? HUD_COLUMN * uiScale() : 0, top = shown("score") ? SCOREBOARD_HEIGHT * uiScale() : 0;
  return { w: r.width - 2 * side, h: r.height - top, top };
}
// Framings are kept as the world point at the centre of the free area; the view centre sits
// below it by half the scoreboard (the main view is never rotated, the overlay has no top).
export const viewCy = (focusCy, scale) => focusCy + viewArea().top / 2 / scale;

export function frameMap(keepView = false) {
  const pts = [];
  for (const p of state.spawns?.points || []) pts.push(p.pos);
  for (const o of state.snaps.at(-1)?.objectives || []) if (o.pos) pts.push(o.pos);
  if (pts.length < 3) return false;
  const v = state.view, pad = FRAME_PAD, { w, h } = viewArea();
  // Centre and scale for a rotation, measuring the points along the rotated axes.
  const fit = rot => {
    const c = Math.cos(rot), sn = Math.sin(rot);
    const us = pts.map(p => p[0] * c - p[1] * sn), vs = pts.map(p => p[0] * sn + p[1] * c);
    const u0 = Math.min(...us) - pad, u1 = Math.max(...us) + pad, v0 = Math.min(...vs) - pad, v1 = Math.max(...vs) + pad;
    const uc = (u0 + u1) / 2, vc = (v0 + v1) / 2;
    const scale = Math.min(w / (u1 - u0), h / (v1 - v0)) * 0.96;
    return { rot, cx: uc * c + vc * sn, cy: -uc * sn + vc * c, scale };
  };
  const want = OVERLAY ? (OPT.get("rotate") ?? "0") : "0";
  let best;
  if (want === "auto") {
    const upright = fit(0), turned = fit(Math.PI / 2);
    best = turned.scale > upright.scale * 1.1 ? turned : upright;
  } else {
    best = fit((Number(want) || 0) * Math.PI / 180);
  }
  if (!keepView || best.rot !== v.rot) Object.assign(v, { rot: best.rot, cx: best.cx, cy: viewCy(best.cy, best.scale), scale: best.scale });
  state.full = { cx: best.cx, cy: best.cy, scale: best.scale };
  state.camTarget = null;
  state.fitted = state.framed = true;
  return true;
}
// The view can be rotated (the overlay turns a map so its long axis fits the panel).
export function toScreen(x, y) {
  const r = canvasSize(), v = state.view;
  const dx = x - v.cx, dy = y - v.cy, c = Math.cos(v.rot), s = Math.sin(v.rot);
  return [(dx * c - dy * s) * v.scale + r.width / 2, -(dx * s + dy * c) * v.scale + r.height / 2];
}
// A world yaw (degrees, counter-clockwise from +x) as a canvas angle.
export function screenAngle(deg) { return -(deg * Math.PI / 180 + state.view.rot); }

// ---------- drawing
// Text drawn once per string, font and colour, then copied: setting a canvas font and drawing
// text are slow enough to show every frame. `x` of a sprite is the left of its text.
const TEXT_PAD = 3;
const textSprites = new Map();
export function textSprite(text, font, fill, outline) {
  const dpr = window.devicePixelRatio || 1, key = `${text}\n${font}\n${fill}\n${outline}\n${dpr}`;
  let sp = textSprites.get(key);
  if (!sp) {
    if (textSprites.size > 128) textSprites.clear();
    const c = document.createElement("canvas"), g = c.getContext("2d");
    g.font = font;
    const m = g.measureText(text), size = parseFloat(font.match(/(\d+(?:\.\d+)?)px/)?.[1] || 12);
    const w = Math.ceil(m.width) + TEXT_PAD * 2, h = Math.ceil(size * 1.3) + TEXT_PAD * 2;
    c.width = Math.ceil(w * dpr); c.height = Math.ceil(h * dpr);
    g.scale(dpr, dpr); g.font = font; g.textBaseline = "top"; g.lineJoin = "round";
    if (outline) { g.lineWidth = 3; g.strokeStyle = outline; g.strokeText(text, TEXT_PAD, TEXT_PAD); }
    g.fillStyle = fill; g.fillText(text, TEXT_PAD, TEXT_PAD);
    textSprites.set(key, sp = { c, w, h });
  }
  return sp;
}
// Draws a text sprite with the top of its text at y, centred on x or starting at x.
export function drawText(text, font, fill, outline, x, y, centre) {
  const { c, w, h } = textSprite(text, font, fill, outline);
  ctx.drawImage(c, (centre ? x - w / 2 : x - TEXT_PAD), y - TEXT_PAD, w, h);
}

export function px(units, min) { return Math.max(min * uiScale(), units * state.view.scale); }
export const playerRad = () => (OVERLAY ? overlayUnit() : px(30, 11));

// The overview images have an opaque black surround; on a rotated, transparent overlay that
// would show as a tilted black box, so make the near-black pixels transparent.
function withoutBlack(img) {
  const c = document.createElement("canvas");
  c.width = img.naturalWidth; c.height = img.naturalHeight;
  const g = c.getContext("2d");
  g.drawImage(img, 0, 0);
  try {
    const d = g.getImageData(0, 0, c.width, c.height), rgba = d.data;
    for (let i = 0; i < rgba.length; i += 4) {
      const lum = Math.max(rgba[i], rgba[i + 1], rgba[i + 2]);
      if (lum < 14) rgba[i + 3] = 0; else if (lum < 28) rgba[i + 3] = Math.round(255 * (lum - 14) / 14);
    }
    g.putImageData(d, 0, 0);
  } catch { return img; }
  return c;
}

// The overview at 85% over the page background, drawn once: a plain copy each frame is much
// cheaper than blending an image element.
function flattened(img) {
  const c = document.createElement("canvas");
  c.width = img.naturalWidth; c.height = img.naturalHeight;
  const g = c.getContext("2d");
  g.fillStyle = "#101214"; g.fillRect(0, 0, c.width, c.height);
  g.globalAlpha = 0.85; g.drawImage(img, 0, 0);
  return c;
}

// The map picture is an element under the canvas, moved with a CSS transform: the compositor
// scales and turns it, so the canvas does not redraw a full-screen image every frame.
const bgBox = $("bg");
let bgShown = null, bgTransform = "";
export function drawBackground(r) {
  ctx.clearRect(0, 0, r.width, r.height);
  let pic = null, lo, hi;
  if (state.bg === "overview" && state.overview) {
    const { img, b } = state.overview;
    pic = img; lo = [b.boundary_min.x, b.boundary_min.y]; hi = [b.boundary_max.x, b.boundary_max.y];
  } else if (state.geoCanvas) {
    ({ canvas: pic, lo, hi } = state.geoCanvas);
  }
  if (pic !== bgShown) {
    bgBox.replaceChildren(...(pic ? [pic] : []));
    if (pic) pic.style.opacity = OVERLAY && pic !== state.geoCanvas?.canvas ? "0.95" : "";
    bgShown = pic; bgTransform = "";
  }
  if (!pic) return;
  const [cx, cy] = toScreen((lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2);
  const w = (hi[0] - lo[0]) * state.view.scale, h = (hi[1] - lo[1]) * state.view.scale;
  const pw = pic.naturalWidth || pic.width, ph = pic.naturalHeight || pic.height;
  const t = `translate(${cx}px, ${cy}px) rotate(${-state.view.rot}rad) scale(${w / pw}, ${h / ph}) translate(-50%, -50%)`;
  if (t !== bgTransform) { pic.style.transform = t; bgTransform = t; }
}

// Auto zoom frames the fight: clumps of players first. A player on their own joins the
// framing only after STRAGGLE_MS (a respawn usually rejoins by then) and is then brought in
// slowly. The camera holds its framing until the fight leaves it or tightens a lot, and
// glides: never wider than the full framing, never tighter than MIN_SPAN units.
const MIN_SPAN = 2600, ACTION_PAD = 500, CLUMP_LINK = 1500, STRAGGLE_MS = 3000;
const straggling = new Map();
let lastFrame = performance.now();

// Players linked within CLUMP_LINK of each other form a clump; keep the biggest one and any
// other clump of three or more.
function fightingPlayers(alive) {
  const group = alive.map((_, i) => i);
  const root = i => (group[i] === i ? i : (group[i] = root(group[i])));
  for (let i = 0; i < alive.length; i++)
    for (let j = i + 1; j < alive.length; j++)
      if (Math.hypot(alive[i].pos[0] - alive[j].pos[0], alive[i].pos[1] - alive[j].pos[1]) < CLUMP_LINK) group[root(i)] = root(j);
  const sizes = new Map();
  alive.forEach((_, i) => sizes.set(root(i), (sizes.get(root(i)) || 0) + 1));
  const biggest = Math.max(...sizes.values());
  return alive.filter((_, i) => sizes.get(root(i)) === biggest || sizes.get(root(i)) >= 3);
}

export function followAction(players) {
  const now = performance.now(), dt = Math.min(0.1, (now - lastFrame) / 1000);
  lastFrame = now;
  const v = state.view, full = state.full;
  if (!full) return;
  const { w, h } = viewArea(), c = Math.cos(v.rot), sn = Math.sin(v.rot);
  const toUV = p => [p[0] * c - p[1] * sn, p[0] * sn + p[1] * c];
  const alive = players.filter(p => p.alive && p.pos && (p.team === "red" || p.team === "blue"));
  const fight = alive.length ? fightingPlayers(alive) : [];
  const loners = alive.filter(p => !fight.includes(p));
  for (const id of straggling.keys()) if (!loners.some(p => p.id === id)) straggling.delete(id);
  for (const p of loners) if (!straggling.has(p.id)) straggling.set(p.id, now);
  const framed = [...fight, ...loners.filter(p => now - straggling.get(p.id) > STRAGGLE_MS)];
  let wanted = { ...full };
  if (framed.length) {
    // Along the rotated axes, like the framing, so the box matches the panel.
    const uv = framed.map(p => toUV(p.pos));
    let u0 = Math.min(...uv.map(p => p[0])) - ACTION_PAD, u1 = Math.max(...uv.map(p => p[0])) + ACTION_PAD;
    let v0 = Math.min(...uv.map(p => p[1])) - ACTION_PAD, v1 = Math.max(...uv.map(p => p[1])) + ACTION_PAD;
    const grow = (a, b) => { const m = (a + b) / 2, half = Math.max((b - a) / 2, MIN_SPAN / 2); return [m - half, m + half]; };
    [u0, u1] = grow(u0, u1); [v0, v1] = grow(v0, v1);
    const scale = Math.max(full.scale, Math.min(w / (u1 - u0), h / (v1 - v0)) * 0.96);
    const uc = (u0 + u1) / 2, vc = (v0 + v1) / 2;
    wanted = { cx: uc * c + vc * sn, cy: -uc * sn + vc * c, scale };
  }
  // Hold the current framing while it still works: everyone fighting is well inside it, the
  // action has not shifted by a quarter view and there is no big zoom-in to gain.
  const hold = state.camTarget;
  let keep = false;
  if (hold) {
    const [hu, hv] = toUV([hold.cx, hold.cy]), halfW = w / 2 / hold.scale, halfH = h / 2 / hold.scale;
    const inView = p => { const [pu, pv] = toUV(p.pos); return Math.abs(pu - hu) < halfW * 0.8 && Math.abs(pv - hv) < halfH * 0.8; };
    const [wu, wv] = toUV([wanted.cx, wanted.cy]);
    const shifted = Math.abs(wu - hu) > halfW * 0.5 || Math.abs(wv - hv) > halfH * 0.5;
    const fightInside = fight.every(inView);
    keep = framed.length > 0 && framed.every(inView) && !shifted && wanted.scale < hold.scale * 1.4;
    // Only a straggler is out of view: bring them in, but slowly.
    wanted.slow = fightInside && !shifted;
  }
  if (!keep) state.camTarget = wanted;
  const target = state.camTarget;
  // A critically damped spring keeps the camera's speed continuous; zooming out (someone is
  // about to leave the frame) is quicker than zooming in.
  const cam = (state.cam ??= { vx: 0, vy: 0, vz: 0 }), pan = target.slow ? 3.5 : 1.6;
  const focusCy = v.cy - viewArea().top / 2 / v.scale;
  let fy;
  [v.cx, cam.vx] = smoothDamp(v.cx, target.cx, cam.vx, pan, dt);
  [fy, cam.vy] = smoothDamp(focusCy, target.cy, cam.vy, pan, dt);
  let z = Math.log(v.scale);
  const zoomOut = target.slow ? 3.5 : 1.4;
  [z, cam.vz] = smoothDamp(z, Math.log(target.scale), cam.vz, target.scale < v.scale ? zoomOut : 3, dt);
  v.scale = Math.exp(z);
  v.cy = viewCy(fy, v.scale);
}

// Unity-style SmoothDamp: moves `x` toward `to` with velocity `vel`, settling in about `time` s.
function smoothDamp(x, to, vel, time, dt) {
  const w = 2 / time, e = w * dt, decay = 1 / (1 + e + 0.48 * e * e + 0.235 * e * e * e);
  const change = x - to, temp = (vel + w * change) * dt;
  const v = (vel - w * temp) * decay;
  return [to + (change + temp) * decay, v];
}
