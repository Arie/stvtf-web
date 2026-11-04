import { OVERLAY, state } from "./core.js";
import { viewArea, viewCentre, focusOf, playerRad } from "./view.js";
/** @typedef {import("../types/stream").Player} Player */

// Auto zoom frames the fight, wherever it is, so two fights apart all stay in view: everyone who
// fought in the last ENGAGE_MS (fired, hurt someone or was hurt), and anyone behind enemy lines,
// such as a flanker before their first shot. When there is neither (setup, a reset), the biggest
// clump of players is the fight.
// Anyone else joins the framing only after STRAGGLE_MS without closing in on the fight by CLOSING
// (a wave walking out of spawn is on the way and left out) and is then brought in slowly.
// The camera holds its framing until someone in it reaches its margin, it tightens a lot, or
// stays somewhat tighter for SETTLE_MS; it glides, and catches up quickly (ESCAPE_S) when
// someone in the fight is leaving the view. Never wider than the full framing, never tighter
// than MIN_SPAN units (less in the overlay). Markers have a minimum size in pixels, so the
// framing keeps them clear of the edge with a margin of one and a half markers on screen
// besides a small padding in map units.
const MIN_SPAN = OVERLAY ? 1600 : 2600,
  ACTION_PAD = 150,
  CLUMP_LINK = 1500,
  ENGAGE_MS = 4000,
  STRAGGLE_MS = 3000,
  CLOSING = 300,
  SETTLE_MS = 500,
  ESCAPE_S = 0.5;
const straggling = new Map();
// When each player last fought, and the projectiles already seen being fired.
const foughtAt = new Map(),
  projectilesSeen = new Set();
let lastFrame = performance.now(),
  tighterSince = null;

// Players linked within CLUMP_LINK of each other form a clump; keep the biggest one and any
// other clump of three or more.
/** @param {{ pos: number[] }[]} alive */
export function fightingPlayers(alive) {
  const group = alive.map((_, i) => i);
  const root = i => (group[i] === i ? i : (group[i] = root(group[i])));
  for (let i = 0; i < alive.length; i++)
    for (let j = i + 1; j < alive.length; j++)
      if (Math.hypot(alive[i].pos[0] - alive[j].pos[0], alive[i].pos[1] - alive[j].pos[1]) < CLUMP_LINK)
        group[root(i)] = root(j);
  const sizes = new Map();
  alive.forEach((_, i) => sizes.set(root(i), (sizes.get(root(i)) || 0) + 1));
  const biggest = Math.max(...sizes.values());
  return alive.filter((_, i) => sizes.get(root(i)) === biggest || sizes.get(root(i)) >= 3);
}

// Who an event shows fighting: shooting, airblasting, hurting or being hurt (a sentry's builder too).
const FIGHTERS = {
  shot: ["player"],
  airblast: ["player"],
  damage: ["attacker", "victim"],
  sentry_shot: ["builder", "target"],
};
/** @param {import("../types/stream").GameEvent} ev @param {number} now */
export function noteCombat(ev, now) {
  if (!Object.hasOwn(FIGHTERS, ev.type)) return;
  for (const k of FIGHTERS[ev.type]) if (ev[k] != null) foughtAt.set(ev[k], now);
}
// Projectiles fired since the last frame count as fighting too (rockets, pipes, arrows …).
function noteFired(now) {
  const flying = state.snaps.at(-1)?.projectiles || [];
  for (const pr of flying)
    if (!projectilesSeen.has(pr.id)) {
      projectilesSeen.add(pr.id);
      if (pr.owner != null) foughtAt.set(pr.owner, now);
    }
  if (projectilesSeen.size > 4096) projectilesSeen.clear();
}

// The players behind enemy lines: past more than half of the other team, along the line from
// their own team's middle to the other's.
/** @template {{ pos: number[], team: string }} P @param {P[]} alive @returns {P[]} */
export function behindLines(alive) {
  const middle = team => {
    const t = alive.filter(p => p.team === team);
    return t.length
      ? [t.reduce((a, p) => a + p.pos[0], 0) / t.length, t.reduce((a, p) => a + p.pos[1], 0) / t.length]
      : null;
  };
  const red = middle("red"),
    blue = middle("blue");
  if (!red || !blue) return [];
  return alive.filter(p => {
    const [from, to] = p.team === "red" ? [red, blue] : [blue, red];
    const along = q => (q.pos[0] - from[0]) * (to[0] - from[0]) + (q.pos[1] - from[1]) * (to[1] - from[1]);
    const enemies = alive.filter(q => q.team !== p.team),
      mine = along(p);
    return enemies.filter(q => along(q) < mine).length > enemies.length / 2;
  });
}

// Whether a loner who was at `from` and is now at `to` has come `by` closer to the fight. Both are
// measured against where the fight is now, so the fight coming to them does not count.
export function closingIn(from, to, fight, by) {
  const gap = q => Math.min(...fight.map(f => Math.hypot(q[0] - f.pos[0], q[1] - f.pos[1])));
  return gap(to) < gap(from) - by;
}

// The fight: who fought in the last ENGAGE_MS or is behind enemy lines. A death ends it: whoever
// respawns has not fought yet.
/** @param {Player[]} alive @param {number} now */
function engaged(alive, now) {
  noteFired(now);
  for (const id of foughtAt.keys()) if (!alive.some(p => p.id === id)) foughtAt.delete(id);
  const behind = behindLines(alive);
  const fight = alive.filter(p => now - (foughtAt.get(p.id) ?? -Infinity) < ENGAGE_MS || behind.includes(p));
  return fight.length || !alive.length ? fight : fightingPlayers(alive);
}

// Who to frame: the fight, and the loners who have been on their own for STRAGGLE_MS.
/** @param {Player[]} players @param {number} now */
function framedPlayers(players, now) {
  const alive = players.filter(p => p.alive && p.pos && (p.team === "red" || p.team === "blue"));
  const fight = engaged(alive, now);
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
function holding(hold, wanted, { fight, framed }, at) {
  const { now, w, h, toUV } = at;
  if (wanted.scale <= hold.scale * 1.1) tighterSince = null;
  else tighterSince ??= now;
  const [hu, hv] = toUV([hold.cx, hold.cy]),
    halfW = w / 2 / hold.scale,
    halfH = h / 2 / hold.scale;
  const inView = p => inside(p, hold, at);
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
// continuous; zooming out (someone is about to leave the frame) is quicker than zooming in,
// and someone in the fight leaving the view is caught up with in ESCAPE_S.
// How long the springs take to settle (s): panning, zooming out and zooming in.
const SPRINGS = {
  escaping: { pan: ESCAPE_S, out: ESCAPE_S, in: 1.6 },
  slow: { pan: 3.5, out: 3.5, in: 1.6 },
  normal: { pan: 1.6, out: 1.4, in: 1.6 },
};
function springs(target, escaping) {
  if (escaping) return SPRINGS.escaping;
  return target.slow ? SPRINGS.slow : SPRINGS.normal;
}
function glide(v, target, dt, escaping) {
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
  const who = framedPlayers(players, now);
  const wanted = wantedFraming(who.framed, full, at);
  const hold = state.camTarget;
  if (!hold || !holding(hold, wanted, who, at)) {
    state.camTarget = wanted;
    tighterSince = null;
  }
  // The view as it is now, centred where the framing is (the free area's centre).
  const seen = { ...focusOf(v), scale: v.scale };
  glide(
    v,
    state.camTarget,
    dt,
    who.fight.some(p => !inside(p, seen, at)),
  );
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
