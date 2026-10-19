import { OVERLAY, canvas, ctx, state, renderTick, canvasSize } from "./core.js";
import { fireEvents } from "./events.js";
import { updatePanel, renderScoreboard } from "./hud.js";
import { layerOn, renderSettings } from "./layers.js";
import { drawObjectives, drawCapAreas, drawOnPoint, drawFlags, drawBuildings, drawSpawnRooms } from "./objectives.js";
import { drawMarks, drawPacks, drawJumpTrails } from "./overkill.js";
import { drawProjectiles, drawHealBeams, drawPlayers, drawEffects } from "./players.js";
import { openStream } from "./stream.js";
import { frameMap, drawBackground, followAction } from "./view.js";

// ---------- interpolation
function lerpAngle(a, b, t) { let d = ((b - a + 540) % 360) - 180; return a + d * t; }
function frameAt(tick) {
  const s = state.snaps;
  if (!s.length) return null;
  let i = s.findIndex(x => x.tick > tick);
  if (i === -1) { const l = s[s.length - 1]; return { snap: l, players: l.players, projectiles: l.projectiles }; }
  if (i === 0) return { snap: s[0], players: s[0].players, projectiles: s[0].projectiles };
  const a = s[i - 1], b = s[i], t = (tick - a.tick) / (b.tick - a.tick);
  const prev = new Map(a.players.map(p => [p.id, p]));
  const players = b.players.map(p => {
    const q = prev.get(p.id);
    if (!q || !q.alive || !p.alive) return t < 0.5 && q ? q : p;
    return { ...p,
      pos: [0, 1, 2].map(k => q.pos[k] + (p.pos[k] - q.pos[k]) * t),
      yaw: lerpAngle(q.yaw, p.yaw, t), hp: t < 0.5 ? q.hp : p.hp };
  });
  // Projectiles: lerp the ones in both snapshots, keep their heading from the movement;
  // new ones appear where they were first seen.
  const before = new Map(a.projectiles.map(p => [p.id, p]));
  const projectiles = b.projectiles.map(p => {
    const q = before.get(p.id);
    if (!q) return p;
    const dx = p.pos[0] - q.pos[0], dy = p.pos[1] - q.pos[1];
    return { ...p, pos: [0, 1, 2].map(k => q.pos[k] + (p.pos[k] - q.pos[k]) * t),
      heading: Math.hypot(dx, dy) > 4 ? Math.atan2(dy, dx) * 180 / Math.PI : null };
  });
  return { snap: t < 0.5 ? a : b, players, projectiles };
}

// At most 60 fps: the data comes 20 times a second, and a 120 or 240 Hz screen would otherwise
// redraw the whole map 2 to 4 times as often for nothing.
// A frame is drawn when the next 60 fps slot is nearer than the refresh after it.
const FRAME_MS = 1000 / 60;
let lastDraw = -Infinity, lastRefresh = 0, refreshMs = FRAME_MS;
function frame(now) {
  const gap = now - lastRefresh;
  lastRefresh = now;
  if (gap > 0 && gap < 50) refreshMs += (gap - refreshMs) * 0.1;
  if (now - lastDraw + refreshMs / 2 >= FRAME_MS) {
    lastDraw = now;
    try { draw(); } catch (e) { console.error(e); }
  }
  requestAnimationFrame(frame);
}

function draw() {
  const r = canvasSize(), dpr = window.devicePixelRatio || 1;
  if (canvas.width !== Math.round(r.width * dpr) || canvas.height !== Math.round(r.height * dpr)) {
    canvas.width = Math.round(r.width * dpr); canvas.height = Math.round(r.height * dpr);
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  // Round joins: outlined text with sharp (miter) joins grows spikes off pointed glyphs like "y".
  ctx.lineJoin = "round";
  const tick = renderTick();
  const f = frameAt(tick);
  if (f) {
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
  }
  drawBackground(r);
  if (f) {
    const byId = new Map(f.players.map(p => [p.id, p]));
    fireEvents(tick, byId);
    drawSpawnRooms(f.snap);
    if (layerOn("marks")) drawMarks();
    // Under the points and buildings, which some maps put right on top of a pack.
    if (layerOn("pickups")) drawPacks();
    if (layerOn("caps")) drawCapAreas(f.snap);
    drawObjectives(f.snap, tick);
    if (layerOn("points") && !OVERLAY) drawOnPoint(f.snap);
    drawBuildings(f.snap);
    if (layerOn("jumps")) drawJumpTrails(f.players);
    // Above the players in the overlay, where a dot would otherwise cover them.
    if (!OVERLAY) drawProjectiles(f.projectiles);
    drawEffects(byId);
    drawHealBeams(f.players, byId);
    drawPlayers(f.players);
    drawFlags(f.snap, f.players, tick);
    if (OVERLAY) drawProjectiles(f.projectiles);
    updatePanel(f.players, f.snap);
  }
}

renderSettings();
openStream();

requestAnimationFrame(frame);
