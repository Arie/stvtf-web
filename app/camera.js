import { OVERLAY, state } from "./core.js";
import { viewArea, viewCy, playerRad } from "./view.js";
/** @typedef {import("../types/stream").Player} Player */

// Auto zoom frames the fight: clumps of players first. A player on their own joins the
// framing only after STRAGGLE_MS without closing in on the fight by CLOSING (one walking back
// from spawn is on the way and left out) and is then brought in slowly. The camera holds its framing until the fight leaves it, tightens a lot, or stays
// somewhat tighter for SETTLE_MS, and glides: never wider than the full framing, never
// tighter than MIN_SPAN units (less in the overlay). Markers have a minimum size in pixels, so
// the framing keeps them clear of the edge with a margin of one and a half markers on screen
// besides a small padding in map units.
const MIN_SPAN = OVERLAY ? 1600 : 2600,
  ACTION_PAD = 150,
  CLUMP_LINK = 1500,
  STRAGGLE_MS = 3000,
  CLOSING = 300,
  SETTLE_MS = 500;
const straggling = new Map();
// When each player last came alive, to tell a wave fresh out of spawn (for FRESH_MS) from the fight.
const FRESH_MS = 8000,
  spawnedAt = new Map(),
  wasAlive = new Map();
let lastFrame = performance.now(),
  tighterSince = null;

// Players linked within CLUMP_LINK of each other form a clump; keep the biggest one and any
// other clump of three or more. Players fresh from spawn come out together, so they do not count
// towards a clump's size (unless everyone is fresh): a wave walking out is not the fight.
/** @param {{ pos: number[] }[]} alive @param {(p: { pos: number[] }) => boolean} [fresh] */
export function fightingPlayers(alive, fresh = () => false) {
  const group = alive.map((_, i) => i);
  const root = i => (group[i] === i ? i : (group[i] = root(group[i])));
  for (let i = 0; i < alive.length; i++)
    for (let j = i + 1; j < alive.length; j++)
      if (Math.hypot(alive[i].pos[0] - alive[j].pos[0], alive[i].pos[1] - alive[j].pos[1]) < CLUMP_LINK)
        group[root(i)] = root(j);
  const counted = alive.every(fresh) ? () => true : p => !fresh(p);
  const sizes = new Map();
  alive.forEach((p, i) => sizes.set(root(i), (sizes.get(root(i)) || 0) + (counted(p) ? 1 : 0)));
  const biggest = Math.max(...sizes.values());
  return alive.filter((_, i) => sizes.get(root(i)) > 0 && (sizes.get(root(i)) === biggest || sizes.get(root(i)) >= 3));
}

// Whether a loner who was at `from` and is now at `to` has come `by` closer to the fight. Both are
// measured against where the fight is now, so the fight coming to them does not count.
export function closingIn(from, to, fight, by) {
  const gap = q => Math.min(...fight.map(f => Math.hypot(q[0] - f.pos[0], q[1] - f.pos[1])));
  return gap(to) < gap(from) - by;
}

// Someone seen dead and now alive has respawned; those alive when first seen are not fresh.
/** @param {Player[]} players @param {number} now */
function noteRespawns(players, now) {
  for (const p of players) {
    if (p.alive && wasAlive.get(p.id) === false) spawnedAt.set(p.id, now);
    wasAlive.set(p.id, Boolean(p.alive));
  }
}

// Who to frame: the fight, and the loners who have been on their own for STRAGGLE_MS.
/** @param {Player[]} players @param {number} now */
function framedPlayers(players, now) {
  const fresh = p => now - (spawnedAt.get(p.id) ?? -Infinity) < FRESH_MS;
  const alive = players.filter(p => p.alive && p.pos && (p.team === "red" || p.team === "blue"));
  const fight = alive.length ? fightingPlayers(alive, fresh) : [];
  const loners = alive.filter(p => !fight.includes(p));
  for (const id of straggling.keys()) if (!loners.some(p => p.id === id)) straggling.delete(id);
  // Each loner's wait starts again when they walk CLOSING closer to the fight, until they are
  // framed (then they stay framed, so the camera does not pump).
  for (const p of loners) {
    const s = straggling.get(p.id);
    if (!s) straggling.set(p.id, { since: now, from: p.pos });
    else if (now - s.since <= STRAGGLE_MS && closingIn(s.from, p.pos, fight, CLOSING))
      Object.assign(s, { since: now, from: p.pos });
  }
  return { fight, framed: [...fight, ...loners.filter(p => now - straggling.get(p.id).since > STRAGGLE_MS)] };
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

// Whether to hold the current framing `hold` while it still works: everyone fighting is well
// inside it, the action has not shifted by a quarter view and there is no big zoom-in to gain,
// nor a smaller one that has lasted.
function holding(hold, wanted, { fight, framed }, at) {
  const { now, w, h, toUV } = at;
  if (wanted.scale <= hold.scale * 1.1) tighterSince = null;
  else tighterSince ??= now;
  const [hu, hv] = toUV([hold.cx, hold.cy]),
    halfW = w / 2 / hold.scale,
    halfH = h / 2 / hold.scale;
  const inView = p => {
    const [pu, pv] = toUV(p.pos);
    return Math.abs(pu - hu) < halfW * 0.8 && Math.abs(pv - hv) < halfH * 0.8;
  };
  const [wu, wv] = toUV([wanted.cx, wanted.cy]);
  const shifted = Math.abs(wu - hu) > halfW * 0.5 || Math.abs(wv - hv) > halfH * 0.5;
  const fightInside = fight.every(inView);
  const settled = tighterSince != null && now - tighterSince > SETTLE_MS;
  const keep = framed.length > 0 && framed.every(inView) && !shifted && wanted.scale < hold.scale * 1.4 && !settled;
  // Only a straggler is out of view: bring them in, but slowly.
  wanted.slow = fightInside && !shifted;
  return keep;
}

// Moves the view toward `target`. A critically damped spring keeps the camera's speed
// continuous; zooming out (someone is about to leave the frame) is quicker than zooming in.
function glide(v, target, dt) {
  const cam = (state.cam ??= { vx: 0, vy: 0, vz: 0 }),
    pan = target.slow ? 3.5 : 1.6;
  const focusCy = v.cy - viewArea().top / 2 / v.scale;
  let fy;
  [v.cx, cam.vx] = smoothDamp(v.cx, target.cx, cam.vx, pan, dt);
  [fy, cam.vy] = smoothDamp(focusCy, target.cy, cam.vy, pan, dt);
  let z = Math.log(v.scale);
  const zoomOut = target.slow ? 3.5 : 1.4;
  [z, cam.vz] = smoothDamp(z, Math.log(target.scale), cam.vz, target.scale < v.scale ? zoomOut : 1.6, dt);
  v.scale = Math.exp(z);
  v.cy = viewCy(fy, v.scale);
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
  glide(v, state.camTarget, dt);
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
