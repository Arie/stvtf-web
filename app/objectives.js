import { msPerTick, TEAM, OVERLAY, overlayUnit, uiScale, ctx, state, optOn } from "./core.js";
import { toScreen, screenAngle, px, playerRad } from "./view.js";
import { layerOn } from "./layers.js";
/** @typedef {import("../types/stream").Objective} Objective */
/** @typedef {import("../types/stream").PlacedObjective} PlacedObjective */
/** @typedef {import("../types/stream").Player} Player */
/** @typedef {import("../types/stream").LivePlayer} LivePlayer */
/** @typedef {import("../types/stream").Snapshot} Snapshot */
// numbers=0 leaves the number (and the cap rate) off the control points.
const SHOW_NUMBERS = optOn("numbers");

// The payload cart: a mine cart in the pushing team's colour carrying TF2's big black bomb, facing
// along the track. Heading comes from movement (kept while stopped); rolling back still faces forward.
export const cartState = new Map();
// The cart turns to face where it went once it moved this many units; a longer move at once is a
// round reset, not travel.
const CART_MIN_MOVE = 6,
  CART_RESET_JUMP = 300;
// A capture area belongs to the point standing in it, or one at most this many units outside.
const CAP_AREA_REACH = 256;
// Where a moving objective (cart, ball, robot, tank) is between the snapshots around `tick`.
/** @param {PlacedObjective} o @param {number} tick */
function objPos(o, tick) {
  const s = state.snaps;
  const i = s.findIndex(x => x.tick > tick);
  if (i < 1) return o.pos;
  const a = s[i - 1].objectives?.find(q => q.kind === o.kind && q.id === o.id);
  const b = s[i].objectives?.find(q => q.kind === o.kind && q.id === o.id);
  if (!a?.pos || !b?.pos) return o.pos;
  const t = (tick - s[i - 1].tick) / (s[i].tick - s[i - 1].tick);
  return [a.pos[0] + (b.pos[0] - a.pos[0]) * t, a.pos[1] + (b.pos[1] - a.pos[1]) * t];
}
// TF2's big black bomb with a lit fuse, of radius R: on the payload cart, and Mann vs. Machine's.
function drawBomb(x, y, R, lw) {
  ctx.save();
  ctx.fillStyle = "#15171a";
  ctx.strokeStyle = "#0b0c0e";
  ctx.lineWidth = lw;
  ctx.beginPath();
  ctx.arc(x, y, R, 0, Math.PI * 2);
  ctx.fill();
  ctx.stroke();
  ctx.strokeStyle = "#8a8f96";
  ctx.lineCap = "round";
  ctx.beginPath();
  ctx.moveTo(x + R * 0.45, y - R * 0.8);
  ctx.quadraticCurveTo(x + R * 0.75, y - R * 1.3, x + R * 1.1, y - R * 1.15);
  ctx.stroke();
  ctx.fillStyle = "#ffd25a";
  ctx.beginPath();
  ctx.arc(x + R * 1.1, y - R * 1.15, Math.max(1.2, R * 0.16), 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = "#ffffff55";
  ctx.beginPath();
  ctx.ellipse(x - R * 0.35, y - R * 0.35, R * 0.28, R * 0.18, -Math.PI / 4, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}
/** @param {PlacedObjective} o @param {number} tick */
function drawCart(o, tick) {
  const [wx, wy] = objPos(o, tick);
  let cs = cartState.get(o.id);
  if (!cs) cartState.set(o.id, (cs = { x: wx, y: wy, h: null }));
  const dx = wx - cs.x,
    dy = wy - cs.y;
  const moved = Math.hypot(dx, dy);
  // A jump (round reset back to the start) is not travel: keep the heading.
  if (moved > CART_RESET_JUMP) {
    cs.x = wx;
    cs.y = wy;
  } else if (moved > CART_MIN_MOVE) {
    cs.h = (Math.atan2(dy, dx) * 180) / Math.PI + (o.speed < 0 ? 180 : 0);
    cs.x = wx;
    cs.y = wy;
  }
  const [x, y] = toScreen(wx, wy);
  const u = OVERLAY ? overlayUnit() : 0;
  const L = OVERLAY ? u * 2.4 : px(72, 18),
    W = L * 0.64,
    lw = Math.max(1.5, L * 0.07);
  const team = TEAM[o.team] || TEAM.none,
    dark = "#0b0c0e";
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(screenAngle(cs.h ?? 0));
  ctx.lineWidth = lw;
  ctx.lineJoin = "round";
  if (o.speed) {
    const fwd = o.speed > 0,
      k = fwd ? 1 : -1,
      ax = k * (L / 2 + L * 0.12),
      ah = L * 0.22;
    ctx.beginPath();
    ctx.moveTo(ax, -ah);
    ctx.lineTo(ax + k * ah, 0);
    ctx.lineTo(ax, ah);
    ctx.lineWidth = lw * 1.4;
    ctx.lineCap = "round";
    ctx.strokeStyle = fwd ? team : "#ff8a5c";
    ctx.globalAlpha = 0.85;
    ctx.stroke();
    ctx.globalAlpha = 1;
    ctx.lineWidth = lw;
  }
  ctx.fillStyle = dark;
  for (const sx of [-1, 1])
    for (const sy of [-1, 1]) {
      ctx.beginPath();
      ctx.roundRect(sx * L * 0.3 - L * 0.1, (sy * W) / 2 - W * 0.12 + sy * W * 0.04, L * 0.2, W * 0.24, lw);
      ctx.fill();
    }
  ctx.beginPath();
  ctx.roundRect(-L / 2, -W / 2, L, W, W * 0.22);
  ctx.fillStyle = team;
  ctx.fill();
  ctx.strokeStyle = dark;
  ctx.stroke();
  ctx.restore();
  ctx.save();
  drawBomb(x, y, W * 0.36, lw);
  if (o.cappers > 0) {
    // A pill centred under the cart's lowest edge in screen space, touching it.
    const t = `×${o.cappers}`,
      th = screenAngle(cs.h ?? 0),
      fs = Math.round(OVERLAY ? u * 1.1 : Math.max(12 * uiScale(), L * 0.42));
    // The wheels stick out past the body, so clear them too.
    const low = (Math.abs(Math.sin(th)) * L) / 2 + Math.abs(Math.cos(th)) * W * 0.66 + lw / 2;
    ctx.font = `800 ${fs}px system-ui`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    const pw = ctx.measureText(t).width + fs * 0.7,
      ph = fs * 1.2,
      py = y + low - lw / 2;
    ctx.beginPath();
    ctx.roundRect(x - pw / 2, py, pw, ph, ph / 2);
    ctx.fillStyle = dark;
    ctx.fill();
    ctx.strokeStyle = "#fff";
    ctx.lineWidth = Math.max(1, lw * 0.6);
    ctx.stroke();
    ctx.fillStyle = "#fff";
    ctx.fillText(t, x, py + ph / 2 + 0.5);
  }
  ctx.restore();
}

// Pass time goals: a ring in the colour of the team that scores there.
/** @param {PlacedObjective} o */
function drawGoal(o) {
  const [x, y] = toScreen(o.pos[0], o.pos[1]),
    r = OVERLAY ? overlayUnit() * 1.4 : px(90, 14);
  const col = TEAM[o.team] || TEAM.none;
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = col + "33";
  ctx.fill();
  ctx.lineWidth = OVERLAY ? Math.max(2, r * 0.14) : 3;
  ctx.strokeStyle = col;
  ctx.stroke();
  ctx.fillStyle = "#fff";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = `800 ${Math.round(Math.max(9 * uiScale(), r * 0.55))}px system-ui`;
  ctx.lineWidth = 3;
  ctx.strokeStyle = "#0b0c0e";
  ctx.strokeText("GOAL", x, y + 0.5);
  ctx.fillText("GOAL", x, y + 0.5);
  ctx.restore();
}
// Robot destruction robots like buildings: the team colour drains from the top as health drops.
/** @param {PlacedObjective} o @param {number} tick */
function drawRobot(o, tick) {
  const [wx, wy] = objPos(o, tick);
  const [x, y] = toScreen(wx, wy),
    s = OVERLAY ? overlayUnit() * 1.6 : px(40, 15);
  const hpf = Math.max(0, Math.min(1, o.hp / (o.max_hp || 1)));
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, s / 2, 0, Math.PI * 2);
  ctx.fillStyle = "rgba(11,12,14,0.6)";
  ctx.fill();
  ctx.save();
  ctx.clip();
  ctx.fillStyle = TEAM[o.team] || TEAM.none;
  ctx.fillRect(x - s / 2, y + s / 2 - s * hpf, s, s * hpf);
  ctx.restore();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.fillStyle = "#fff";
  ctx.font = `800 ${Math.round(Math.max(9, s * 0.5))}px system-ui`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText("R", x, y + 0.5);
  ctx.restore();
}
// A dropped robot destruction core: a small diamond in its team's colour.
/** @param {PlacedObjective} o */
function drawCore(o) {
  const [x, y] = toScreen(o.pos[0], o.pos[1]),
    s = OVERLAY ? overlayUnit() * 0.7 : px(24, 7);
  ctx.save();
  ctx.beginPath();
  ctx.moveTo(x, y - s);
  ctx.lineTo(x + s, y);
  ctx.lineTo(x, y + s);
  ctx.lineTo(x - s, y);
  ctx.closePath();
  ctx.fillStyle = TEAM[o.team] || TEAM.none;
  ctx.fill();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.restore();
}
// A Mann vs. Machine revive marker: a cross in the dead player's team colour.
/** @param {PlacedObjective} o */
function drawRevive(o) {
  const [x, y] = toScreen(o.pos[0], o.pos[1]),
    s = OVERLAY ? overlayUnit() * 0.8 : px(30, 9);
  const team = state.lastPlayers.find(p => p.id === o.owner)?.team;
  ctx.save();
  ctx.beginPath();
  ctx.rect(x - s / 3, y - s, (s * 2) / 3, s * 2);
  ctx.rect(x - s, y - s / 3, s * 2, (s * 2) / 3);
  ctx.fillStyle = TEAM[team] || TEAM.none;
  ctx.fill();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.restore();
}
// Mann vs. Machine money on the ground: a small green note.
/** @param {PlacedObjective} o */
function drawMoney(o) {
  const [x, y] = toScreen(o.pos[0], o.pos[1]),
    s = OVERLAY ? overlayUnit() * 0.45 : px(16, 5);
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(x - s, y - s * 0.6, s * 2, s * 1.2, s * 0.2);
  ctx.fillStyle = "#57b35a";
  ctx.fill();
  ctx.lineWidth = 1;
  ctx.strokeStyle = "#0b0c0e";
  ctx.stroke();
  ctx.restore();
}
// A Mann vs. Machine tank: a big dark hull with its health above.
/** @param {PlacedObjective} o @param {number} tick */
function drawTank(o, tick) {
  const [wx, wy] = objPos(o, tick);
  const [x, y] = toScreen(wx, wy),
    L = OVERLAY ? overlayUnit() * 3.2 : px(200, 26),
    W = L * 0.6;
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(x - L / 2, y - W / 2, L, W, W * 0.2);
  ctx.fillStyle = "#3b4a35";
  ctx.fill();
  ctx.lineWidth = 2;
  ctx.strokeStyle = "#0b0c0e";
  ctx.stroke();
  const bw = L,
    bh = Math.max(4, W * 0.16),
    by = y - W / 2 - bh - 3;
  ctx.fillStyle = "#0b0c0e";
  ctx.fillRect(x - bw / 2, by, bw, bh);
  ctx.fillStyle = TEAM.blue;
  ctx.fillRect(x - bw / 2, by, bw * Math.max(0, Math.min(1, o.health)), bh);
  ctx.fillStyle = "#fff";
  ctx.font = `800 ${Math.round(Math.max(10, W * 0.4))}px system-ui`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText("TANK", x, y + 0.5);
  ctx.restore();
}

/** @param {Snapshot} snap @param {number} tick */
export function drawObjectives(snap, tick) {
  for (const o of snap.objectives || []) {
    if (o.kind === "cart") {
      drawCart(o, tick);
      continue;
    }
    if (o.kind === "goal") {
      drawGoal(o);
      continue;
    }
    if (o.kind === "robot") {
      drawRobot(o, tick);
      continue;
    }
    if (o.kind === "core") {
      drawCore(o);
      continue;
    }
    if (o.kind === "tank") {
      drawTank(o, tick);
      continue;
    }
    if (o.kind === "revive") {
      drawRevive(o);
      continue;
    }
    if (o.kind === "money") {
      drawMoney(o);
      continue;
    }
    // The flag and the pass time ball go on top of the players (drawFlags).
    if (o.kind === "control_point") drawPoint(o, snap, tick);
  }
}

// Points are squares like TF2's HUD, so they never read as players: the owner's colour, filled
// left to right in the capping team's colour while a capture is under way.
/** @param {Objective} o @param {Snapshot} snap @param {number} tick */
function drawPoint(o, snap, tick) {
  if (!o.pos) return;
  const [x, y] = toScreen(o.pos[0], o.pos[1]);
  const u = OVERLAY ? overlayUnit() : 0;
  const half = OVERLAY ? u * 1.2 : px(110, 11);
  const x0 = x - half,
    y0 = y - half,
    side = half * 2,
    corner = Math.max(2, half * 0.18);
  // Advance between snapshots at the rate stvtf extrapolates from the cap times and cappers.
  const progress = Math.max(
    0,
    Math.min(1, o.cap_progress + ((o.cap_rate || 0) * Math.max(0, tick - snap.tick) * msPerTick) / 1000),
  );
  const capping = o.capping_team !== "none" && progress > 0;
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(x0, y0, side, side, corner);
  ctx.fillStyle = (TEAM[o.owner] || TEAM.none) + (OVERLAY ? "e6" : "99");
  ctx.fill();
  if (capping) {
    ctx.save();
    ctx.clip();
    ctx.fillStyle = TEAM[o.capping_team];
    ctx.fillRect(x0, y0, side * progress, side);
    ctx.restore();
  }
  if (o.locked) {
    ctx.fillStyle = "#0b0c0e99";
    ctx.fill();
  }
  // Outlined in the colour of the team holding it, grey while nobody does.
  ctx.lineWidth = OVERLAY ? Math.max(2, u * 0.18) : 2;
  ctx.strokeStyle = TEAM[o.owner] || "#b8bcc2";
  ctx.stroke();
  if (SHOW_NUMBERS) drawPointNumber(o, capping, x, y, half);
  ctx.restore();
}

// The point's number, or while it is capped the cap rate (×2 for two cappers, as TF2's HUD shows it).
function drawPointNumber(o, capping, x, y, half) {
  const u = OVERLAY ? overlayUnit() : 0;
  ctx.fillStyle = "#fff";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = `800 ${Math.round(OVERLAY ? u * 1.3 : Math.max(12 * uiScale(), half * 0.9))}px system-ui`;
  const n = layerOn("points") && capping && o.on_point ? o.on_point[o.capping_team] : 0;
  const text = n > 0 ? `\u00d7${n}` : String(o.index + 1);
  ctx.lineWidth = 3;
  ctx.strokeStyle = "#0b0c0e";
  ctx.strokeText(text, x, y + 1);
  ctx.fillText(text, x, y + 1);
}

// Capture areas: just an outline in the owner's colour; progress is shown on the point square.
// An area belongs to the point standing in it: point numbers in the map file don't always match
// the game's (multi-round maps number per round).
/** @param {Snapshot} snap */
export function drawCapAreas(snap) {
  const areas = state.geometry?.cap_areas;
  if (!areas?.length) return;
  const cps = (snap.objectives || []).filter(o => o.kind === "control_point" && o.pos != null);
  const near = (a, p) =>
    Math.hypot(Math.max(a.min[0] - p[0], 0, p[0] - a.max[0]), Math.max(a.min[1] - p[1], 0, p[1] - a.max[1]));
  for (const a of areas) {
    const cp = cps.reduce(
      (b, o) => (!b || near(a, o.pos) < near(a, b.pos) ? o : b),
      /** @type {(typeof cps)[number] | null} */ (null),
    );
    if (!cp || near(a, cp.pos) > CAP_AREA_REACH) continue;
    ctx.save();
    ctx.beginPath();
    [
      [a.min[0], a.min[1]],
      [a.max[0], a.min[1]],
      [a.max[0], a.max[1]],
      [a.min[0], a.max[1]],
    ].forEach(([x, y], i) => {
      const [sx, sy] = toScreen(x, y);
      (i ? ctx.lineTo : ctx.moveTo).call(ctx, sx, sy);
    });
    ctx.closePath();
    ctx.strokeStyle = (TEAM[cp.owner] || TEAM.none) + "99";
    ctx.lineWidth = OVERLAY ? 1.5 : 2;
    ctx.stroke();
    ctx.restore();
  }
}

// Who carries an objective, while they are alive.
/** @param {Player[]} players @param {number | null | undefined} id @returns {LivePlayer | null} */
function liveCarrier(players, id) {
  const p = id == null ? null : players.find(q => q.id === id);
  return p?.alive ? p : null;
}

// The pass time ball: a ball in the colour of the team that has it (yellow while neutral),
// riding on its carrier.
/** @param {PlacedObjective} o @param {Player[]} players @param {number} tick */
function drawBall(o, players, tick) {
  const carrier = liveCarrier(players, o.carrier);
  const at = carrier ? carrier.pos : objPos(o, tick);
  let [x, y] = toScreen(at[0], at[1]);
  const r = OVERLAY ? overlayUnit() * 0.55 : px(18, 7);
  if (carrier) {
    const pr = playerRad();
    x += pr * 0.9;
    y -= pr * 0.9;
  }
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = o.team === "red" || o.team === "blue" ? TEAM[o.team] : "#f2c94c";
  ctx.fill();
  ctx.lineWidth = Math.max(1.5, r * 0.22);
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.fillStyle = "#ffffff88";
  ctx.beginPath();
  ctx.arc(x - r * 0.3, y - r * 0.3, r * 0.3, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}

// A player destruction stack of pickups: a disc with how many points it holds, on its carrier
// or where it was dropped (with the seconds until it goes).
/** @param {PlacedObjective} o @param {LivePlayer | null} carrier @param {Snapshot} snap @param {number} tick */
function drawPickups(o, carrier, snap, tick) {
  let [x, y] = toScreen(...(carrier ? carrier.pos : o.pos).slice(0, 2));
  const r = OVERLAY ? overlayUnit() * 0.6 : px(22, 8);
  if (carrier) {
    const pr = playerRad();
    x += pr * 0.9;
    y -= pr * 0.9;
  }
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = "#7bd88f";
  ctx.fill();
  ctx.lineWidth = Math.max(1.5, r * 0.18);
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.fillStyle = "#0b0c0e";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = `800 ${Math.round(Math.max(9, r * 1.2))}px system-ui`;
  ctx.fillText(String(o.points), x, y + 0.5);
  if (o.status === "dropped" && o.returns_in != null) {
    const left = Math.max(0, Math.ceil(o.returns_in - (Math.max(0, tick - snap.tick) * msPerTick) / 1000));
    ctx.font = `700 ${Math.round(Math.max(10 * uiScale(), r * 0.9))}px "HUD Condensed", system-ui`;
    ctx.textBaseline = "top";
    ctx.lineJoin = "round";
    ctx.lineWidth = 3;
    ctx.strokeStyle = "#000";
    ctx.strokeText(String(left), x, y + r + 2);
    ctx.fillStyle = "#fff";
    ctx.fillText(String(left), x, y + r + 2);
  }
  ctx.restore();
}

// The intelligence as a briefcase in its team's colour (a neutral flag, such as the bball ball,
// in yellow; Mann vs. Machine's bomb as a bomb): at home, dropped with the seconds until it goes
// home, or riding on its carrier.
/** @param {Snapshot} snap @param {Player[]} players @param {number} tick */
export function drawFlags(snap, players, tick) {
  for (const o of snap.objectives || []) {
    if (o.kind === "ball") {
      drawBall(o, players, tick);
      continue;
    }
    if (o.kind !== "flag") continue;
    const carrier = o.status === "carried" ? liveCarrier(players, o.carrier) : null;
    if (o.points != null && o.team !== "red" && o.team !== "blue") {
      drawPickups(o, carrier, snap, tick);
      continue;
    }
    const pos = carrier ? carrier.pos : o.pos;
    let [x, y] = toScreen(pos[0], pos[1]);
    const s = OVERLAY ? overlayUnit() * 1.3 : px(48, 17);
    if (carrier) {
      const r = playerRad();
      x += r * 0.9;
      y -= r * 0.9;
    }
    ctx.save();
    if (state.map?.startsWith("mvm_")) drawBomb(x, y, s * 0.45, Math.max(1.5, s * 0.1));
    else {
      ctx.lineWidth = Math.max(1.5, s * 0.12);
      ctx.strokeStyle = "#000";
      ctx.beginPath();
      ctx.roundRect(x - s * 0.25, y - s * 0.62, s * 0.5, s * 0.36, s * 0.1);
      ctx.stroke();
      ctx.beginPath();
      ctx.roundRect(x - s * 0.6, y - s * 0.35, s * 1.2, s * 0.8, s * 0.14);
      ctx.fillStyle = o.team === "red" || o.team === "blue" ? TEAM[o.team] : "#f2c94c";
      ctx.fill();
      ctx.stroke();
    }
    if (o.status === "dropped" && o.returns_in != null) {
      const left = Math.max(0, Math.ceil(o.returns_in - (Math.max(0, tick - snap.tick) * msPerTick) / 1000));
      ctx.font = `700 ${Math.round(Math.max(11 * uiScale(), s * 0.7))}px "HUD Condensed", system-ui`;
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
      ctx.lineJoin = "round";
      ctx.lineWidth = 3;
      ctx.strokeText(String(left), x, y + s * 0.55);
      ctx.fillStyle = "#fff";
      ctx.fillText(String(left), x, y + s * 0.55);
    }
    ctx.restore();
  }
}
