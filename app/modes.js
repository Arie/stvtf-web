import { TEAM, OVERLAY, overlayUnit, uiScale, ctx, state, teamColour, INK, objPos } from "./core.js";
import { toScreen, px, label } from "./view.js";
/** @typedef {import("../types/stream").PlacedObjective} PlacedObjective */

// The game modes' own objectives: pass time goals, robot destruction's robots and cores, and
// Mann vs. Machine's tank, revive markers and money.
// Pass time goals: a ring in the colour of the team that scores there.
/** @param {PlacedObjective} o */
export function drawGoal(o) {
  const [x, y] = toScreen(o.pos[0], o.pos[1]),
    r = OVERLAY ? overlayUnit() * 1.4 : px(90, 14);
  const col = teamColour(o.team);
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
  ctx.strokeStyle = INK;
  ctx.strokeText("GOAL", x, y + 0.5);
  ctx.fillText("GOAL", x, y + 0.5);
  ctx.restore();
}
// Robot destruction robots like buildings: the team colour drains from the top as health drops.
/** @param {PlacedObjective} o @param {number} tick */
export function drawRobot(o, tick) {
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
  ctx.fillStyle = teamColour(o.team);
  ctx.fillRect(x - s / 2, y + s / 2 - s * hpf, s, s * hpf);
  ctx.restore();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
  label("R", x, y, Math.max(9, s * 0.5));
  ctx.restore();
}
// A dropped robot destruction core: a small diamond in its team's colour.
/** @param {PlacedObjective} o */
export function drawCore(o) {
  const [x, y] = toScreen(o.pos[0], o.pos[1]),
    s = OVERLAY ? overlayUnit() * 0.7 : px(24, 7);
  ctx.save();
  ctx.beginPath();
  ctx.moveTo(x, y - s);
  ctx.lineTo(x + s, y);
  ctx.lineTo(x, y + s);
  ctx.lineTo(x - s, y);
  ctx.closePath();
  ctx.fillStyle = teamColour(o.team);
  ctx.fill();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.restore();
}
// A Mann vs. Machine revive marker: a cross in the dead player's team colour.
/** @param {PlacedObjective} o */
export function drawRevive(o) {
  const [x, y] = toScreen(o.pos[0], o.pos[1]),
    s = OVERLAY ? overlayUnit() * 0.8 : px(30, 9);
  const team = state.lastPlayers.find(p => p.id === o.owner)?.team;
  ctx.save();
  ctx.beginPath();
  ctx.rect(x - s / 3, y - s, (s * 2) / 3, s * 2);
  ctx.rect(x - s, y - s / 3, s * 2, (s * 2) / 3);
  ctx.fillStyle = teamColour(team);
  ctx.fill();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.restore();
}
// Mann vs. Machine money on the ground: a small green note.
/** @param {PlacedObjective} o */
export function drawMoney(o) {
  const [x, y] = toScreen(o.pos[0], o.pos[1]),
    s = OVERLAY ? overlayUnit() * 0.45 : px(16, 5);
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(x - s, y - s * 0.6, s * 2, s * 1.2, s * 0.2);
  ctx.fillStyle = "#57b35a";
  ctx.fill();
  ctx.lineWidth = 1;
  ctx.strokeStyle = INK;
  ctx.stroke();
  ctx.restore();
}
// A Mann vs. Machine tank: a big dark hull with its health above.
/** @param {PlacedObjective} o @param {number} tick */
export function drawTank(o, tick) {
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
  ctx.strokeStyle = INK;
  ctx.stroke();
  const bw = L,
    bh = Math.max(4, W * 0.16),
    by = y - W / 2 - bh - 3;
  ctx.fillStyle = INK;
  ctx.fillRect(x - bw / 2, by, bw, bh);
  ctx.fillStyle = TEAM.blue;
  ctx.fillRect(x - bw / 2, by, bw * Math.max(0, Math.min(1, o.health)), bh);
  label("TANK", x, y, Math.max(10, W * 0.4));
  ctx.restore();
}
