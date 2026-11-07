import { TEAM, ctx } from "./core.js";
/** @typedef {import("../types/stream").Player} Player */

// Medigun charges are drawn inside the dot, so a charged player stays the same size and the class
// icon goes on top: über flowing team-coloured liquid metal, kritz lightning, quick-fix ripples
// and the vaccinator a team-coloured bubble, its resistance in a badge on the rim.
const TAU = Math.PI * 2;
/** @typedef {{ base: string; id: number; x: number; y: number; rad: number; t: number }} Charge */

/** @param {Player} p @param {number} x @param {number} y @param {number} rad */
export function drawChargeInside(p, x, y, rad) {
  const draw = p.ubered && (CHARGES.get(p.ubered) ?? (p.ubered.startsWith("vaccinator_") ? vaccinator : null));
  if (!draw || (p.ubered === "uber" && !uberShown(p))) return;
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, rad, 0, TAU);
  ctx.clip();
  draw({ base: TEAM[p.team] ?? TEAM.none, id: p.id, x, y, rad, t: performance.now() / 1000 });
  ctx.restore();
}

// A team colour mixed a fraction k of the way to black (k < 0) or white (k > 0), or with alpha a.
/** @param {string} hex @param {number} k @param {number} [a] */
const mix = (hex, k, a = 1) => {
  const c = [1, 3, 5].map(i => {
    const v = parseInt(hex.substr(i, 2), 16);
    return Math.round(k < 0 ? v * (1 + k) : v + (255 - v) * k);
  });
  return a < 1 ? `rgba(${c.join(",")},${a})` : "#" + c.map(v => v.toString(16).padStart(2, "0")).join("");
};
// A repeatable pseudo-random number in [0, 1) for n.
/** @param {number} n */
const hash = n => {
  const s = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return s - Math.floor(s);
};
/** @param {number} x @param {number} y @param {number} rad @param {CanvasGradient | string} fill */
const fillDot = (x, y, rad, fill) => {
  ctx.fillStyle = fill;
  ctx.fillRect(x - rad, y - rad, rad * 2, rad * 2);
};

// When über leaves a player (the medic heals someone else, or the charge runs out) the game keeps
// them invulnerable a moment longer, flickering, with InvulnerableWearingOff set beside it. Some
// servers leave that flag on a medic for good, so it only counts once the über was seen without it.
/** @type {Map<number, boolean>} */
const cleanUber = new Map();
/** Whether the über is off in this frame of the wearing-off flicker. @param {Player} p */
function uberFlickerOff(p) {
  if (p.ubered !== "uber") {
    cleanUber.delete(p.id);
    return false;
  }
  if (!p.conds?.includes("InvulnerableWearingOff")) {
    cleanUber.set(p.id, true);
    return false;
  }
  return Boolean(cleanUber.get(p.id)) && Math.floor(performance.now() / 90) % 2 === 1;
}

// Über, loud: bright team-coloured metal with flowing bands, a sweeping flash, a rim light and a
// pulsing halo (drawUberGlow); the chrome class icon keeps a dark outline.
const UBER_BANDS = /** @type {const} */ ([
  [0, 0.4],
  [0.18, 0.05],
  [0.33, -0.2],
  [0.5, 0.2],
  [0.68, 0.55],
  [0.85, 0],
]);
// One flash every SWEEP_S seconds, lasting SWEEP_ON of it.
const SWEEP_S = 1.4,
  SWEEP_ON = 0.45;
// The icon's bands: the same pattern, light.
const ICON_BANDS = /** @type {const} */ ([
  [0, 0.85],
  [0.18, 0.55],
  [0.33, 0.3],
  [0.5, 0.6],
  [0.68, 0.9],
  [0.85, 0.4],
]);
// Bands flowing across a box of half-size r round (x, y), shifted per player.
/** @typedef {{ base: string; id: number; t: number; x: number; y: number; r: number }} Bands */
/** @param {CanvasRenderingContext2D} g @param {readonly (readonly [number, number])[]} bands @param {Bands} at */
function metal(g, bands, { base, id, t, x, y, r }) {
  const o = (t * 1.3 + id * 0.2) % 1,
    a = -Math.PI / 3,
    dx = Math.cos(a) * r * 1.3,
    dy = Math.sin(a) * r * 1.3;
  // The band pattern repeats; three copies shifted by o cover the gradient while it scrolls.
  const stops = [];
  for (let k = -1; k <= 1; k++)
    for (const [p, m] of bands) {
      const at = (p + k + o) / 1.6 + 0.2;
      if (at >= 0 && at <= 1) stops.push(/** @type {[number, string]} */ ([at, mix(base, m)]));
    }
  stops.sort((p, q) => p[0] - q[0]);
  const grad = g.createLinearGradient(x - dx, y - dy, x + dx, y + dy);
  for (const [at, col] of stops) grad.addColorStop(at, col);
  return grad;
}
/** @param {Charge} c */
function uber({ base, id, x, y, rad, t }) {
  fillDot(x, y, rad, metal(ctx, UBER_BANDS, { base, id, t, x, y, r: rad }));
  const phase = ((t + id * 0.37) % SWEEP_S) / SWEEP_S / SWEEP_ON;
  if (phase < 1) {
    const at = x - rad * 1.6 + phase * rad * 3.2,
      sweep = ctx.createLinearGradient(at - rad * 0.6, y - rad, at + rad * 0.6, y + rad);
    sweep.addColorStop(0, "rgba(255,255,255,0)");
    sweep.addColorStop(0.5, "rgba(255,255,255,0.85)");
    sweep.addColorStop(1, "rgba(255,255,255,0)");
    fillDot(x, y, rad, sweep);
  }
  const rim = ctx.createRadialGradient(x, y, 0, x, y, rad);
  rim.addColorStop(0, "rgba(255,255,255,0)");
  rim.addColorStop(0.72, "rgba(255,255,255,0)");
  rim.addColorStop(0.92, "rgba(255,255,255,0.7)");
  rim.addColorStop(1, "rgba(0,0,0,0.45)");
  fillDot(x, y, rad, rim);
  const ball = ctx.createRadialGradient(x - rad * 0.3, y - rad * 0.35, rad * 0.1, x, y, rad);
  ball.addColorStop(0, "rgba(255,255,255,0.55)");
  ball.addColorStop(0.5, "rgba(255,255,255,0)");
  fillDot(x, y, rad, ball);
}

// A pulsing team-coloured halo behind an übered player's dot.
/** @param {Player} p @param {number} x @param {number} y @param {number} rad */
export function drawUberGlow(p, x, y, rad) {
  if (!uberShown(p)) return;
  const base = TEAM[p.team] ?? TEAM.none,
    pulse = 0.5 + 0.5 * Math.sin((performance.now() / 1000) * 6 + p.id);
  const r = rad * (2.6 + 0.5 * pulse),
    halo = ctx.createRadialGradient(x, y, rad, x, y, r);
  halo.addColorStop(0, mix(base, 0.2, 0.95));
  halo.addColorStop(0.45, mix(base, 0.05, 0.55));
  halo.addColorStop(1, mix(base, 0, 0));
  ctx.fillStyle = halo;
  ctx.beginPath();
  ctx.arc(x, y, r, 0, TAU);
  ctx.fill();
  // A glowing ring outside the health ring.
  ctx.save();
  ctx.shadowColor = base;
  ctx.shadowBlur = rad * (0.8 + 0.8 * pulse);
  ctx.strokeStyle = mix(base, 0.15);
  ctx.lineWidth = Math.max(2, rad * 0.28);
  ctx.beginPath();
  ctx.arc(x, y, rad * 1.5, 0, TAU);
  ctx.stroke();
  ctx.restore();
}

/** Whether the über look is on for p in this frame (not in a wearing-off flicker). @param {Player} p */
export const uberShown = p => p.ubered === "uber" && !uberFlickerOff(p);
// The medic's cross fills the dot; on über it shrinks to leave the metal room to show.
/** @param {Player} p */
export const uberIconScale = p => (p.class === "medic" && uberShown(p) ? 0.75 : 1);

/** @type {WeakMap<HTMLCanvasElement, HTMLCanvasElement>} */
const darkIcons = new WeakMap();
/** @type {HTMLCanvasElement | null} */
let chromeScratch = null;
/** @param {HTMLCanvasElement} img @param {(g: CanvasRenderingContext2D) => void} paint */
function recolour(img, paint, into = document.createElement("canvas")) {
  into.width = img.width;
  into.height = img.height;
  const g = /** @type {CanvasRenderingContext2D} */ (into.getContext("2d"));
  g.drawImage(img, 0, 0);
  g.globalCompositeOperation = "source-in";
  paint(g);
  return into;
}
// An übered player's class icon: chrome, the light bands flowing through it like the metal round
// it, inside a thin dark outline that keeps it apart from the dot.
/** @param {HTMLCanvasElement} img @param {Player} p @param {number} x @param {number} y @param {number} s */
export function drawUberIcon(img, p, x, y, s) {
  let dark = darkIcons.get(img);
  if (!dark)
    darkIcons.set(
      img,
      (dark = recolour(img, g => {
        g.fillStyle = "#0b0c0e";
        g.fillRect(0, 0, img.width, img.height);
      })),
    );
  const w = Math.max(0.8, s * 0.04);
  for (let i = 0; i < 8; i++) {
    const a = (i / 8) * TAU;
    ctx.drawImage(dark, x - s / 2 + Math.cos(a) * w, y - s / 2 + Math.sin(a) * w, s, s);
  }
  const n = img.width,
    t = performance.now() / 1000;
  chromeScratch = recolour(
    img,
    g => {
      g.fillStyle = metal(g, ICON_BANDS, {
        base: TEAM[p.team] ?? TEAM.none,
        id: p.id,
        t,
        x: n / 2,
        y: n / 2,
        r: n / 2,
      });
      g.fillRect(0, 0, n, n);
    },
    chromeScratch ?? undefined,
  );
  ctx.drawImage(chromeScratch, x - s / 2, y - s / 2, s, s);
}

// Kritz like the patient's crackling weapon: two or three arcs of team-coloured lightning with a
// white core flicker across a darkened dot, new ones 14 times a second.
/** @param {Charge} c */
function kritz({ base, id, x, y, rad, t }) {
  fillDot(x, y, rad, mix(base, -0.35));
  const f = Math.floor(t * 14),
    glow = mix(base, 0.45);
  ctx.lineJoin = "round";
  for (let j = 0; j < 3; j++) if (hash(f + j * 31 + id) > 0.3) bolt(x, y, rad, f * 3 + j * 17 + id * 101, glow);
}
// A jagged arc from one point on the rim to another, picked by seed.
/** @param {number} x @param {number} y @param {number} rad @param {number} seed @param {string} glow */
function bolt(x, y, rad, seed, glow) {
  const a0 = hash(seed) * TAU,
    a1 = a0 + Math.PI * (0.6 + hash(seed + 1) * 0.8),
    [x0, y0, x1, y1] = [x + Math.cos(a0) * rad, y + Math.sin(a0) * rad, x + Math.cos(a1) * rad, y + Math.sin(a1) * rad],
    len = Math.hypot(x1 - x0, y1 - y0) || 1,
    [nx, ny] = [(y0 - y1) / len, (x1 - x0) / len];
  const path = () => {
    ctx.beginPath();
    ctx.moveTo(x0, y0);
    for (let i = 1; i < 6; i++) {
      const f = i / 6,
        j = (hash(seed + i * 7) - 0.5) * rad * 0.7;
      ctx.lineTo(x0 + (x1 - x0) * f + nx * j, y0 + (y1 - y0) * f + ny * j);
    }
    ctx.lineTo(x1, y1);
  };
  path();
  ctx.shadowColor = glow;
  ctx.shadowBlur = rad * 0.6;
  ctx.strokeStyle = glow;
  ctx.lineWidth = Math.max(1.5, rad * 0.16);
  ctx.stroke();
  ctx.shadowBlur = 0;
  path();
  ctx.strokeStyle = "#fff";
  ctx.lineWidth = Math.max(0.8, rad * 0.06);
  ctx.stroke();
}

// Quick-fix's fast healing: white rings run from the centre out to the edge, three at a time.
/** @param {Charge} c */
function quickfix({ id, x, y, rad, t }) {
  for (let i = 0; i < 3; i++) {
    const k = (t * 1.5 + i / 3 + id * 0.1) % 1;
    ctx.beginPath();
    ctx.arc(x, y, rad * (0.15 + k * 0.85), 0, TAU);
    ctx.strokeStyle = `rgba(255,255,255,${0.85 * (1 - k)})`;
    ctx.lineWidth = Math.max(1.2, rad * 0.14);
    ctx.stroke();
  }
}

// The vaccinator's charges (vaccinator_bullet, _blast, _fire): like the game's shield, a glossy
// bubble in the team's colour, its edge pulsing. The resistance is the badge (drawResistBadge).
/** @param {Charge} c */
function vaccinator({ base, id, x, y, rad, t }) {
  const p = 0.5 + 0.5 * Math.sin(t * 3 + id),
    edge = mix(base, 0.55);
  const bubble = ctx.createRadialGradient(x, y, 0, x, y, rad);
  bubble.addColorStop(0, mix(edge, 0, 0));
  bubble.addColorStop(0.6, mix(edge, 0, 0.1));
  bubble.addColorStop(0.88, mix(edge, 0, 0.55 + 0.25 * p));
  bubble.addColorStop(1, mix(edge, 0, 0.9));
  fillDot(x, y, rad, bubble);
  ctx.beginPath();
  ctx.arc(x, y, rad * 0.7, Math.PI * 1.1, Math.PI * 1.45);
  ctx.strokeStyle = "rgba(255,255,255,0.85)";
  ctx.lineWidth = Math.max(1.2, rad * 0.13);
  ctx.lineCap = "round";
  ctx.stroke();
}

/** @type {Map<string, (c: Charge) => void>} */
const CHARGES = new Map([
  ["uber", uber],
  ["kritz", kritz],
  ["quickfix", quickfix],
]);

// The vaccinator's resistance as the game shows it, by shape: a bullet, an explosion or a flame,
// white in a dark badge on the dot's top-right rim, over the class icon.
/** @param {Player} p @param {number} x @param {number} y @param {number} rad */
export function drawResistBadge(p, x, y, rad) {
  const kind = p.ubered?.startsWith("vaccinator_") && p.ubered.slice(11);
  if (!kind) return;
  // The glyph g is a little larger than the badge's radius s, filling most of it.
  const s = Math.max(5, rad * 0.44),
    g = s * 1.42,
    bx = x + rad * 0.72,
    by = y - rad * 0.72;
  ctx.save();
  ctx.beginPath();
  ctx.arc(bx, by, s, 0, TAU);
  ctx.fillStyle = "#111";
  ctx.fill();
  ctx.strokeStyle = mix(TEAM[p.team] ?? TEAM.none, 0.4);
  ctx.lineWidth = Math.max(1, s * 0.18);
  ctx.stroke();
  ctx.fillStyle = "#fff";
  ctx.beginPath();
  if (kind === "bullet") {
    ctx.moveTo(bx - g * 0.22, by + g * 0.55);
    ctx.lineTo(bx - g * 0.22, by - g * 0.1);
    ctx.quadraticCurveTo(bx, by - g * 0.8, bx + g * 0.22, by - g * 0.1);
    ctx.lineTo(bx + g * 0.22, by + g * 0.55);
    ctx.closePath();
  } else if (kind === "blast") {
    for (let i = 0; i < 16; i++) {
      const a = (i / 16) * TAU,
        r = g * (i % 2 ? 0.3 : 0.62);
      ctx.lineTo(bx + Math.cos(a) * r, by + Math.sin(a) * r);
    }
    ctx.closePath();
  } else {
    // A flame: a tall tip leaning right, a smaller tongue on the left, a round base.
    ctx.moveTo(bx, by + g * 0.62);
    ctx.bezierCurveTo(bx - g * 0.55, by + g * 0.62, bx - g * 0.55, by + g * 0.05, bx - g * 0.28, by - g * 0.28);
    ctx.quadraticCurveTo(bx - g * 0.2, by + g * 0.02, bx - g * 0.04, by + g * 0.08);
    ctx.quadraticCurveTo(bx - g * 0.12, by - g * 0.4, bx + g * 0.14, by - g * 0.72);
    ctx.bezierCurveTo(bx + g * 0.22, by - g * 0.35, bx + g * 0.62, by - g * 0.1, bx + g * 0.46, by + g * 0.26);
    ctx.bezierCurveTo(bx + g * 0.38, by + g * 0.55, bx + g * 0.2, by + g * 0.62, bx, by + g * 0.62);
  }
  ctx.fill();
  ctx.restore();
}

// Jarate, Mad Milk and bleeding fill the bottom of the dot like a liquid (side by side when there
// are several) and burning licks up from it, under the class icon.
const LIQUIDS = [
  ["Urine", "#e8c81f"],
  ["MadMilk", "#dfe3e6"],
  ["Bleeding", "#5c0610"],
];
/** @param {string[]} conds @param {number} id @param {number} x @param {number} y @param {number} rad */
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
      /** @param {number} k @param {string} fill */
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
/** @param {string[]} conds @param {number} id @param {number} x @param {number} y @param {number} rad */
export function drawStatus(conds, id, x, y, rad) {
  const t = performance.now() / 1000,
    r = Math.max(3, rad * 0.4),
    lw = Math.max(1, r * 0.2);
  const has = /** @param {string} n */ n => conds.includes(n);
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
