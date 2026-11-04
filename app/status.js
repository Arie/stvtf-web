import { TEAM, OVERLAY, uiScale, ctx } from "./core.js";
/** @typedef {import("../types/stream").Player} Player */

// Each charge looks different: über a candy-cane team ring with a dark edge and a soft pulse, kritz crackling, quick-fix
// pulsing rings, vaccinator a shield with its resistance.
/** @param {Player} p */
export function drawUberRing(p, x, y, rad, col) {
  const t = performance.now() / 1000;
  ctx.save();
  if (p.ubered === "uber") {
    const base = TEAM[p.team],
      pulse = 0.5 + 0.5 * Math.sin(t * 5);
    const tint =
      "#" +
      [1, 3, 5]
        .map(i =>
          Math.round(parseInt(base.substr(i, 2), 16) * 0.5 + 128)
            .toString(16)
            .padStart(2, "0"),
        )
        .join("");
    const lw = OVERLAY ? Math.max(3, rad * 0.3) : 5,
      r0 = rad + 3 + lw / 2,
      ri = r0 - lw / 2,
      ro = r0 + lw / 2;
    ctx.beginPath();
    ctx.arc(x, y, r0, 0, Math.PI * 2);
    ctx.shadowColor = base;
    ctx.shadowBlur = 10 + 8 * pulse;
    ctx.strokeStyle = "#0b0c0e";
    ctx.lineWidth = lw + 3;
    ctx.stroke();
    ctx.shadowBlur = 0;
    ctx.strokeStyle = base;
    ctx.lineWidth = lw;
    ctx.stroke();
    // Candy cane like the HUD card: tint stripes slanted by their outer edge, turning three stripe pairs a second.
    const n = 12,
      step = (Math.PI * 2) / n,
      slant = step * 0.8,
      rot = t * step * 3;
    ctx.fillStyle = tint;
    ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const a = rot + i * step,
        w = step / 2;
      ctx.moveTo(x + Math.cos(a) * ri, y + Math.sin(a) * ri);
      ctx.lineTo(x + Math.cos(a + w) * ri, y + Math.sin(a + w) * ri);
      ctx.lineTo(x + Math.cos(a + w + slant) * ro, y + Math.sin(a + w + slant) * ro);
      ctx.lineTo(x + Math.cos(a + slant) * ro, y + Math.sin(a + slant) * ro);
      ctx.closePath();
    }
    ctx.fill();
    ctx.beginPath();
    ctx.arc(x, y, ri, 0, Math.PI * 2);
    ctx.strokeStyle = "rgba(11,12,14,0.7)";
    ctx.lineWidth = 1;
    ctx.stroke();
  } else if (p.ubered === "kritz") {
    ctx.beginPath();
    for (let i = 0; i <= 24; i++) {
      const a = (i / 24) * Math.PI * 2 + t * 3,
        r = rad + 6 + (i % 2 ? 4 : -1) + Math.sin(t * 20 + i) * 1.5;
      (i ? ctx.lineTo : ctx.moveTo).call(ctx, x + Math.cos(a) * r, y + Math.sin(a) * r);
    }
    ctx.strokeStyle = "#fff";
    ctx.shadowColor = col;
    ctx.shadowBlur = 10;
    ctx.lineWidth = 2;
    ctx.stroke();
  } else if (p.ubered === "quickfix") {
    for (let i = 0; i < 2; i++) {
      const k = (t * 1.2 + i / 2) % 1;
      ctx.beginPath();
      ctx.arc(x, y, rad + 4 + k * 16, 0, Math.PI * 2);
      ctx.strokeStyle = col;
      ctx.globalAlpha = 1 - k;
      ctx.lineWidth = 3;
      ctx.stroke();
    }
  } else {
    ctx.beginPath();
    ctx.arc(x, y, rad + 7, -Math.PI * 0.8, -Math.PI * 0.2);
    ctx.strokeStyle = "#fff";
    ctx.lineWidth = 4;
    ctx.shadowColor = col;
    ctx.shadowBlur = 12;
    ctx.stroke();
    ctx.shadowBlur = 0;
    ctx.font = `bold ${Math.round(10 * uiScale())}px system-ui`;
    ctx.textAlign = "center";
    ctx.textBaseline = "bottom";
    ctx.fillStyle = "#fff";
    ctx.fillText(p.ubered.split("_")[1].toUpperCase(), x, y - rad - 9);
  }
  ctx.restore();
}

// Jarate, Mad Milk and bleeding fill the bottom of the dot like a liquid (side by side when there
// are several) and burning licks up from it, under the class icon.
const LIQUIDS = [
  ["Urine", "#e8c81f"],
  ["MadMilk", "#dfe3e6"],
  ["Bleeding", "#5c0610"],
];
export function drawStatusInside(conds, id, x, y, rad) {
  const liquids = LIQUIDS.filter(([c]) => conds.includes(c)),
    burning = conds.includes("Burning");
  if (!liquids.length && !burning) return;
  const t = performance.now() / 1000,
    level = y + rad * 0.15;
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, rad, 0, Math.PI * 2);
  ctx.clip();
  liquids.forEach(([, fill], i) => {
    const w = (rad * 2) / liquids.length,
      x0 = x - rad + i * w,
      surface = [];
    for (let k = 0; k <= 6; k++) {
      const sx = x0 + (w * k) / 6;
      surface.push([sx, level + Math.sin(t * 3 + ((sx - x) / rad) * 3 + id) * rad * 0.07]);
    }
    ctx.beginPath();
    ctx.moveTo(x0, y + rad);
    for (const [sx, sy] of surface) ctx.lineTo(sx, sy);
    ctx.lineTo(x0 + w, y + rad);
    ctx.closePath();
    ctx.fillStyle = fill;
    ctx.fill();
    // A dark edge on the surface keeps it apart from the dot and the white class icon.
    ctx.beginPath();
    surface.forEach(([sx, sy], k) => (k ? ctx.lineTo(sx, sy) : ctx.moveTo(sx, sy)));
    ctx.strokeStyle = "rgba(0,0,0,0.45)";
    ctx.lineWidth = Math.max(1, rad * 0.08);
    ctx.stroke();
  });
  if (burning)
    for (let j = 0; j < 3; j++) {
      const cx = x + (j - 1) * rad * 0.55,
        h = rad * (0.95 + 0.3 * Math.sin(t * 14 + j * 2.1 + id)),
        sway = Math.sin(t * 9 + j + id) * rad * 0.12;
      const tongue = (k, fill) => {
        ctx.beginPath();
        ctx.moveTo(cx - rad * 0.38 * k, y + rad);
        ctx.quadraticCurveTo(cx - rad * 0.3 * k, y + rad - h * 0.6 * k, cx + sway, y + rad - h * k);
        ctx.quadraticCurveTo(cx + rad * 0.3 * k, y + rad - h * 0.6 * k, cx + rad * 0.38 * k, y + rad);
        ctx.closePath();
        ctx.fillStyle = fill;
        ctx.fill();
      };
      tongue(1, "#ff6a14");
      tongue(0.55, "#ffd23a");
    }
  ctx.restore();
}

// Marked for death sits above the dot and stun stars orbit it. Nothing moves the dot or covers its icon and ring.
export function drawStatus(conds, id, x, y, rad) {
  const t = performance.now() / 1000,
    r = Math.max(3, rad * 0.4),
    lw = Math.max(1, r * 0.2);
  const has = n => conds.includes(n);
  ctx.save();
  ctx.lineWidth = lw;
  ctx.strokeStyle = "#000";
  if (has("MarkedForDeath") || has("MarkedForDeathSilent")) {
    const cx = x,
      cy = y - rad - r * 1.3 - 3;
    ctx.beginPath();
    ctx.arc(cx, cy, r * 0.95, 0, Math.PI * 2);
    ctx.fillStyle = "#fff";
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = "#000";
    ctx.beginPath();
    ctx.arc(cx - r * 0.35, cy - r * 0.1, r * 0.25, 0, Math.PI * 2);
    ctx.arc(cx + r * 0.35, cy - r * 0.1, r * 0.25, 0, Math.PI * 2);
    ctx.fill();
  }
  if (has("Stunned"))
    for (let i = 0; i < 3; i++) {
      const a = t * 4 + id + (i * Math.PI * 2) / 3,
        cx = x + Math.cos(a) * rad * 0.9,
        cy = y - rad * 0.8 + Math.sin(a) * rad * 0.28,
        k = r * 0.55;
      ctx.beginPath();
      ctx.moveTo(cx, cy - k);
      ctx.lineTo(cx + k * 0.3, cy - k * 0.3);
      ctx.lineTo(cx + k, cy);
      ctx.lineTo(cx + k * 0.3, cy + k * 0.3);
      ctx.lineTo(cx, cy + k);
      ctx.lineTo(cx - k * 0.3, cy + k * 0.3);
      ctx.lineTo(cx - k, cy);
      ctx.lineTo(cx - k * 0.3, cy - k * 0.3);
      ctx.closePath();
      ctx.fillStyle = "#ffe14a";
      ctx.fill();
      ctx.stroke();
    }
  ctx.restore();
}
