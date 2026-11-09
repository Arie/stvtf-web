import { OVERLAY, state } from "../core/core.js";
import { viewArea, viewCentre, focusOf, playerRad } from "./view.js";
/** @typedef {import("../../types/stream").Player} Player */
/** @typedef {import("../../types/stream").LivePlayer} LivePlayer */
/** @typedef {import("../../types/page").Framing} Framing */
/** @typedef {import("../../types/page").View} View */
// A frame's shared measures: when it is, the free area's size, the view's rotation (cos and sin)
// and a world point along the rotated axes.
/** @typedef {{ now: number, w: number, h: number, c: number, sn: number, toUV: (p: number[]) => [number, number] }} At */
/** @typedef {{ settled: LivePlayer[], framed: LivePlayer[] }} Framed */

// Auto zoom frames every living player, wherever they are: more of them on screen is better.
// It catches up quickly (ESCAPE_S) when one of them is leaving the view, except who spawned in
// the last FRESH_MS: they are brought in too, but gently. Before the round starts nobody counts
// as fresh, and the view takes in everyone in ROUND_START_S.
// The camera holds its framing until someone in it reaches its margin, it tightens a lot, or
// stays a quarter tighter for SETTLE_MS; it glides, with some weight (its aim glides toward a new
// framing in AIM_S first, so it never sets off abruptly), and never zooms in so far that a settled
// player leaves the view. Never wider than the full framing, never tighter than MIN_SPAN units
// (less in the overlay). Markers have a minimum size in pixels, so the framing keeps them clear
// of the edge with a margin of one and a half markers on screen besides a small padding in map
// units.
const MIN_SPAN = OVERLAY ? 1600 : 2600,
  ACTION_PAD = 300,
  ROUND_START_S = 0.4,
  SETTLE_MS = 2000,
  FRESH_MS = 10000,
  ESCAPE_S = 0.5,
  AIM_S = 0.3;
// When each player last came alive (a respawn).
/** @type {Map<number, number>} */
const spawnedAt = new Map(),
  /** @type {Map<number, boolean>} */
  wasAlive = new Map();
let lastFrame = performance.now();
/** @type {number | null} */
let tighterSince = null;

// Whether the round is waiting to start, everyone in their spawn.
const beforeRound = () => state.snaps.at(-1)?.round === "preround";

// Someone seen dead and now alive has respawned; those alive when first seen are not fresh.
/** @param {Player[]} players @param {number} now */
function noteRespawns(players, now) {
  for (const p of players) {
    if (p.alive && wasAlive.get(p.id) === false) spawnedAt.set(p.id, now);
    wasAlive.set(p.id, Boolean(p.alive));
  }
}

// Who to frame: everyone alive; `settled` are those who have been alive a while (or everyone,
// before the round), whom the camera hurries to keep in view.
/** @param {Player[]} players @param {number} now @returns {Framed} */
function framedPlayers(players, now) {
  const alive = players.filter(
    /** @returns {p is LivePlayer} */ p => p.alive && p.pos && (p.team === "red" || p.team === "blue"),
  );
  const settled = beforeRound() ? alive : alive.filter(p => !(now - (spawnedAt.get(p.id) ?? -Infinity) < FRESH_MS));
  return { settled, framed: alive };
}

// The framing round the framed players, or the full one when there are none. Along the rotated
// axes, like the framing, so the box matches the panel.
/** @param {LivePlayer[]} framed @param {Framing} full @param {At} at @returns {Framing} */
function wantedFraming(framed, full, at) {
  if (!framed.length) return { ...full };
  const { w, h, c, sn, toUV } = at;
  const uv = framed.map(p => toUV(p.pos));
  let u0 = Math.min(...uv.map(p => p[0])) - ACTION_PAD,
    u1 = Math.max(...uv.map(p => p[0])) + ACTION_PAD;
  let v0 = Math.min(...uv.map(p => p[1])) - ACTION_PAD,
    v1 = Math.max(...uv.map(p => p[1])) + ACTION_PAD;
  /** @param {number} a @param {number} b */
  const grow = (a, b) => {
    const m = (a + b) / 2,
      half = Math.max((b - a) / 2, MIN_SPAN / 2);
    return [m - half, m + half];
  };
  [u0, u1] = grow(u0, u1);
  [v0, v1] = grow(v0, v1);
  const m = 1.5 * playerRad();
  const scale = Math.max(full.scale, Math.min((w - 2 * m) / (u1 - u0), (h - 2 * m) / (v1 - v0)));
  const uc = (u0 + u1) / 2,
    vc = (v0 + v1) / 2;
  return { cx: uc * c + vc * sn, cy: -uc * sn + vc * c, scale };
}

// Whether `p` is inside the framing `f` (seen as on the panel), clear of its margin.
/** @param {LivePlayer} p @param {Framing} f @param {At} at */
function inside(p, f, at) {
  const { w, h, toUV } = at,
    m = 1.5 * playerRad();
  const [fu, fv] = toUV([f.cx, f.cy]),
    [pu, pv] = toUV(p.pos);
  return Math.abs(pu - fu) * f.scale < w / 2 - m && Math.abs(pv - fv) * f.scale < h / 2 - m;
}

// Whether to hold the current framing `hold` while it still works: everyone framed is inside
// it, the action has not shifted by a quarter view and there is no big zoom-in to gain,
// nor a smaller one that has lasted.
/** @param {Framing} hold @param {Framing} wanted @param {Framed} who @param {At} at */
function holding(hold, wanted, { settled, framed }, at) {
  const { now, w, h, toUV } = at;
  if (wanted.scale <= hold.scale * 1.25) tighterSince = null;
  else tighterSince ??= now;
  const [hu, hv] = toUV([hold.cx, hold.cy]),
    halfW = w / 2 / hold.scale,
    halfH = h / 2 / hold.scale;
  const inView = /** @param {LivePlayer} p */ p => inside(p, hold, at);
  const [wu, wv] = toUV([wanted.cx, wanted.cy]);
  const shifted = Math.abs(wu - hu) > halfW * 0.5 || Math.abs(wv - hv) > halfH * 0.5;
  const settledInside = settled.every(inView);
  const tightened = tighterSince != null && now - tighterSince > SETTLE_MS;
  const keep = framed.length > 0 && framed.every(inView) && !shifted && wanted.scale < hold.scale * 1.4 && !tightened;
  // While everyone settled is inside, a new framing is only for fresh spawners: bring them in gently.
  wanted.slow = settledInside;
  return keep;
}

// Moves the view toward `target`. A critically damped spring keeps the camera's speed
// continuous; zooming out (someone is about to leave the frame) is quicker than zooming in,
// and a settled player leaving the view is caught up with in ESCAPE_S.
// How long the springs take to settle (s): panning, zooming out and zooming in.
const SPRINGS = {
  roundStart: { pan: ROUND_START_S, out: ROUND_START_S, in: ROUND_START_S },
  escaping: { pan: ESCAPE_S, out: ESCAPE_S, in: 1.6 },
  slow: { pan: 3.5, out: 3.5, in: 1.6 },
  normal: { pan: 1.6, out: 1.4, in: 1.6 },
};
/** @param {Framing} target @param {boolean} escaping */
function springs(target, escaping) {
  if (beforeRound()) return SPRINGS.roundStart;
  if (escaping) return SPRINGS.escaping;
  return target.slow ? SPRINGS.slow : SPRINGS.normal;
}
// How far in the view at `focus` can zoom with everyone in `keep` still inside its margin.
/** @param {LivePlayer[]} keep @param {{ cx: number, cy: number }} focus @param {At} at */
function deepestZoom(keep, focus, at) {
  const { w, h, toUV } = at,
    m = 1.5 * playerRad(),
    [fu, fv] = toUV([focus.cx, focus.cy]);
  let most = Infinity;
  for (const p of keep) {
    const [pu, pv] = toUV(p.pos);
    most = Math.min(most, (w / 2 - m) / Math.abs(pu - fu), (h / 2 - m) / Math.abs(pv - fv));
  }
  return most;
}

/**
 * @param {View} v @param {Framing} target @param {number} dt
 * @param {{ escaping: boolean, keep: LivePlayer[], at: At }} how
 */
function glide(v, target, dt, { escaping, keep, at }) {
  const cam = (state.cam ??= { vx: 0, vy: 0, vz: 0 }),
    t = springs(target, escaping),
    pan = t.pan;
  // The framing point glides, and the view is put back round it at the new zoom.
  const focus = focusOf(v);
  // The aim glides toward the framing, and the camera toward the aim: a new framing starts the
  // camera off gently instead of at once.
  const aim = (cam.aim ??= { cx: focus.cx, cy: focus.cy, z: Math.log(v.scale), vx: 0, vy: 0, vz: 0 });
  [aim.cx, aim.vx] = smoothDamp(aim.cx, target.cx, aim.vx, AIM_S, dt);
  [aim.cy, aim.vy] = smoothDamp(aim.cy, target.cy, aim.vy, AIM_S, dt);
  [aim.z, aim.vz] = smoothDamp(aim.z, Math.log(target.scale), aim.vz, AIM_S, dt);
  let fx, fy;
  [fx, cam.vx] = smoothDamp(focus.cx, aim.cx, cam.vx, pan, dt);
  [fy, cam.vy] = smoothDamp(focus.cy, aim.cy, cam.vy, pan, dt);
  let z = Math.log(v.scale);
  [z, cam.vz] = smoothDamp(z, aim.z, cam.vz, Math.exp(aim.z) < v.scale ? t.out : t.in, dt);
  // Zooming in while still panning would cut off the far side: wait for the pan.
  const limit = Math.max(v.scale, deepestZoom(keep, { cx: fx, cy: fy }, at));
  if (Math.exp(z) > limit) {
    z = Math.log(limit);
    cam.vz = 0;
  }
  v.scale = Math.exp(z);
  Object.assign(v, viewCentre({ cx: fx, cy: fy }, v.scale));
}

// A room's admin's camera, replayed this far behind it so its samples (about every 50 ms) are in
// before they are needed and the view moves between them smoothly. A gap longer than
// MIRROR_STILL_MS was their camera standing still: it starts moving again from there.
const MIRROR_BEHIND_MS = 120,
  MIRROR_STILL_MS = 250,
  MIRROR_KEEP_MS = 2000;
// The share of the way a frame goes toward the first sample, which comes from anywhere.
const MIRROR_EASE = 0.25;

/** @param {{ cx: number; cy: number; w: number; h: number; rot: number }} m */
export function pushMirror(m) {
  const now = performance.now(),
    s = (state.mirror ??= []),
    last = s[s.length - 1];
  if (last && now - last.at > MIRROR_STILL_MS) s.push({ ...last, at: now - MIRROR_STILL_MS / 5 });
  s.push({ at: now, cx: m.cx, cy: m.cy, w: m.w, h: m.h, rot: m.rot });
  while (s.length > 2 && s[1].at < now - MIRROR_KEEP_MS) s.shift();
}

// The view as the admin's was MIRROR_BEHIND_MS ago, fitted to this one.
/** @param {{ at: number; cx: number; cy: number; w: number; h: number; rot: number }[]} s */
export function mirror(s) {
  const v = state.view,
    { w, h } = viewArea(),
    t = performance.now() - MIRROR_BEHIND_MS,
    i = s.findIndex(x => x.at > t);
  /** @type {{ cx: number; cy: number; w: number; h: number; rot: number }} */
  let m,
    ease = 1;
  if (i === -1) m = s[s.length - 1];
  else if (i === 0) [m, ease] = [s[0], MIRROR_EASE];
  else {
    const a = s[i - 1],
      b = s[i],
      k = (t - a.at) / (b.at - a.at);
    m = {
      cx: a.cx + (b.cx - a.cx) * k,
      cy: a.cy + (b.cy - a.cy) * k,
      w: a.w * (b.w / a.w) ** k,
      h: a.h * (b.h / a.h) ** k,
      rot: b.rot,
    };
  }
  if (s.length === 1) ease = MIRROR_EASE;
  const z = Math.log(v.scale),
    focus = focusOf(v);
  v.rot = m.rot;
  v.scale = Math.exp(z + (Math.log(Math.min(w / m.w, h / m.h)) - z) * ease);
  Object.assign(
    v,
    viewCentre({ cx: focus.cx + (m.cx - focus.cx) * ease, cy: focus.cy + (m.cy - focus.cy) * ease }, v.scale),
  );
}

/** @param {Player[]} players */
export function followAction(players) {
  const now = performance.now(),
    dt = Math.min(0.1, (now - lastFrame) / 1000);
  lastFrame = now;
  const v = state.view,
    full = state.full;
  if (!full) return;
  const { w, h } = viewArea(),
    c = Math.cos(v.rot),
    sn = Math.sin(v.rot);
  /** @param {number[]} p @returns {[number, number]} */
  const toUV = p => [p[0] * c - p[1] * sn, p[0] * sn + p[1] * c];
  const at = { now, w, h, c, sn, toUV };
  noteRespawns(players, now);
  const who = framedPlayers(players, now);
  const wanted = wantedFraming(who.framed, full, at);
  const hold = state.camTarget;
  if (!hold || !holding(hold, wanted, who, at)) {
    state.camTarget = wanted;
    tighterSince = null;
  }
  // The view as it is now, centred where the framing is (the free area's centre).
  const seen = { ...focusOf(v), scale: v.scale };
  // state.camTarget is set: kept above, or just replaced.
  glide(v, /** @type {Framing} */ (state.camTarget), dt, {
    escaping: who.settled.some(p => !inside(p, seen, at)),
    keep: who.settled,
    at,
  });
}

// Unity-style SmoothDamp: moves `x` toward `to` with velocity `vel`, settling in about `time` s.
/** @param {number} x @param {number} to @param {number} vel @param {number} time @param {number} dt @returns {[number, number]} */
function smoothDamp(x, to, vel, time, dt) {
  const w = 2 / time,
    e = w * dt,
    decay = 1 / (1 + e + 0.48 * e * e + 0.235 * e * e * e);
  const change = x - to,
    temp = (vel + w * change) * dt;
  const v = (vel - w * temp) * decay;
  return [to + (change + temp) * decay, v];
}
