import { drawBuildings } from "./buildings.js";
import { followAction } from "./camera.js";
import { OVERLAY, canvas, ctx, state, renderTick, canvasSize, optOn } from "./core.js";
import { EMBEDDED, embedState } from "./embed.js";
import { fireEvents } from "./events.js";
import { updatePanel, renderScoreboard, shownScore } from "./hud.js";
import { layerOn, renderSettings } from "./layers.js";
import { drawObjectives } from "./objectives.js";
import { drawCapAreas } from "./points.js";
import { drawFlags } from "./flags.js";
import { drawMarks, drawPacks, drawJumpTrails } from "./overkill.js";
import { drawHealBeams, drawPlayers } from "./players.js";
import { drawProjectiles } from "./projectiles.js";
import { drawEffects } from "./effects.js";
import { initRoom } from "./room.js";
import { drawSpawnRooms } from "./spawns.js";
import { openStream } from "./stream.js";
import { frameMap, drawBackground } from "./view.js";

const SHOW_BUILDINGS = optOn("buildings", !OVERLAY),
  SHOW_PROJECTILES = optOn("projectiles", !OVERLAY),
  SHOW_BEAMS = optOn("beams");

// ---------- interpolation
/** @param {number} a @param {number} b @param {number} t */
function lerpAngle(a, b, t) {
  const d = ((b - a + 540) % 360) - 180;
  return a + d * t;
}
/** @param {number[]} a @param {number[]} b @param {number} t @returns {import("../types/stream").Vec3} */
function lerpPos(a, b, t) {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
}
/** @param {number} tick */
function frameAt(tick) {
  const s = state.snaps;
  if (!s.length) return null;
  const i = s.findIndex(x => x.tick > tick);
  if (i === -1) {
    const l = s[s.length - 1];
    return { snap: l, players: l.players, projectiles: l.projectiles };
  }
  if (i === 0) return { snap: s[0], players: s[0].players, projectiles: s[0].projectiles };
  const a = s[i - 1],
    b = s[i],
    t = (tick - a.tick) / (b.tick - a.tick);
  const prev = new Map(a.players.map(p => [p.id, p]));
  const players = b.players.map(p => {
    const q = prev.get(p.id);
    if (!q || !q.alive || !p.alive) return t < 0.5 && q ? q : p;
    return {
      ...p,
      pos: lerpPos(q.pos, p.pos, t),
      yaw: lerpAngle(q.yaw ?? 0, p.yaw ?? 0, t),
      hp: t < 0.5 ? q.hp : p.hp,
    };
  });
  // Projectiles: lerp the ones in both snapshots, keep their heading from the movement;
  // new ones appear where they were first seen.
  const before = new Map(a.projectiles.map(p => [p.id, p]));
  const projectiles = b.projectiles.map(p => {
    const q = before.get(p.id);
    if (!q) return p;
    const dx = p.pos[0] - q.pos[0],
      dy = p.pos[1] - q.pos[1];
    return {
      ...p,
      pos: lerpPos(q.pos, p.pos, t),
      heading: Math.hypot(dx, dy) > MIN_HEADING_STEP ? (Math.atan2(dy, dx) * 180) / Math.PI : null,
    };
  });
  return { snap: t < 0.5 ? a : b, players, projectiles };
}
/** @typedef {NonNullable<ReturnType<typeof frameAt>>} Frame A snapshot with its players and projectiles moved to a tick. */

// At most 60 fps: the data comes about 17 times a second, and a 120 or 240 Hz screen would otherwise
// redraw the whole map 2 to 4 times as often for nothing.
// A frame is drawn when the next 60 fps slot is nearer than the refresh after it.
const FRAME_MS = 1000 / 60,
  MAX_REFRESH_MS = 50,
  REFRESH_SMOOTHING = 0.1;
// A projectile faces where it moved, once it moved this many units between snapshots.
const MIN_HEADING_STEP = 4;
let lastDraw = -Infinity,
  lastRefresh = 0,
  refreshMs = FRAME_MS;
/** @param {number} now */
function frame(now) {
  const gap = now - lastRefresh;
  lastRefresh = now;
  // The screen's refresh interval, smoothed; a gap this long is a stall, not a refresh.
  if (gap > 0 && gap < MAX_REFRESH_MS) refreshMs += (gap - refreshMs) * REFRESH_SMOOTHING;
  if (now - lastDraw + refreshMs / 2 >= FRAME_MS) {
    lastDraw = now;
    try {
      draw();
    } catch (e) {
      console.error(e);
    }
  }
  requestAnimationFrame(frame);
}

function draw() {
  const r = canvasSize(),
    dpr = window.devicePixelRatio || 1;
  if (canvas.width !== Math.round(r.width * dpr) || canvas.height !== Math.round(r.height * dpr)) {
    canvas.width = Math.round(r.width * dpr);
    canvas.height = Math.round(r.height * dpr);
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  // Round joins: outlined text with sharp (miter) joins grows spikes off pointed glyphs like "y".
  ctx.lineJoin = "round";
  const tick = renderTick();
  const f = frameAt(tick);
  if (f) aim(f, tick);
  drawBackground(r);
  if (f) drawFrame(f, tick);
}
// Before the background: frame the map, follow the action or a player, and show the score.
/** @param {Frame} f @param {number} tick */
function aim(f, tick) {
  // Frame once the map's spawn points are in (or known to be missing).
  if (!state.framed && state.mapData) frameMap();
  if (state.framed && state.autoZoom && state.follow == null) followAction(f.players);
  state.lastPlayers = f.players;
  // Dead players are reported where they died (no position if before we connected).
  const target = state.follow != null && f.players.find(p => p.id === state.follow);
  if (target && target.pos) {
    state.view.cx += (target.pos[0] - state.view.cx) * 0.18;
    state.view.cy += (target.pos[1] - state.view.cy) * 0.18;
  }
  renderScoreboard(f.snap, tick);
  embedState(shownScore(f.snap) ?? null, f.players);
}
// Everything on the map, bottom up, over its background.
/** @param {Frame} f @param {number} tick */
function drawFrame(f, tick) {
  const byId = new Map(f.players.map(p => [p.id, p]));
  fireEvents(tick, byId);
  drawSpawnRooms(f.snap);
  if (layerOn("marks")) drawMarks();
  // Under the points and buildings, which some maps put right on top of a pack.
  if (layerOn("pickups")) drawPacks();
  if (layerOn("caps")) drawCapAreas(f.snap);
  drawObjectives(f.snap, tick);
  if (SHOW_BUILDINGS) drawBuildings(f.snap);
  if (layerOn("jumps")) drawJumpTrails(f.players);
  // Above the players in the overlay, where a dot would otherwise cover them.
  if (SHOW_PROJECTILES && !OVERLAY) drawProjectiles(f.projectiles);
  drawEffects(byId);
  if (SHOW_BEAMS) drawHealBeams(f.players, byId);
  drawPlayers(f.players);
  drawFlags(f.snap, f.players, tick);
  if (SHOW_PROJECTILES && OVERLAY) drawProjectiles(f.projectiles);
  updatePanel(f.players, f.snap);
}

// Browsers stop animation frames in an iframe scrolled out of view: playback, and with it the
// parent page's messages, then goes on without drawing.
const UNDRAWN_MS = 250;
function advance() {
  if (performance.now() - lastDraw < UNDRAWN_MS) return;
  const tick = renderTick(),
    f = frameAt(tick);
  if (!f) return;
  state.lastPlayers = f.players;
  fireEvents(tick, new Map(f.players.map(p => [p.id, p])));
  const now = performance.now();
  state.effects = state.effects.filter(e => now - e.t0 < e.life);
  embedState(shownScore(f.snap) ?? null, f.players);
}

renderSettings();
initRoom();
openStream();

requestAnimationFrame(frame);
if (EMBEDDED)
  setInterval(() => {
    try {
      advance();
    } catch (e) {
      console.error(e);
    }
  }, UNDRAWN_MS);
