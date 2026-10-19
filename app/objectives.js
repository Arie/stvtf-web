import { MS_PER_TICK, TEAM, OVERLAY, overlayUnit, uiScale, ctx, state } from "./core.js";
import { toScreen, screenAngle, px, playerRad, drawText } from "./view.js";
import { layerOn } from "./layers.js";

// The payload cart: a mine cart in the pushing team's colour carrying TF2's big black bomb, facing
// along the track. Heading comes from movement (kept while stopped); rolling back still faces forward.
export const cartState = new Map();
function cartPos(o, tick) {
  const s = state.snaps;
  const i = s.findIndex(x => x.tick > tick);
  if (i < 1) return o.pos;
  const a = s[i - 1].objectives?.find(q => q.kind === "cart" && q.id === o.id);
  const b = s[i].objectives?.find(q => q.kind === "cart" && q.id === o.id);
  if (!a || !b) return o.pos;
  const t = (tick - s[i - 1].tick) / (s[i].tick - s[i - 1].tick);
  return [a.pos[0] + (b.pos[0] - a.pos[0]) * t, a.pos[1] + (b.pos[1] - a.pos[1]) * t];
}
function drawCart(o, tick) {
  const [wx, wy] = cartPos(o, tick);
  let cs = cartState.get(o.id);
  if (!cs) cartState.set(o.id, cs = { x: wx, y: wy, h: null });
  const dx = wx - cs.x, dy = wy - cs.y;
  const moved = Math.hypot(dx, dy);
  // A jump (round reset back to the start) is not travel: keep the heading.
  if (moved > 300) { cs.x = wx; cs.y = wy; }
  else if (moved > 6) {
    cs.h = Math.atan2(dy, dx) * 180 / Math.PI + (o.speed < 0 ? 180 : 0);
    cs.x = wx; cs.y = wy;
  }
  const [x, y] = toScreen(wx, wy);
  const u = OVERLAY ? overlayUnit() : 0;
  const L = OVERLAY ? u * 2.4 : px(72, 18), W = L * 0.64, lw = Math.max(1.5, L * 0.07);
  const team = TEAM[o.team] || TEAM.none, dark = "#0b0c0e";
  ctx.save();
  ctx.translate(x, y); ctx.rotate(screenAngle(cs.h ?? 0));
  ctx.lineWidth = lw; ctx.lineJoin = "round";
  if (o.speed) {
    const fwd = o.speed > 0, k = fwd ? 1 : -1, ax = k * (L / 2 + L * 0.12), ah = L * 0.22;
    ctx.beginPath();
    ctx.moveTo(ax, -ah); ctx.lineTo(ax + k * ah, 0); ctx.lineTo(ax, ah);
    ctx.lineWidth = lw * 1.4; ctx.lineCap = "round"; ctx.strokeStyle = fwd ? team : "#ff8a5c"; ctx.globalAlpha = 0.85; ctx.stroke();
    ctx.globalAlpha = 1; ctx.lineWidth = lw;
  }
  ctx.fillStyle = dark;
  for (const sx of [-1, 1]) for (const sy of [-1, 1]) {
    ctx.beginPath(); ctx.roundRect(sx * L * 0.3 - L * 0.1, sy * W / 2 - W * 0.12 + sy * W * 0.04, L * 0.2, W * 0.24, lw); ctx.fill();
  }
  ctx.beginPath(); ctx.roundRect(-L / 2, -W / 2, L, W, W * 0.22);
  ctx.fillStyle = team; ctx.fill(); ctx.strokeStyle = dark; ctx.stroke();
  ctx.restore();
  ctx.save();
  const R = W * 0.36;
  ctx.fillStyle = "#15171a"; ctx.strokeStyle = dark; ctx.lineWidth = lw;
  ctx.beginPath(); ctx.arc(x, y, R, 0, Math.PI * 2); ctx.fill(); ctx.stroke();
  ctx.strokeStyle = "#8a8f96"; ctx.lineCap = "round";
  ctx.beginPath(); ctx.moveTo(x + R * 0.45, y - R * 0.8); ctx.quadraticCurveTo(x + R * 0.75, y - R * 1.3, x + R * 1.1, y - R * 1.15); ctx.stroke();
  ctx.fillStyle = "#ffd25a"; ctx.beginPath(); ctx.arc(x + R * 1.1, y - R * 1.15, Math.max(1.2, R * 0.16), 0, Math.PI * 2); ctx.fill();
  ctx.fillStyle = "#ffffff55"; ctx.beginPath(); ctx.ellipse(x - R * 0.35, y - R * 0.35, R * 0.28, R * 0.18, -Math.PI / 4, 0, Math.PI * 2); ctx.fill();
  if (o.cappers > 0) {
    // A pill centred under the cart's lowest edge in screen space, touching it.
    const t = `×${o.cappers}`, th = screenAngle(cs.h ?? 0), fs = Math.round(OVERLAY ? u * 1.1 : Math.max(12 * uiScale(), L * 0.42));
    // The wheels stick out past the body, so clear them too.
    const low = Math.abs(Math.sin(th)) * L / 2 + Math.abs(Math.cos(th)) * W * 0.66 + lw / 2;
    ctx.font = `800 ${fs}px system-ui`; ctx.textAlign = "center"; ctx.textBaseline = "middle";
    const pw = ctx.measureText(t).width + fs * 0.7, ph = fs * 1.2, py = y + low - lw / 2;
    ctx.beginPath(); ctx.roundRect(x - pw / 2, py, pw, ph, ph / 2);
    ctx.fillStyle = dark; ctx.fill(); ctx.strokeStyle = "#fff"; ctx.lineWidth = Math.max(1, lw * 0.6); ctx.stroke();
    ctx.fillStyle = "#fff"; ctx.fillText(t, x, py + ph / 2 + 0.5);
  }
  ctx.restore();
}

export function drawObjectives(snap, tick) {
  for (const o of snap.objectives || []) {
    if (o.kind === "flag") continue;
    if (o.kind === "cart") { drawCart(o, tick); continue; }
    const [x, y] = toScreen(o.pos[0], o.pos[1]);
    // Points are squares like TF2's HUD, so they never read as players: the owner's colour,
    // filled left to right in the capping team's colour while a capture is under way.
    const u = OVERLAY ? overlayUnit() : 0;
    const half = OVERLAY ? u * 1.2 : px(110, 11);
    const x0 = x - half, y0 = y - half, side = half * 2, corner = Math.max(2, half * 0.18);
    // Advance between snapshots at the rate stvtf extrapolates from the cap times and cappers.
    const progress = Math.max(0, Math.min(1, o.cap_progress + (o.cap_rate || 0) * Math.max(0, tick - snap.tick) * MS_PER_TICK / 1000));
    const capping = o.capping_team !== "none" && progress > 0;
    ctx.save();
    ctx.beginPath(); ctx.roundRect(x0, y0, side, side, corner);
    ctx.fillStyle = (TEAM[o.owner] || TEAM.none) + (OVERLAY ? "e6" : "99"); ctx.fill();
    if (capping) {
      ctx.save(); ctx.clip();
      ctx.fillStyle = TEAM[o.capping_team]; ctx.fillRect(x0, y0, side * progress, side);
      ctx.restore();
    }
    if (o.locked) { ctx.fillStyle = "#0b0c0e99"; ctx.fill(); }
    // Outlined in the colour of the team holding it, grey while nobody does.
    ctx.lineWidth = OVERLAY ? Math.max(2, u * 0.18) : 2;
    ctx.strokeStyle = TEAM[o.owner] || "#b8bcc2"; ctx.stroke();
    ctx.fillStyle = "#fff"; ctx.textAlign = "center"; ctx.textBaseline = "middle";
    ctx.font = `800 ${Math.round(OVERLAY ? u * 1.3 : Math.max(12 * uiScale(), half * 0.9))}px system-ui`;
    // While capping, the cap rate (×2 for two cappers, as TF2's HUD shows it) instead of the number.
    const n = layerOn("points") && capping && o.on_point ? o.on_point[o.capping_team] : 0;
    const text = n > 0 ? `\u00d7${n}` : String(o.index + 1);
    ctx.lineWidth = 3; ctx.strokeStyle = "#0b0c0e"; ctx.strokeText(text, x, y + 1);
    ctx.fillText(text, x, y + 1);
    ctx.restore();
  }
}

// Capture areas: just an outline in the owner's colour; progress is shown on the point square.
export function drawCapAreas(snap) {
  const areas = state.geometry?.cap_areas;
  if (!areas?.length) return;
  const cps = new Map((snap.objectives || []).filter(o => o.kind === "control_point").map(o => [o.index, o]));
  for (const a of areas) {
    ctx.save();
    ctx.beginPath();
    [[a.min[0], a.min[1]], [a.max[0], a.min[1]], [a.max[0], a.max[1]], [a.min[0], a.max[1]]].forEach(([x, y], i) => { const [sx, sy] = toScreen(x, y); (i ? ctx.lineTo : ctx.moveTo).call(ctx, sx, sy); });
    ctx.closePath();
    ctx.strokeStyle = (TEAM[cps.get(a.point)?.owner] || TEAM.none) + "99"; ctx.lineWidth = OVERLAY ? 1.5 : 2; ctx.stroke();
    ctx.restore();
  }
}

// The intelligence as a briefcase in its team's colour (a neutral flag, such as the bball ball,
// in yellow): at home, dropped with the seconds until it goes home, or riding on its carrier.
export function drawFlags(snap, players, tick) {
  for (const o of snap.objectives || []) {
    if (o.kind !== "flag") continue;
    const carrier = o.status === "carried" ? players.find(p => p.id === o.carrier && p.alive && p.pos) : null;
    const pos = carrier ? carrier.pos : o.pos;
    let [x, y] = toScreen(pos[0], pos[1]);
    const s = OVERLAY ? overlayUnit() * 1.3 : px(48, 17);
    if (carrier) { const r = playerRad(); x += r * 0.9; y -= r * 0.9; }
    ctx.save();
    ctx.lineWidth = Math.max(1.5, s * 0.12); ctx.strokeStyle = "#000";
    ctx.beginPath(); ctx.roundRect(x - s * 0.25, y - s * 0.62, s * 0.5, s * 0.36, s * 0.1); ctx.stroke();
    ctx.beginPath(); ctx.roundRect(x - s * 0.6, y - s * 0.35, s * 1.2, s * 0.8, s * 0.14);
    ctx.fillStyle = o.team === "red" || o.team === "blue" ? TEAM[o.team] : "#f2c94c"; ctx.fill(); ctx.stroke();
    if (o.status === "dropped" && o.returns_in != null) {
      const left = Math.max(0, Math.ceil(o.returns_in - Math.max(0, tick - snap.tick) * MS_PER_TICK / 1000));
      ctx.font = `700 ${Math.round(Math.max(11 * uiScale(), s * 0.7))}px "HUD Condensed", system-ui`;
      ctx.textAlign = "center"; ctx.textBaseline = "top"; ctx.lineJoin = "round";
      ctx.lineWidth = 3; ctx.strokeText(String(left), x, y + s * 0.55);
      ctx.fillStyle = "#fff"; ctx.fillText(String(left), x, y + s * 0.55);
    }
    ctx.restore();
  }
}

// S1-S3 / D1-D3 / T1-T3 like the engineer's HUD (MS for a mini-sentry); the sentry's aim
// sticks out, a teleporter exit gets a ring, and buildings under construction are faint.
export function drawBuildings(snap) {
  for (const b of snap.buildings || []) {
    const [x, y] = toScreen(b.pos[0], b.pos[1]), s = OVERLAY ? overlayUnit() * 1.9 : px(44, 18);
    const label = b.kind === "sentry" && b.mini ? "MS" : (b.kind[0] || "?").toUpperCase() + (b.level || 1);
    ctx.save();
    if (b.building) ctx.globalAlpha = 0.55;
    if (b.kind === "sentry") {
      const target = b.attacking && b.target != null ? state.lastPlayers.find(p => p.id === b.target && p.alive && p.pos) : null;
      if (target) {
        const [tx, ty] = toScreen(target.pos[0], target.pos[1]);
        ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(tx, ty);
        ctx.setLineDash([6, 5]); ctx.strokeStyle = (TEAM[b.team] || "#fff") + "aa"; ctx.lineWidth = 1.5; ctx.stroke(); ctx.setLineDash([]);
      }
      const a = screenAngle(b.aim ?? b.yaw ?? 0);
      ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(x + Math.cos(a) * s * 1.2, y + Math.sin(a) * s * 1.2);
      ctx.strokeStyle = "#fff"; ctx.lineWidth = 3; ctx.stroke();
    }
    // The team colour drains from the top as health drops.
    const hpf = Math.max(0, Math.min(1, b.hp / (b.max_hp || 1)));
    const tele = b.kind === "teleporter", shape = () => { ctx.beginPath(); if (tele) ctx.arc(x, y, s / 2, 0, Math.PI * 2); else ctx.roundRect(x - s / 2, y - s / 2, s, s, 3); };
    shape();
    ctx.fillStyle = "rgba(11,12,14,0.6)"; ctx.fill();
    ctx.save(); ctx.clip();
    ctx.fillStyle = TEAM[b.team] || TEAM.none; ctx.fillRect(x - s / 2, y + s / 2 - s * hpf, s, s * hpf);
    ctx.restore();
    ctx.lineWidth = b.sapped ? 2.5 : 1.5; ctx.strokeStyle = b.sapped ? "#fff" : "#000"; ctx.stroke();
    if (b.kind === "teleporter" && b.entrance === false) {
      ctx.beginPath(); ctx.arc(x, y, s * 0.72, 0, Math.PI * 2); ctx.strokeStyle = TEAM[b.team] || "#fff"; ctx.lineWidth = 2; ctx.stroke();
    }
    ctx.fillStyle = "#fff"; ctx.font = `800 ${Math.round(Math.max(9, s * 0.5))}px system-ui`;
    ctx.textAlign = "center"; ctx.textBaseline = "middle";
    ctx.fillText(label, x, y + 0.5);
    if (b.sapped) { ctx.fillStyle = "#fff"; ctx.font = `800 ${Math.round(9 * uiScale())}px system-ui`; ctx.fillText("SAPPED", x, y + s / 2 + 7); }
    ctx.restore();
  }
}

// ---------- spawn rooms: which are in use
// A spawn point tied to a control point is enabled for its team while that team owns the
// point (TF2's rule); untied ones always are. Of the enabled rooms a team spawns in the most
// forward one: the one farthest from its main spawn (the room with the most spawn points).
// Once a team has been seen spawning since the last change of ownership, that room wins.
let ownersKey = "", ownersChanged = 0;
const inRoom = (r, p) => p[0] >= r.min[0] && p[0] <= r.max[0] && p[1] >= r.min[1] && p[1] <= r.max[1] && p[2] >= r.min[2] - 64 && p[2] <= r.max[2] + 64;
const centre = r => [(r.min[0] + r.max[0]) / 2, (r.min[1] + r.max[1]) / 2];
export function noteSpawn(ev) {
  const rooms = state.spawns?.rooms || [];
  const i = ev.pos ? rooms.findIndex(r => r.team === ev.team && inRoom(r, ev.pos)) : -1;
  if (i >= 0) state.spawnSeen[ev.team] = { room: i, at: performance.now() };
}
function activeRooms(snap) {
  const s = state.spawns;
  if (!s) return new Set();
  const owner = {};
  for (const o of snap.objectives || []) if (o.kind === "control_point") owner[o.index] = o.owner;
  const key = JSON.stringify(owner);
  if (key !== ownersKey) { ownersKey = key; ownersChanged = performance.now(); }
  const live = s.points.filter(p => !p.disabled && (p.point == null || owner[p.point] === undefined || owner[p.point] === p.team));
  const active = new Set();
  for (const team of ["red", "blue"]) {
    const seen = state.spawnSeen[team];
    if (seen && seen.at >= ownersChanged) { active.add(seen.room); continue; }
    const rooms = s.rooms.map((r, i) => ({ r, i, n: s.points.filter(p => p.team === team && inRoom(r, p.pos)).length })).filter(x => x.r.team === team);
    const main = rooms.reduce((a, b) => (b.n > (a?.n ?? -1) ? b : a), null);
    if (!main) continue;
    const enabled = rooms.filter(x => !x.r.disabled && live.some(p => p.team === team && inRoom(x.r, p.pos)));
    const far = x => Math.hypot(centre(x.r)[0] - centre(main.r)[0], centre(x.r)[1] - centre(main.r)[1]);
    const pick = enabled.reduce((a, b) => (!a || far(b) > far(a) ? b : a), null);
    if (pick) active.add(pick.i);
  }
  return active;
}
export function drawSpawnRooms(snap) {
  const rooms = state.spawns?.rooms;
  if (!rooms?.length) return;
  const active = activeRooms(snap);
  rooms.forEach((r, i) => {
    if (OVERLAY && !active.has(i)) return;
    const corners = [[r.min[0], r.min[1]], [r.max[0], r.min[1]], [r.max[0], r.max[1]], [r.min[0], r.max[1]]].map(([x, y]) => toScreen(x, y));
    const col = TEAM[r.team] || "#999";
    ctx.save();
    ctx.beginPath(); corners.forEach(([x, y], k) => (k ? ctx.lineTo(x, y) : ctx.moveTo(x, y))); ctx.closePath();
    if (active.has(i)) {
      ctx.fillStyle = col + "2e"; ctx.fill();
      ctx.strokeStyle = col; ctx.lineWidth = OVERLAY ? 1.5 : 2; ctx.stroke();
      const top = corners.reduce((a, b) => (b[1] < a[1] || (b[1] === a[1] && b[0] < a[0]) ? b : a));
      if (!OVERLAY && Math.hypot(corners[1][0] - corners[0][0], corners[1][1] - corners[0][1]) > 40) {
        drawText("SPAWN", `800 ${Math.round(10 * uiScale())}px system-ui`, col, null, top[0] + 4, top[1] + 3, false);
      }
    } else {
      ctx.setLineDash([4, 4]); ctx.strokeStyle = col + "66"; ctx.lineWidth = 1; ctx.stroke();
    }
    ctx.restore();
  });
}
