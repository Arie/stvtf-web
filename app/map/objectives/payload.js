import { OVERLAY, overlayUnit, uiScale, ctx, teamColour, INK, objPos } from "../../core/core.js";
import { toScreen, screenAngle, px } from "../view.js";
/** @typedef {import("../../../types/stream").PlacedObjective} PlacedObjective */

// The payload cart, and the bomb it (and Mann vs. Machine's carrier) carries.
// The payload cart: a mine cart in the pushing team's colour carrying TF2's big black bomb, facing
// along the track. Heading comes from movement (kept while stopped); rolling back still faces forward.
/** @type {Map<number | undefined, { x: number; y: number; h: number | null }>} */
export const cartState = new Map();
// The cart turns to face where it went once it moved this many units; a longer move at once is a
// round reset, not travel.
const CART_MIN_MOVE = 6,
  CART_RESET_JUMP = 300;
// TF2's big black bomb with a lit fuse, of radius R: on the payload cart, and Mann vs. Machine's.
/** @param {number} x @param {number} y @param {number} R @param {number} lw */
export function drawBomb(x, y, R, lw) {
  ctx.save();
  ctx.fillStyle = "#15171a";
  ctx.strokeStyle = INK;
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
export function drawCart(o, tick) {
  const [wx, wy] = objPos(o, tick);
  const a = screenAngle(cartHeading(o, wx, wy));
  const [x, y] = toScreen(wx, wy);
  const u = OVERLAY ? overlayUnit() : 0;
  const L = OVERLAY ? u * 2.4 : px(72, 18),
    size = { L, W: L * 0.64, lw: Math.max(1.5, L * 0.07) };
  const team = teamColour(o.team);
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(a);
  ctx.lineWidth = size.lw;
  ctx.lineJoin = "round";
  if (o.speed) drawCartArrow(o.speed, size, team);
  drawCartBody(size, team);
  ctx.restore();
  ctx.save();
  drawBomb(x, y, size.W * 0.36, size.lw);
  if (o.cappers > 0) drawCappers(o.cappers, [x, y], a, size, u);
  ctx.restore();
}
/** @typedef {{ L: number; W: number; lw: number }} CartSize the cart's length, width and line width */

// The cart's heading in degrees: the way it last moved (rolling back still faces forward), kept
// while it stands still.
/** @param {PlacedObjective} o @param {number} wx @param {number} wy */
function cartHeading(o, wx, wy) {
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
  return cs.h ?? 0;
}

// An arrowhead ahead of the cart while it moves (behind it, in orange, while it rolls back).
/** @param {number} speed @param {CartSize} size @param {string} team */
function drawCartArrow(speed, { L, lw }, team) {
  const fwd = speed > 0,
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

// Four wheels and the body, along the x axis.
/** @param {CartSize} size @param {string} team */
function drawCartBody({ L, W, lw }, team) {
  ctx.fillStyle = INK;
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
  ctx.strokeStyle = INK;
  ctx.stroke();
}

// How many push the cart: a pill centred under its lowest edge on screen, touching it.
/**
 * @param {number} cappers @param {[number, number]} at @param {number} th the cart's screen angle
 * @param {CartSize} size @param {number} u
 */
function drawCappers(cappers, [x, y], th, { L, W, lw }, u) {
  const t = `×${cappers}`,
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
  ctx.fillStyle = INK;
  ctx.fill();
  ctx.strokeStyle = "#fff";
  ctx.lineWidth = Math.max(1, lw * 0.6);
  ctx.stroke();
  ctx.fillStyle = "#fff";
  ctx.fillText(t, x, py + ph / 2 + 0.5);
}
