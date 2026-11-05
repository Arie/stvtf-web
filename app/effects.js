import { TEAM, OVERLAY, optOn, overlayUnit, uiScale, ctx, state, teamColour } from "./core.js";
import { toScreen, screenAngle, px, playerRad } from "./view.js";
import { SHOW_SHOTS, SHOW_BUILDINGS } from "./players.js";
/** @typedef {import("../types/stream").Player} Player */
/** @typedef {import("../types/stream").GameEvent} GameEvent */
/** @typedef {import("../types/page").Effect} Effect */

// World units: a hitscan tracer's length, and a sentry's range (its shot when the target is unknown).
const SHOT_TRACER = 900,
  SENTRY_RANGE = 1100;
// deaths: the main view's brief kill marks (the overlay keeps an X where a player fell instead).
const SHOW_KILLS = optOn("deaths");
// Health and repairs float up in this green.
const HEAL_GREEN = "#6fe08a";
const SHOT_EFFECTS = new Set(["shot", "boom", "airblast"]),
  BUILDING_EFFECTS = new Set(["sentryshot", "upgrade", "repair"]);

/** @param {string | undefined} team */
function tracerColor(team) {
  if (team === "red") return "#ffb3a8";
  if (team === "blue") return "#b3dcff";
  return "#ffe9a8";
}
/** @param {GameEvent} ev */
function damageColor(ev) {
  if (ev.crit) return "#7dff6a";
  if (ev.minicrit) return "#ffd04a";
  return "#ff5a4a";
}
/** @param {string} kind */
function hitColor(kind) {
  if (kind === "blood") return "#c4302b";
  if (kind === "sparks") return "#ffd36a";
  return "#e8f4ff";
}

// Each kind of effect, drawn k of the way through its life (with the alpha already set from it).
/** @param {Effect} e */
function drawShot(e) {
  const [x, y] = toScreen(e.ev.origin[0], e.ev.origin[1]);
  const a = screenAngle(e.ev.yaw),
    len = SHOT_TRACER * Math.cos((e.ev.pitch * Math.PI) / 180) * state.view.scale;
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(x + Math.cos(a) * len, y + Math.sin(a) * len);
  ctx.strokeStyle = tracerColor(e.team);
  ctx.lineWidth = 1.2;
  ctx.stroke();
}
/** @param {Effect} e @param {number} k @param {Map<number, Player>} byId */
function drawSentryShot(e, k, byId) {
  const [x, y] = toScreen(e.from[0], e.from[1]);
  const t = byId.get(e.target);
  let [tx, ty] = [0, 0];
  if (t?.pos) [tx, ty] = toScreen(t.pos[0], t.pos[1]);
  else {
    const a = screenAngle(e.aim || 0);
    [tx, ty] = [x + Math.cos(a) * SENTRY_RANGE * state.view.scale, y + Math.sin(a) * SENTRY_RANGE * state.view.scale];
  }
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(tx, ty);
  ctx.strokeStyle = "#ffe9a8";
  ctx.lineWidth = 1.5;
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(x, y, px(14, 5), 0, Math.PI * 2);
  ctx.fillStyle = "#ffd36a";
  ctx.fill();
}
/** @param {Effect} e @param {number} k */
function drawBoom(e, k) {
  const [x, y] = toScreen(e.ev.origin[0], e.ev.origin[1]);
  // A quick flash, smaller than the blast radius: the crater stays behind as the lasting mark.
  ctx.beginPath();
  ctx.arc(
    x,
    y,
    Math.max(px(65, 4), OVERLAY ? overlayUnit() * 0.8 : 0) * (0.3 + 0.7 * (1 - (1 - k) ** 3)),
    0,
    Math.PI * 2,
  );
  ctx.fillStyle = "#ffa04028";
  ctx.fill();
  ctx.strokeStyle = "#ffb050";
  ctx.lineWidth = 1.5;
  ctx.stroke();
}
/** @param {Effect} e @param {number} k @param {Map<number, Player>} byId */
function drawDamage(e, k, byId) {
  if (OVERLAY) return;
  const pos = byId.get(e.ev.victim)?.pos || e.pos;
  if (!pos) return;
  const [x, y] = toScreen(pos[0], pos[1]);
  ctx.font = `bold ${Math.round((e.ev.crit ? 15 : 12) * uiScale())}px system-ui`;
  ctx.textAlign = "center";
  ctx.textBaseline = "bottom";
  ctx.fillStyle = damageColor(e.ev);
  ctx.strokeStyle = "#000";
  ctx.lineWidth = 3;
  const ty = y - playerRad() - 4 - k * 22;
  ctx.strokeText("-" + e.ev.amount, x, ty);
  ctx.fillText("-" + e.ev.amount, x, ty);
}
/** @param {Effect} e */
function drawKill(e) {
  if (OVERLAY || !SHOW_KILLS) return;
  const v = e.ev.victim_pos;
  if (!v) return;
  const [x, y] = toScreen(v[0], v[1]),
    s = 7;
  if (e.ev.attacker_pos && e.ev.attacker !== e.ev.victim) {
    const [ax, ay] = toScreen(e.ev.attacker_pos[0], e.ev.attacker_pos[1]);
    ctx.setLineDash([4, 4]);
    ctx.beginPath();
    ctx.moveTo(ax, ay);
    ctx.lineTo(x, y);
    ctx.strokeStyle = "#fff8";
    ctx.lineWidth = 1;
    ctx.stroke();
    ctx.setLineDash([]);
  }
  ctx.strokeStyle = teamColour(e.team, "#fff");
  ctx.lineWidth = 3;
  ctx.beginPath();
  ctx.moveTo(x - s, y - s);
  ctx.lineTo(x + s, y + s);
  ctx.moveTo(x + s, y - s);
  ctx.lineTo(x - s, y + s);
  ctx.stroke();
}
/** @param {Effect} e @param {number} k */
function drawSpawn(e, k) {
  if (!e.pos) return;
  const [x, y] = toScreen(e.pos[0], e.pos[1]);
  ctx.beginPath();
  ctx.arc(x, y, playerRad() + k * 18, 0, Math.PI * 2);
  ctx.strokeStyle = teamColour(e.team, "#fff");
  ctx.lineWidth = 2;
  ctx.stroke();
}
/** @param {Effect} e @param {number} k @param {Map<number, Player>} byId */
function drawAirblast(e, k, byId) {
  // A puff of air widening out in front of the pyro.
  const pl = byId.get(e.id);
  if (!pl?.pos) return;
  const [x, y] = toScreen(pl.pos[0], pl.pos[1]),
    a = screenAngle(pl.yaw ?? 0);
  const rad = playerRad(),
    r = Math.max((40 + 220 * k) * state.view.scale, rad * (1.4 + 2 * k));
  ctx.beginPath();
  ctx.arc(x, y, rad + 3, a + 0.7, a - 0.7, true);
  ctx.arc(x, y, r, a - 0.7, a + 0.7);
  ctx.closePath();
  ctx.fillStyle = "#e8f4ff30";
  ctx.fill();
  ctx.beginPath();
  ctx.arc(x, y, r, a - 0.7, a + 0.7);
  ctx.strokeStyle = "#e8f4ff";
  ctx.lineWidth = Math.max(3, rad * 0.35);
  ctx.lineCap = "round";
  ctx.stroke();
  ctx.lineCap = "butt";
}
/** @param {Effect} e @param {number} k @param {Map<number, Player>} byId */
function drawHeal(e, k, byId) {
  const pos = byId.get(e.id)?.pos || e.ev.pos;
  if (!pos) return;
  const [x, y] = toScreen(pos[0], pos[1]);
  ctx.font = `bold ${Math.round(12 * uiScale())}px system-ui`;
  ctx.textAlign = "center";
  ctx.textBaseline = "bottom";
  ctx.fillStyle = HEAL_GREEN;
  ctx.strokeStyle = "#000";
  ctx.lineWidth = 3;
  const ty = y - playerRad() - 4 - k * 22,
    tx = x + playerRad() * 0.9,
    txt = "+" + e.ev.amount;
  ctx.strokeText(txt, tx, ty);
  ctx.fillText(txt, tx, ty);
}
/** @param {Effect} e @param {number} k */
function drawUpgrade(e, k) {
  // A building going up a level: a burst of sparks, like the engineer's wrench.
  const [x, y] = toScreen(e.pos[0], e.pos[1]),
    r0 = px(30, 8),
    r1 = r0 + px(80, 18) * k;
  ctx.strokeStyle = "#ffe27a";
  ctx.lineWidth = 2;
  ctx.lineCap = "round";
  for (let i = 0; i < 10; i++) {
    const a = (i / 10) * Math.PI * 2 + 0.3;
    ctx.beginPath();
    ctx.moveTo(x + Math.cos(a) * (r0 + (r1 - r0) * 0.5), y + Math.sin(a) * (r0 + (r1 - r0) * 0.5));
    ctx.lineTo(x + Math.cos(a) * r1, y + Math.sin(a) * r1);
    ctx.stroke();
  }
  ctx.lineCap = "butt";
}
/** @param {Effect} e @param {number} k */
function drawRepair(e, k) {
  const [x, y] = toScreen(e.pos[0], e.pos[1]);
  ctx.font = `bold ${Math.round(12 * uiScale())}px system-ui`;
  ctx.textAlign = "center";
  ctx.textBaseline = "bottom";
  ctx.fillStyle = HEAL_GREEN;
  ctx.strokeStyle = "#000";
  ctx.lineWidth = 3;
  const ty = y - px(30, 12) - 4 - k * 22,
    txt = "+" + e.ev.amount;
  ctx.strokeText(txt, x, ty);
  ctx.fillText(txt, x, ty);
}
/** @param {Effect} e @param {number} k */
function drawHit(e, k) {
  const [x, y] = toScreen(e.ev.pos[0], e.ev.pos[1]);
  const seed = Math.abs(Math.round(e.ev.pos[0] * 7 + e.ev.pos[1] * 13)),
    r = Math.max(5 * uiScale(), px(40, 5));
  const blood = e.ev.kind === "blood";
  ctx.strokeStyle = hitColor(e.ev.kind);
  ctx.fillStyle = ctx.strokeStyle;
  ctx.lineWidth = 1.5;
  ctx.lineCap = "round";
  for (let i = 0; i < (blood ? 6 : 4); i++) {
    const a = ((seed * (i + 3)) % 628) / 100,
      d = r * (blood ? 0.4 + k : 0.5 + 1.2 * k);
    if (blood) {
      ctx.beginPath();
      ctx.arc(x + Math.cos(a) * d, y + Math.sin(a) * d, Math.max(1.5, r * 0.18 * (1 - k)), 0, Math.PI * 2);
      ctx.fill();
    } else {
      ctx.beginPath();
      ctx.moveTo(x + Math.cos(a) * d, y + Math.sin(a) * d);
      ctx.lineTo(x + Math.cos(a) * (d + r * 0.4), y + Math.sin(a) * (d + r * 0.4));
      ctx.stroke();
    }
  }
  ctx.lineCap = "butt";
}
/** @param {Effect} e @param {number} k */
function drawCapBreak(e, k) {
  const [x, y] = toScreen(e.pos[0], e.pos[1]),
    half = OVERLAY ? overlayUnit() * 1.2 : px(110, 11);
  ctx.beginPath();
  ctx.arc(x, y, half * (1.1 + 1.2 * k), 0, Math.PI * 2);
  ctx.strokeStyle = "#ffb050";
  ctx.lineWidth = 3;
  ctx.stroke();
}
/** @param {Effect} e @param {number} k @param {Map<number, Player>} byId */
function drawUberPop(e, k, byId) {
  const m = byId.get(e.id);
  if (!m?.pos) return;
  const [x, y] = toScreen(m.pos[0], m.pos[1]);
  ctx.beginPath();
  ctx.arc(x, y, playerRad() + 6 + k * 50, 0, Math.PI * 2);
  ctx.strokeStyle = TEAM[e.team];
  ctx.lineWidth = 4;
  ctx.stroke();
}
const EFFECTS = new Map([
  ["shot", drawShot],
  ["sentryshot", drawSentryShot],
  ["boom", drawBoom],
  ["dmg", drawDamage],
  ["kill", drawKill],
  ["spawn", drawSpawn],
  ["airblast", drawAirblast],
  ["heal", drawHeal],
  ["upgrade", drawUpgrade],
  ["repair", drawRepair],
  ["hit", drawHit],
  ["capbreak", drawCapBreak],
  ["uber", drawUberPop],
]);

/** @param {Map<number, Player>} byId */
export function drawEffects(byId) {
  const now = performance.now();
  state.effects = state.effects.filter(e => now - e.t0 < e.life);
  for (const e of state.effects) {
    if ((!SHOW_SHOTS && SHOT_EFFECTS.has(e.kind)) || (!SHOW_BUILDINGS && BUILDING_EFFECTS.has(e.kind))) continue;
    const k = (now - e.t0) / e.life;
    ctx.globalAlpha = 1 - k;
    EFFECTS.get(e.kind)?.(e, k, byId);
  }
  ctx.globalAlpha = 1;
}
