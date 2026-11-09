import { OVERLAY, overlayUnit, ctx, teamColour } from "../core/core.js";
import { toScreen, screenAngle, px } from "./view.js";
/** @typedef {import("../../types/stream").Projectile} Projectile */
/** @typedef {import("../../types/stream").Vec3} Vec3 */
/** A projectile as drawn: between snapshots it carries the heading it moved in (main.js). */
/** @typedef {Projectile & { heading?: number | null }} MovedProjectile */

// Rockets leave a trail of smoke puffs that shrink, darken and fade, and linger after the rocket.
const PUFF_SPACING = 30,
  PUFF_LIFE = 800,
  PUFF_CAP = 500;
/** @type {{ pos: [number, number]; t0: number }[]} */
const puffs = [],
  /** @type {Map<number, Vec3>} */
  lastPuff = new Map();
/** @param {MovedProjectile[]} list @param {(units: number, min: number) => number} size */
function drawSmoke(list, size) {
  const now = performance.now(),
    seen = new Set();
  for (const p of list || []) {
    if (p.kind !== "Rocket") continue;
    seen.add(p.id);
    const last = lastPuff.get(p.id);
    if (!last || Math.hypot(p.pos[0] - last[0], p.pos[1] - last[1]) >= PUFF_SPACING) {
      puffs.push({ pos: [p.pos[0], p.pos[1]], t0: now });
      lastPuff.set(p.id, p.pos);
    }
  }
  for (const id of lastPuff.keys()) if (!seen.has(id)) lastPuff.delete(id);
  while (puffs.length && (now - puffs[0].t0 >= PUFF_LIFE || puffs.length > PUFF_CAP)) puffs.shift();
  const r0 = size(12, 4);
  for (const f of puffs) {
    const k = (now - f.t0) / PUFF_LIFE,
      g = Math.round(200 - 88 * k);
    const [x, y] = toScreen(f.pos[0], f.pos[1]);
    ctx.globalAlpha = 0.5 * (1 - k);
    ctx.fillStyle = `rgb(${g},${g},${g})`;
    ctx.beginPath();
    ctx.arc(x, y, r0 * (1 - 0.35 * k), 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;
}

// The overlay sizes projectiles from its panel like the players, a little larger than their
// usual minimum, so they read at minimap scale.
/** @param {number} units @param {number} min */
const size = (units, min) => (OVERLAY ? (min * overlayUnit()) / 9 : px(units, min));

// Each kind's shape, drawn around the origin (the projectile) with its team colour and screen angle.
/** @param {MovedProjectile} p @param {string} col @param {number} a */
function drawRocket(p, col, a) {
  ctx.rotate(a);
  const l = size(26, 12),
    w = l * 0.32;
  const flame = ctx.createLinearGradient(-l * 1.3, 0, -l * 0.5, 0);
  flame.addColorStop(0, "#ffb34700");
  flame.addColorStop(1, "#ffd9a0cc");
  ctx.fillStyle = flame;
  ctx.fillRect(-l * 1.3, -w * 0.4, l * 0.8, w * 0.8);
  ctx.beginPath();
  ctx.moveTo(l * 0.6, 0);
  ctx.lineTo(-l * 0.5, -w);
  ctx.lineTo(-l * 0.5, w);
  ctx.closePath();
  ctx.fillStyle = p.crit ? "#fff" : col;
  ctx.fill();
  ctx.stroke();
}
/** @param {MovedProjectile} p @param {string} col */
function drawPipe(p, col) {
  const r = size(9, 4.5);
  ctx.beginPath();
  ctx.arc(0, 0, r, 0, Math.PI * 2);
  ctx.fillStyle = p.crit ? "#fff" : col;
  ctx.fill();
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(0, 0, r * 0.45, 0, Math.PI * 2);
  ctx.fillStyle = "#0008";
  ctx.fill();
}
/** @param {MovedProjectile} p @param {string} col */
function drawSticky(p, col) {
  const r = size(10, 6);
  ctx.beginPath();
  for (let i = 0; i < 16; i++) {
    const rr = i % 2 ? r * 0.55 : r * 1.15,
      t = (i / 16) * Math.PI * 2;
    (i ? ctx.lineTo : ctx.moveTo).call(ctx, Math.cos(t) * rr, Math.sin(t) * rr);
  }
  ctx.closePath();
  ctx.fillStyle = p.crit ? "#fff" : col;
  ctx.fill();
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(0, 0, r * 0.25, 0, Math.PI * 2);
  ctx.fillStyle = "#fff";
  ctx.fill();
}
/** @param {MovedProjectile} p @param {string} col @param {number} a */
function drawArrow(p, col, a) {
  ctx.rotate(a);
  const l = size(34, 18);
  ctx.beginPath();
  ctx.moveTo(-l * 0.6, 0);
  ctx.lineTo(l * 0.4, 0);
  ctx.strokeStyle = "#000";
  ctx.lineWidth = 3.5;
  ctx.stroke();
  ctx.strokeStyle = col;
  ctx.lineWidth = 2;
  ctx.stroke();
  ctx.beginPath();
  ctx.moveTo(l * 0.6, 0);
  ctx.lineTo(l * 0.3, -l * 0.16);
  ctx.lineTo(l * 0.3, l * 0.16);
  ctx.closePath();
  ctx.fillStyle = "#fff";
  ctx.fill();
  ctx.strokeStyle = "#000";
  ctx.lineWidth = 1;
  ctx.stroke();
  if (p.kind === "HealingArrow") {
    ctx.rotate(-a);
    ctx.fillStyle = "#7dff8a";
    ctx.fillRect(-l * 0.32, -1.5, l * 0.3, 3);
    ctx.fillRect(-l * 0.32 + l * 0.15 - 1.5, -l * 0.15, 3, l * 0.3);
  }
}
function drawFireball() {
  ctx.beginPath();
  ctx.arc(0, 0, size(9, 3.5), 0, Math.PI * 2);
  ctx.fillStyle = "#ffb347";
  ctx.shadowColor = "#ff7a00";
  ctx.shadowBlur = 12;
  ctx.fill();
}
/** @param {MovedProjectile} p @param {string} col */
function drawEnergy(p, col) {
  ctx.beginPath();
  ctx.arc(0, 0, size(12, 4.5), 0, Math.PI * 2);
  ctx.strokeStyle = col;
  ctx.lineWidth = 3;
  ctx.shadowColor = col;
  ctx.shadowBlur = 12;
  ctx.stroke();
}
// Thrown jars, and the cleaver in grey.
/** @type {{ [kind: string]: string }} */
const JAR_FILL = { Jarate: "#ffe14a", MadMilk: "#f4f4f4", GasPasser: "#ff9a3c" };
/** @param {MovedProjectile} p */
function drawJar(p) {
  const r = size(9, 4);
  ctx.rotate(Math.PI / 4);
  ctx.fillStyle = JAR_FILL[p.kind] || "#bbb";
  ctx.fillRect(-r, -r, r * 2, r * 2);
  ctx.strokeRect(-r, -r, r * 2, r * 2);
}
/** @param {MovedProjectile} p @param {string} col */
function drawOther(p, col) {
  ctx.beginPath();
  ctx.arc(0, 0, size(8, 3), 0, Math.PI * 2);
  ctx.fillStyle = col;
  ctx.fill();
  ctx.stroke();
}
const PROJECTILES = new Map([
  ["Rocket", drawRocket],
  ["Pipe", drawPipe],
  ["LooseCannon", drawPipe],
  ["Sticky", drawSticky],
  ["Arrow", drawArrow],
  ["HealingArrow", drawArrow],
  ["Flare", drawFireball],
  ["Fireball", drawFireball],
  ["EnergyBall", drawEnergy],
  ["EnergyRing", drawEnergy],
  ["ShortCircuitOrb", drawEnergy],
  ["Jarate", drawJar],
  ["MadMilk", drawJar],
  ["GasPasser", drawJar],
  ["Cleaver", drawJar],
]);

/** @param {MovedProjectile[]} list */
export function drawProjectiles(list) {
  drawSmoke(list, size);
  for (const p of list || []) {
    const [x, y] = toScreen(p.pos[0], p.pos[1]);
    const col = teamColour(p.team, "#ccc");
    // Rockets and arrows face their flight (the networked yaw is exact); grenades tumble.
    const deg = p.kind === "Rocket" || p.kind.endsWith("Arrow") ? p.yaw : (p.heading ?? p.yaw ?? 0);
    const a = screenAngle(deg);
    ctx.save();
    ctx.translate(x, y);
    ctx.lineJoin = "round";
    ctx.strokeStyle = "#000";
    ctx.lineWidth = 1.2;
    if (p.crit) {
      ctx.shadowColor = col;
      ctx.shadowBlur = 10;
    }
    (PROJECTILES.get(p.kind) || drawOther)(p, col, a);
    ctx.restore();
  }
}
