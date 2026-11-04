import { OVERLAY, state } from "./core.js";
import { viewArea, viewCentre, focusOf, playerRad } from "./view.js";
/** @typedef {import("../types/stream").Player} Player */

// Auto zoom frames every living player, wherever they are: more of them on screen is better.
// It catches up quickly (ESCAPE_S) when one of them is leaving the view, except who spawned in
// the last FRESH_MS: they are brought in too, but gently. Before the round starts nobody counts
// as fresh, and the view takes in everyone in ROUND_START_S.
// The camera holds its framing until someone in it reaches its margin, it tightens a lot, or
// stays a quarter tighter for SETTLE_MS; it glides, and never zooms in so far that a settled
// player leaves the view. Never wider than the full framing, never tighter than MIN_SPAN units
// (less in the overlay). Markers have a minimum size in pixels, so the framing keeps them clear
// of the edge with a margin of one and a half markers on screen besides a small padding in map
// units.
const MIN_SPAN = OVERLAY ? 1600 : 2600,
  ACTION_PAD = 150,
  ROUND_START_S = 0.4,
  SETTLE_MS = 2000,
  FRESH_MS = 10000,
  ESCAPE_S = 0.5;
// When each player last came alive (a respawn).
const spawnedAt = new Map(),
  wasAlive = new Map();
let lastFrame = performance.now(),
  tighterSince = null;

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
/** @param {Player[]} players @param {number} now */
function framedPlayers(players, now) {
  const alive = players.filter(p => p.alive && p.pos && (p.team === "red" || p.team === "blue"));
  const settled = beforeRound() ? alive : alive.filter(p => !(now - (spawnedAt.get(p.id) ?? -Infinity) < FRESH_MS));
  return { settled, framed: alive };
}

// The framing round the framed players, or the full one when there are none. Along the rotated
// axes, like the framing, so the box matches the panel.
function wantedFraming(framed, full, at) {
  if (!framed.length) return { ...full };
  const { w, h, c, sn, toUV } = at;
  const uv = framed.map(p => toUV(p.pos));
  let u0 = Math.min(...uv.map(p => p[0])) - ACTION_PAD,
    u1 = Math.max(...uv.map(p => p[0])) + ACTION_PAD;
  let v0 = Math.min(...uv.map(p => p[1])) - ACTION_PAD,
    v1 = Math.max(...uv.map(p => p[1])) + ACTION_PAD;
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
function holding(hold, wanted, { settled, framed }, at) {
  const { now, w, h, toUV } = at;
  if (wanted.scale <= hold.scale * 1.25) tighterSince = null;
  else tighterSince ??= now;
  const [hu, hv] = toUV([hold.cx, hold.cy]),
    halfW = w / 2 / hold.scale,
    halfH = h / 2 / hold.scale;
  const inView = p => inside(p, hold, at);
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
function springs(target, escaping) {
  if (beforeRound()) return SPRINGS.roundStart;
  if (escaping) return SPRINGS.escaping;
  return target.slow ? SPRINGS.slow : SPRINGS.normal;
}
// How far in the view at `focus` can zoom with everyone in `keep` still inside its margin.
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

function glide(v, target, dt, { escaping, keep, at }) {
  const cam = (state.cam ??= { vx: 0, vy: 0, vz: 0 }),
    t = springs(target, escaping),
    pan = t.pan;
  // The framing point glides, and the view is put back round it at the new zoom.
  const focus = focusOf(v);
  let fx, fy;
  [fx, cam.vx] = smoothDamp(focus.cx, target.cx, cam.vx, pan, dt);
  [fy, cam.vy] = smoothDamp(focus.cy, target.cy, cam.vy, pan, dt);
  let z = Math.log(v.scale);
  [z, cam.vz] = smoothDamp(z, Math.log(target.scale), cam.vz, target.scale < v.scale ? t.out : t.in, dt);
  // Zooming in while still panning would cut off the far side: wait for the pan.
  const limit = Math.max(v.scale, deepestZoom(keep, { cx: fx, cy: fy }, at));
  if (Math.exp(z) > limit) {
    z = Math.log(limit);
    cam.vz = 0;
  }
  v.scale = Math.exp(z);
  Object.assign(v, viewCentre({ cx: fx, cy: fy }, v.scale));
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
  glide(v, state.camTarget, dt, { escaping: who.settled.some(p => !inside(p, seen, at)), keep: who.settled, at });
}

// Unity-style SmoothDamp: moves `x` toward `to` with velocity `vel`, settling in about `time` s.
function smoothDamp(x, to, vel, time, dt) {
  const w = 2 / time,
    e = w * dt,
    decay = 1 / (1 + e + 0.48 * e * e + 0.235 * e * e * e);
  const change = x - to,
    temp = (vel + w * change) * dt;
  const v = (vel - w * temp) * decay;
  return [to + (change + temp) * decay, v];
}
