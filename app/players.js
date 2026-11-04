import { TEAM, CLASS, icon, OVERLAY, optOn, uiScale, ctx, state } from "./core.js";
import { layerOn } from "./layers.js";
import { toScreen, screenAngle, playerRad, drawText } from "./view.js";
import { drawUberRing, drawStatusInside, drawStatus } from "./status.js";
/** @typedef {import("../types/stream").Player} Player */
/** @typedef {{ p: Player, x: number, y: number, rad: number, col: string, cloaked: boolean }} Dot */

// The medigun beam like TF2's: two strands twisting around a soft glow in the team colour, with
// sparkles drifting from the medic to the patient; thicker and brighter while the charge is out.
const BEAM_LIGHT = { red: "#ffc2b8", blue: "#c2e2ff" };
/** @param {Player[]} players @param {Map<number, Player>} byId */
export function drawHealBeams(players, byId) {
  const t = performance.now() / 1000;
  for (const m of players) {
    const g = m.medigun;
    if (!m.alive || !g || g.target == null) continue;
    const pt = byId.get(g.target);
    if (!pt || !pt.alive) continue;
    const beam = beamCurve(m, pt);
    if (beam) drawHealBeam(m, beam, g.releasing, t);
  }
}

// A quadratic curve leaving the medigun along the medic's aim, as TF2's beam does; aimed
// straight at the patient it still bends a little. Null when they are on top of each other.
/** @param {Player} m @param {Player} pt */
function beamCurve(m, pt) {
  const [x0, y0] = toScreen(m.pos[0], m.pos[1]),
    [x1, y1] = toScreen(pt.pos[0], pt.pos[1]);
  const len = Math.hypot(x1 - x0, y1 - y0);
  if (len < 2) return null;
  const a = screenAngle(m.yaw),
    ux = (x1 - x0) / len,
    uy = (y1 - y0) / len;
  let cx = x0 + Math.cos(a) * len * 0.5,
    cy = y0 + Math.sin(a) * len * 0.5;
  const side = (cx - x0) * -uy + (cy - y0) * ux;
  if (Math.abs(side) < len * 0.08) {
    const k = (side < 0 ? -1 : 1) * len * 0.08 - side;
    cx += -uy * k;
    cy += ux * k;
  }
  const at = f => [
    (1 - f) * (1 - f) * x0 + 2 * f * (1 - f) * cx + f * f * x1,
    (1 - f) * (1 - f) * y0 + 2 * f * (1 - f) * cy + f * f * y1,
  ];
  const normal = f => {
    const tx = 2 * (1 - f) * (cx - x0) + 2 * f * (x1 - cx),
      ty = 2 * (1 - f) * (cy - y0) + 2 * f * (y1 - cy),
      n = Math.hypot(tx, ty) || 1;
    return [-ty / n, tx / n];
  };
  return { x0, y0, x1, y1, cx, cy, len, at, normal };
}

/** @param {Player} m */
function drawHealBeam(m, beam, hot, t) {
  const { x0, y0, x1, y1, cx, cy, len } = beam;
  const col = TEAM[m.team] || "#fff",
    light = BEAM_LIGHT[m.team] || "#fff";
  const amp = (hot ? 6 : 4) * uiScale(),
    wave = 70 * uiScale(),
    steps = Math.max(10, Math.min(90, Math.round(len / 6)));
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.beginPath();
  ctx.moveTo(x0, y0);
  ctx.quadraticCurveTo(cx, cy, x1, y1);
  ctx.strokeStyle = col;
  ctx.globalAlpha = hot ? 0.5 : 0.3;
  ctx.lineWidth = (hot ? 11 : 7) * uiScale();
  ctx.stroke();
  ctx.globalAlpha = hot ? 0.9 : 0.6;
  ctx.lineWidth = (hot ? 4 : 2.5) * uiScale();
  ctx.stroke();
  // The strands meet the medic and the patient, and twist along the beam towards the patient.
  ctx.strokeStyle = light;
  ctx.lineWidth = (hot ? 2 : 1.4) * uiScale();
  ctx.globalAlpha = hot ? 1 : 0.85;
  for (const phase of [0, Math.PI]) {
    ctx.beginPath();
    for (let i = 0; i <= steps; i++) {
      const f = i / steps,
        [ax, ay] = beam.at(f),
        [nx, ny] = beam.normal(f);
      const off = amp * Math.sin(Math.PI * f) * Math.sin(((f * len) / wave) * Math.PI * 2 - t * 9 + phase);
      if (i) ctx.lineTo(ax + nx * off, ay + ny * off);
      else ctx.moveTo(ax + nx * off, ay + ny * off);
    }
    ctx.stroke();
  }
  drawBeamSparks(beam, amp, hot, t);
  ctx.restore();
}

function drawBeamSparks(beam, amp, hot, t) {
  ctx.fillStyle = "#fff";
  const sparks = Math.max(2, Math.round(beam.len / 45));
  for (let k = 0; k < sparks; k++) {
    const f = (k / sparks + t * (hot ? 0.9 : 0.6)) % 1,
      [ax, ay] = beam.at(f),
      [nx, ny] = beam.normal(f);
    const off = amp * 0.9 * Math.sin(Math.PI * f) * Math.sin(k * 2.4 + t * 5);
    ctx.globalAlpha = Math.sin(Math.PI * f) * (hot ? 1 : 0.8);
    ctx.beginPath();
    ctx.arc(ax + nx * off, ay + ny * off, (hot ? 2.2 : 1.6) * uiScale(), 0, Math.PI * 2);
    ctx.fill();
  }
}

// The class icons are SVGs, which Firefox rasterizes again on every draw: each size is drawn once.
// Their art sits in a padded box (some fill only half of it), so each is cropped to a square
// around what it draws: the art itself fills the size asked for.
const ICON_SIZE = 1.55,
  iconSprites = new Map(),
  iconBoxes = new Map();
function iconBox(cls, img) {
  let box = iconBoxes.get(cls);
  if (box) return box;
  const w = img.naturalWidth,
    h = img.naturalHeight,
    n = 128;
  box = { x: 0, y: 0, side: Math.max(w, h) };
  try {
    const c = document.createElement("canvas");
    c.width = c.height = n;
    const g = c.getContext("2d", { willReadFrequently: true });
    g.drawImage(img, 0, 0, n, n);
    const a = g.getImageData(0, 0, n, n).data;
    let x0 = n,
      y0 = n,
      x1 = -1,
      y1 = -1;
    for (let y = 0; y < n; y++)
      for (let x = 0; x < n; x++) {
        if (a[(y * n + x) * 4 + 3] < 16) continue;
        x0 = Math.min(x0, x);
        x1 = Math.max(x1, x);
        y0 = Math.min(y0, y);
        y1 = Math.max(y1, y);
      }
    if (x1 >= 0) {
      const side = Math.max(x1 - x0, y1 - y0) + 1,
        cx = (x0 + x1 + 1) / 2,
        cy = (y0 + y1 + 1) / 2;
      box = { x: ((cx - side / 2) * w) / n, y: ((cy - side / 2) * h) / n, side: (side * Math.max(w, h)) / n };
    }
  } catch {
    /* unreadable (cross-origin): the whole box */
  }
  iconBoxes.set(cls, box);
  return box;
}
function iconSprite(cls, size) {
  const img = icon(cls);
  if (!img) return null;
  const n = Math.max(1, Math.round(size * (window.devicePixelRatio || 1))),
    key = `${cls} ${n}`;
  let c = iconSprites.get(key);
  if (!c) {
    if (iconSprites.size > 64) iconSprites.clear();
    const b = iconBox(cls, img);
    c = document.createElement("canvas");
    c.width = c.height = n;
    c.getContext("2d").drawImage(img, b.x, b.y, b.side, b.side, 0, 0, n, n);
    iconSprites.set(key, c);
  }
  return c;
}

// What the query string can turn off, or on where a view leaves it out.
const SHOW_NAMES = optOn("names", !OVERLAY),
  SHOW_HEALTH = optOn("health", !OVERLAY),
  SHOW_FACING = optOn("facing");
export const SHOW_SHOTS = optOn("shots", !OVERLAY),
  SHOW_BUILDINGS = optOn("buildings", !OVERLAY);
// deaths: the X kept where a player fell (the overlay's); the main view's brief kill marks are in effects.js.
const SHOW_FALLEN = optOn("deaths", OVERLAY);

/** @param {Player[]} players */
export function drawPlayers(players) {
  /** @type {Dot[]} */
  const alive = [];
  // The followed player is drawn last, so their dot and name are on top.
  players = [...players].sort((a, b) => Number(a.id === state.follow) - Number(b.id === state.follow));
  for (const p of players) {
    if (p.team !== "red" && p.team !== "blue") continue;
    if (!p.alive) {
      // The overlay keeps an X where each dead player fell until they respawn, like a CS radar.
      if (SHOW_FALLEN && p.pos) drawFallen(p);
      continue;
    }
    const [x, y] = toScreen(p.pos[0], p.pos[1]);
    alive.push({ p, x, y, rad: playerRad(), col: TEAM[p.team], cloaked: p.conds?.includes("Stealthed") });
  }
  // In passes, so in a clump every class icon sits on top of the other players' discs.
  if (SHOW_FACING) for (const d of alive) drawPointer(d);
  for (const d of alive) drawDisc(d);
  // Flames over the dots, so they show in a fight, but under the class icons.
  if (SHOW_SHOTS) for (const { p, x, y, rad } of alive) if (p.flames) drawFlames(p, x, y, rad, screenAngle(p.yaw));
  for (const d of alive) drawIcon(d);
  if (layerOn("effects")) for (const d of alive) drawStatusOver(d);
  if (SHOW_NAMES) for (const d of alive) drawName(d);
  ctx.globalAlpha = 1;
}

/** @param {Player} p */
function drawFallen(p) {
  const [x, y] = toScreen(p.pos[0], p.pos[1]),
    s = playerRad() * 0.6;
  ctx.save();
  ctx.globalAlpha = 0.75;
  ctx.lineCap = "round";
  ctx.strokeStyle = "#000";
  ctx.lineWidth = 5;
  ctx.beginPath();
  ctx.moveTo(x - s, y - s);
  ctx.lineTo(x + s, y + s);
  ctx.moveTo(x + s, y - s);
  ctx.lineTo(x - s, y + s);
  ctx.stroke();
  ctx.strokeStyle = TEAM[p.team];
  ctx.lineWidth = 2.5;
  ctx.stroke();
  ctx.restore();
}

/** @param {Dot} d */
function drawPointer({ p, x, y, rad, col, cloaked }) {
  ctx.globalAlpha = cloaked ? 0.35 : 1;
  // A pointer on the disc where they look; how wide they see is up to their client. demos.tf's
  // dart (0.8 of the disc's radius long, 0.6 of that to each side, notched base) from the health ring.
  const a = screenAngle(p.yaw),
    base = rad + 3,
    len = rad * 0.8,
    half = len * 0.6;
  /** @returns {[number, number]} */
  const at = (f, l) => [x + Math.cos(a) * f - Math.sin(a) * l, y + Math.sin(a) * f + Math.cos(a) * l];
  ctx.beginPath();
  for (const [f, l] of [
    [base, -half],
    [base + len * 0.2, 0],
    [base, half],
    [base + len, 0],
  ])
    ctx.lineTo(...at(f, l));
  ctx.closePath();
  ctx.fillStyle = col;
  ctx.fill();
  ctx.lineWidth = OVERLAY ? Math.max(1.5, rad * 0.16) : 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
}

/** @param {Dot} d */
function drawDisc({ p, x, y, rad, col, cloaked }) {
  ctx.globalAlpha = cloaked ? 0.35 : 1;
  if (p.ubered) drawUberRing(p, x, y, rad, col);
  ctx.beginPath();
  ctx.arc(x, y, rad, 0, Math.PI * 2);
  ctx.fillStyle = col;
  ctx.fill();
  if (layerOn("effects") && p.conds?.length) drawStatusInside(p.conds, p.id, x, y, rad);
  ctx.beginPath();
  ctx.arc(x, y, rad, 0, Math.PI * 2);
  ctx.lineWidth = OVERLAY ? Math.max(1.5, rad * 0.16) : 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
  if (SHOW_HEALTH) drawHealth(p, x, y, rad);
}

function healthColor(frac) {
  if (frac > 0.5) return "#9fd27f";
  if (frac > 0.25) return "#e0c040";
  return "#e05a40";
}
/** @param {Player} p */
function drawHealth(p, x, y, rad) {
  const max = p.max_hp || 1,
    frac = Math.max(0, Math.min(1, p.hp / max));
  const ring = (from, to, color) => {
    ctx.beginPath();
    ctx.arc(x, y, rad + 2.5, -Math.PI / 2 + Math.PI * 2 * from, -Math.PI / 2 + Math.PI * 2 * to);
    ctx.strokeStyle = color;
    ctx.stroke();
  };
  ctx.lineWidth = OVERLAY ? Math.max(2.5, rad * 0.26) : 2.5;
  ring(0, frac, healthColor(frac));
  // Overheal laps the green ring in blue, full at the medigun cap: +50%, rounded down to 5.
  const over = Math.min(1, (p.hp - max) / (Math.floor((max * 1.5) / 5) * 5 - max || 1));
  if (over > 0) ring(0, over, "#1f4fe0");
}

/** @param {Dot} d */
function drawIcon({ p, x, y, rad, cloaked }) {
  ctx.globalAlpha = cloaked ? 0.35 : 1;
  // The art's longer side, cropped of its padding: about 1.3x as big as before, still on the disc.
  const s = rad * ICON_SIZE,
    img = iconSprite(p.class, s);
  if (img) {
    ctx.drawImage(img, x - s / 2, y - s / 2, s, s);
  } else {
    ctx.fillStyle = "#fff";
    ctx.font = `bold ${Math.round(Math.max(9, rad * 0.9))}px system-ui`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(CLASS[p.class] || "?", x, y + 0.5);
  }
  if (SHOW_BUILDINGS && p.carrying) drawCarried(p, x, y, rad);
  if (p.id === state.follow) {
    ctx.beginPath();
    ctx.arc(x, y, rad + 7, 0, Math.PI * 2);
    ctx.strokeStyle = "#fff";
    ctx.lineWidth = 2;
    ctx.stroke();
  }
}

// The building an engineer carries, as a small badge like the ones on the map.
/** @param {Player} p */
function drawCarried(p, x, y, rad) {
  const bs = Math.max(12, rad),
    bx = x - rad * 0.95,
    by = y - rad * 0.95;
  ctx.beginPath();
  ctx.roundRect(bx - bs / 2, by - bs / 2, bs, bs, 2);
  ctx.fillStyle = TEAM[p.team] || TEAM.none;
  ctx.fill();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.fillStyle = "#fff";
  ctx.font = `800 ${Math.round(bs * 0.62)}px system-ui`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(p.carrying[0].toUpperCase(), bx, by + 0.5);
}

/** @param {Dot} d */
function drawStatusOver({ p, x, y, rad, cloaked }) {
  if (!p.conds?.length) return;
  ctx.globalAlpha = cloaked ? 0.35 : 1;
  drawStatus(p.conds, p.id, x, y, rad);
}

/** @param {Dot} d */
function drawName({ p, x, y, rad, cloaked }) {
  ctx.globalAlpha = cloaked ? 0.35 : 1;
  const size = OVERLAY ? Math.round(Math.max(11, rad * 0.95)) : Math.round(12 * uiScale());
  drawText(
    [...p.name].slice(0, OVERLAY ? 12 : 18).join(""),
    { font: `700 ${size}px system-ui`, fill: "#e8e8e8", outline: "#0b0c0e" },
    x,
    y + rad + 4,
    true,
  );
}

function flameColor(p) {
  if (!p.flames.crit) return "255,150,40";
  return p.team === "red" ? "255,90,60" : "90,160,255";
}
// A firing flamethrower: a flickering cone about as long as the flames reach. Crit flames take
// the team's colour, and burn brighter while they hit someone.
/** @param {Player} p */
function drawFlames(p, x, y, rad, a) {
  const t = performance.now() / 1000,
    len = Math.max(350 * state.view.scale, rad * (OVERLAY ? 4 : 3)) * (0.92 + 0.08 * Math.sin(t * 23 + p.id));
  const hot = flameColor(p);
  const g = ctx.createRadialGradient(x, y, rad + 3, x, y, len);
  g.addColorStop(0, `rgba(${hot},${p.flames.hit ? 0.85 : 0.6})`);
  g.addColorStop(1, `rgba(${hot},0)`);
  ctx.beginPath();
  ctx.arc(x, y, rad + 3, a + 0.3, a - 0.3, true);
  ctx.arc(x, y, len, a - 0.3, a + 0.3);
  ctx.closePath();
  ctx.fillStyle = g;
  ctx.fill();
}
