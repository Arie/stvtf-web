import { $, OVERLAY, OPT, overlayUnit, canvas, uiScale, ctx, state, canvasSize } from "./core.js";

// ---------- map backgrounds
let boundaries = null;
async function demostfBoundaries() {
  if (!boundaries) {
    try { boundaries = await (await fetch("overview/boundaries.json")).json(); }
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
    img.src = `overview/${alias}.webp`;
  }
  $("bgtoggle").hidden = true;
  try {
    const g = await (await fetch(`map/${encodeURIComponent(map)}`)).json();
    if (state.map !== map) return;
    state.geometry = g;
    state.spawns = g.spawns || null;
    state.geoCanvas = renderGeometry(g);
    if (!state.fitted && g.bounds) fitTo({ x: g.bounds[0][0], y: g.bounds[0][1] }, { x: g.bounds[1][0], y: g.bounds[1][1] });
  } catch (e) { console.warn("no geometry", e); }
  if (state.map === map) state.mapData = true;
}

function renderGeometry(g) {
  const [lo, hi] = g.bounds;
  // World units per pixel; huge maps (MGE) get coarser so the canvas stays within browser limits.
  const res = Math.max(4, Math.max(hi[0] - lo[0], hi[1] - lo[1]) / 4096);
  const w = Math.ceil((hi[0] - lo[0]) / res), h = Math.ceil((hi[1] - lo[1]) / res);
  const oc = document.createElement("canvas"); oc.width = w; oc.height = h;
  const c = oc.getContext("2d");
  const zs = g.floors.map(f => f.z);
  const zlo = zs.length ? zs[Math.floor(zs.length * 0.02)] : 0, zhi = zs.length ? zs[Math.floor(zs.length * 0.98)] : 1;
  // Roofs hide what is under them; keep floors up to a little above the highest pickup/point level.
  const cut = hi[2] - 1536 + 400;
  const X = x => (x - lo[0]) / res, Y = y => (hi[1] - y) / res;
  for (const f of g.floors) {
    if (f.z > cut) continue;
    const t = Math.max(0, Math.min(1, (f.z - zlo) / (zhi - zlo || 1)));
    const l = 16 + t * 30;
    c.fillStyle = `hsl(210 8% ${l}%)`;
    c.beginPath();
    for (let i = 0; i < f.p.length; i += 2) (i ? c.lineTo : c.moveTo).call(c, X(f.p[i]), Y(f.p[i + 1]));
    c.closePath(); c.fill();
  }
  c.strokeStyle = "#0b0c0e"; c.lineWidth = 1.2;
  c.beginPath();
  for (const wl of g.walls) {
    if (wl.z - 64 > cut) continue;
    c.moveTo(X(wl.p[0]), Y(wl.p[1])); c.lineTo(X(wl.p[2]), Y(wl.p[3]));
  }
  c.stroke();
  return { canvas: oc, lo, hi };
}

// ---------- view
// Whether the main view shows the player HUD or the scoreboard (h and s hide them).
const shown = what => !OVERLAY && !document.documentElement.classList.contains("nohud") &&
  !(what === "score" && document.documentElement.classList.contains("noscore"));

function fitTo(min, max) {
  const r = canvas.getBoundingClientRect(), v = state.view;
  v.cx = (min.x + max.x) / 2; v.cy = (min.y + max.y) / 2;
  // The box's extent once rotated into the view.
  const c = Math.abs(Math.cos(v.rot)), s = Math.abs(Math.sin(v.rot));
  const w = max.x - min.x, h = max.y - min.y, rw = w * c + h * s, rh = w * s + h * c;
  // Keep the map clear of the HUD columns on both sides.
  const hud = shown("hud") && r.width > 800 ? 2 * 268 * uiScale() : 0;
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
  const side = shown("hud") && r.width > 800 ? 268 * uiScale() : 0, top = shown("score") ? 140 * uiScale() : 0;
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
  const v = state.view, pad = 600, { w, h } = viewArea();
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
    const d = g.getImageData(0, 0, c.width, c.height), px = d.data;
    for (let i = 0; i < px.length; i += 4) {
      const lum = Math.max(px[i], px[i + 1], px[i + 2]);
      if (lum < 14) px[i + 3] = 0; else if (lum < 28) px[i + 3] = Math.round(255 * (lum - 14) / 14);
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
