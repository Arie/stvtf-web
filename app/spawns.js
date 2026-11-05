import { OVERLAY, uiScale, ctx, state, teamColour } from "./core.js";
import { toScreen, drawText } from "./view.js";
/** @typedef {import("../types/stream").GameEvent} GameEvent */
/** @typedef {import("../types/stream").Snapshot} Snapshot */
/** @typedef {import("../types/stream").Team} Team */
/** @typedef {import("../types/stream").Vec3} Vec3 */
/** @typedef {import("../types/page").SpawnRoom} SpawnRoom */

// Spawn rooms: which are in use.
// A spawn point tied to a control point is enabled for its team while that team owns the
// point (TF2's rule); untied ones always are. Of the enabled rooms a team spawns in the most
// forward one: the one farthest from its main spawn (the room with the most spawn points).
// Once a team has been seen spawning since the last change of ownership, that room wins.
let ownersKey = "",
  ownersChanged = 0;
// Spawn points stand up to SPAWN_SLACK above or below their room's box (as in bsp.rs).
const SPAWN_SLACK = 64;
/** @param {SpawnRoom} r @param {Vec3} p */
const inRoom = (r, p) =>
  p[0] >= r.min[0] &&
  p[0] <= r.max[0] &&
  p[1] >= r.min[1] &&
  p[1] <= r.max[1] &&
  p[2] >= r.min[2] - SPAWN_SLACK &&
  p[2] <= r.max[2] + SPAWN_SLACK;
/** @param {SpawnRoom} r */
const centre = r => [(r.min[0] + r.max[0]) / 2, (r.min[1] + r.max[1]) / 2];
/** @param {GameEvent} ev */
export function noteSpawn(ev) {
  const rooms = state.spawns?.rooms || [];
  const i = ev.pos ? rooms.findIndex(r => r.team === ev.team && inRoom(r, ev.pos)) : -1;
  if (i >= 0) state.spawnSeen[ev.team] = { room: i, at: performance.now() };
}
/** @param {Snapshot} snap @returns {Set<number>} */
function activeRooms(snap) {
  const s = state.spawns;
  if (!s) return new Set();
  // Each control point's owner, by its index.
  /** @type {{ [index: number]: Team | number | undefined }} */
  const owner = {};
  for (const o of snap.objectives || [])
    if (o.kind === "control_point") owner[/** @type {number} */ (o.index)] = o.owner;
  const key = JSON.stringify(owner);
  if (key !== ownersKey) {
    ownersKey = key;
    ownersChanged = performance.now();
  }
  const live = s.points.filter(
    p => !p.disabled && (p.point == null || owner[p.point] === undefined || owner[p.point] === p.team),
  );
  /** @type {Set<number>} */
  const active = new Set();
  for (const team of ["red", "blue"]) {
    const seen = state.spawnSeen[team];
    if (seen && seen.at >= ownersChanged) {
      active.add(seen.room);
      continue;
    }
    const rooms = s.rooms
      .map((r, i) => ({ r, i, n: s.points.filter(p => p.team === team && inRoom(r, p.pos)).length }))
      .filter(x => x.r.team === team);
    const main = rooms.reduce(
      (a, b) => (b.n > (a?.n ?? -1) ? b : a),
      /** @type {(typeof rooms)[number] | null} */ (null),
    );
    if (!main) continue;
    const enabled = rooms.filter(x => !x.r.disabled && live.some(p => p.team === team && inRoom(x.r, p.pos)));
    /** @param {(typeof rooms)[number]} x */
    const far = x => Math.hypot(centre(x.r)[0] - centre(main.r)[0], centre(x.r)[1] - centre(main.r)[1]);
    const pick = enabled.reduce(
      (a, b) => (!a || far(b) > far(a) ? b : a),
      /** @type {(typeof rooms)[number] | null} */ (null),
    );
    if (pick) active.add(pick.i);
  }
  return active;
}
/** @param {Snapshot} snap */
export function drawSpawnRooms(snap) {
  const rooms = state.spawns?.rooms;
  if (!rooms?.length) return;
  const active = activeRooms(snap);
  rooms.forEach((r, i) => {
    if (OVERLAY && !active.has(i)) return;
    const corners = [
      [r.min[0], r.min[1]],
      [r.max[0], r.min[1]],
      [r.max[0], r.max[1]],
      [r.min[0], r.max[1]],
    ].map(([x, y]) => toScreen(x, y));
    const col = teamColour(r.team, "#999");
    ctx.save();
    ctx.beginPath();
    corners.forEach(([x, y], k) => (k ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    ctx.closePath();
    if (active.has(i)) {
      ctx.fillStyle = col + "2e";
      ctx.fill();
      ctx.strokeStyle = col;
      ctx.lineWidth = OVERLAY ? 1.5 : 2;
      ctx.stroke();
      const top = corners.reduce((a, b) => (b[1] < a[1] || (b[1] === a[1] && b[0] < a[0]) ? b : a));
      if (!OVERLAY && Math.hypot(corners[1][0] - corners[0][0], corners[1][1] - corners[0][1]) > 40) {
        const font = `800 ${Math.round(10 * uiScale())}px system-ui`;
        drawText("SPAWN", { font, fill: col, outline: null }, top[0] + 4, top[1] + 3, false);
      }
    } else {
      ctx.setLineDash([4, 4]);
      ctx.strokeStyle = col + "66";
      ctx.lineWidth = 1;
      ctx.stroke();
    }
    ctx.restore();
  });
}
