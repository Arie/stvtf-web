import { OVERLAY, overlayUnit, uiScale, ctx, state, teamColour } from "./core.js";
import { toScreen, screenAngle, px } from "./view.js";
/** @typedef {import("../types/stream").Snapshot} Snapshot */
/** @typedef {import("../types/stream").Building} Building */

// A sentry's dashed line to the player it is shooting, and its aim sticking out.
/** @param {Building} b @param {number} x @param {number} y @param {number} s */
function drawSentryAim(b, x, y, s) {
  const target = b.attacking && b.target != null ? state.lastPlayers.find(p => p.id === b.target) : null;
  if (target?.alive) {
    const [tx, ty] = toScreen(target.pos[0], target.pos[1]);
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(tx, ty);
    ctx.setLineDash([6, 5]);
    ctx.strokeStyle = teamColour(b.team, "#fff") + "aa";
    ctx.lineWidth = 1.5;
    ctx.stroke();
    ctx.setLineDash([]);
  }
  const a = screenAngle(b.aim ?? b.yaw ?? 0);
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(x + Math.cos(a) * s * 1.2, y + Math.sin(a) * s * 1.2);
  ctx.strokeStyle = "#fff";
  ctx.lineWidth = 3;
  ctx.stroke();
}

// S1-S3 / D1-D3 / T1-T3 like the engineer's HUD (MS for a mini-sentry); the sentry's aim
// sticks out, a teleporter exit gets a ring, and buildings under construction are faint.
/** @param {Snapshot} snap */
export function drawBuildings(snap) {
  for (const b of snap.buildings || []) {
    const [x, y] = toScreen(b.pos[0], b.pos[1]),
      s = OVERLAY ? overlayUnit() * 1.9 : px(44, 18);
    const label = b.kind === "sentry" && b.mini ? "MS" : (b.kind[0] || "?").toUpperCase() + (b.level || 1);
    ctx.save();
    if (b.building) ctx.globalAlpha = 0.55;
    if (b.kind === "sentry") drawSentryAim(b, x, y, s);
    // The team colour drains from the top as health drops.
    const hpf = Math.max(0, Math.min(1, b.hp / (b.max_hp || 1)));
    const tele = b.kind === "teleporter",
      shape = () => {
        ctx.beginPath();
        if (tele) ctx.arc(x, y, s / 2, 0, Math.PI * 2);
        else ctx.roundRect(x - s / 2, y - s / 2, s, s, 3);
      };
    shape();
    ctx.fillStyle = "rgba(11,12,14,0.6)";
    ctx.fill();
    ctx.save();
    ctx.clip();
    ctx.fillStyle = teamColour(b.team);
    ctx.fillRect(x - s / 2, y + s / 2 - s * hpf, s, s * hpf);
    ctx.restore();
    ctx.lineWidth = b.sapped ? 2.5 : 1.5;
    ctx.strokeStyle = b.sapped ? "#fff" : "#000";
    ctx.stroke();
    if (b.kind === "teleporter" && b.entrance === false) {
      ctx.beginPath();
      ctx.arc(x, y, s * 0.72, 0, Math.PI * 2);
      ctx.strokeStyle = teamColour(b.team, "#fff");
      ctx.lineWidth = 2;
      ctx.stroke();
    }
    ctx.fillStyle = "#fff";
    ctx.font = `800 ${Math.round(Math.max(9, s * 0.5))}px system-ui`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(label, x, y + 0.5);
    if (b.sapped) {
      ctx.fillStyle = "#fff";
      ctx.font = `800 ${Math.round(9 * uiScale())}px system-ui`;
      ctx.fillText("SAPPED", x, y + s / 2 + 7);
    }
    ctx.restore();
  }
}
