// A speaker badge on a player's dot: while they talk, and while muted (crossed out), as on their
// HUD card; a click on it mutes or unmutes them (panel/speakers.js). See core/voice.js.
import { ctx } from "../core/core.js";
import { isMuted, isTalking } from "../core/voice.js";
/** @typedef {import("../../types/stream").Player} Player */

// The badge's size against the dot's, and the least it is in pixels. It sits on the dot's right,
// a little low (half past three): clear of the vaccinator's resist (upper right), a carried
// building (upper left), marked for death and stun stars (above), and the name (below).
const SIZE = 0.5,
  MIN_PX = 6,
  RIGHT = 0.95,
  LOW = 0.35,
  // How faint a muted player's badge is while they are silent.
  QUIET = 0.4;

/** @type {{ id: number; steamid: string; x: number; y: number; r: number }[]} */
let shown = [];

// The badges of `dots`' players who talk or are muted.
/** @param {{ p: Player; x: number; y: number; rad: number }[]} dots */
export function drawVoiceBadges(dots) {
  shown = [];
  for (const { p, x, y, rad } of dots) {
    const muted = Boolean(p.steamid) && isMuted(p.steamid),
      talking = isTalking(p.id);
    if (!muted && !talking) continue;
    const r = Math.max(MIN_PX, rad * SIZE),
      bx = x + rad * RIGHT,
      by = y + rad * LOW;
    // Faint while a muted player is silent, lit while they talk.
    ctx.save();
    ctx.globalAlpha = talking ? 1 : QUIET;
    badge(bx, by, r, muted);
    ctx.restore();
    shown.push({ id: p.id, steamid: p.steamid, x: bx, y: by, r });
  }
}

// The player whose badge is at x, y (canvas pixels), if one is.
/** @param {number} x @param {number} y */
export function voiceBadgeAt(x, y) {
  return shown.find(b => Math.hypot(b.x - x, b.y - y) <= b.r) ?? null;
}

// A speaker on a dark disc: sound coming out of it, or crossed out in red.
/** @param {number} x @param {number} y @param {number} r @param {boolean} muted */
function badge(x, y, r, muted) {
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = "#0b0c0ee0";
  ctx.fill();
  ctx.lineWidth = Math.max(1, r * 0.14);
  ctx.strokeStyle = muted ? "#e05a4f" : "#fff";
  ctx.stroke();
  const u = r / 7;
  ctx.beginPath();
  ctx.moveTo(x - 3.6 * u, y - 1.4 * u);
  ctx.lineTo(x - 2 * u, y - 1.4 * u);
  ctx.lineTo(x + 0.6 * u, y - 3.6 * u);
  ctx.lineTo(x + 0.6 * u, y + 3.6 * u);
  ctx.lineTo(x - 2 * u, y + 1.4 * u);
  ctx.lineTo(x - 3.6 * u, y + 1.4 * u);
  ctx.closePath();
  ctx.fillStyle = muted ? "#9aa0a6" : "#fff";
  ctx.fill();
  ctx.lineWidth = Math.max(1, u * 0.9);
  ctx.beginPath();
  if (muted) {
    ctx.moveTo(x + 2 * u, y - 2 * u);
    ctx.lineTo(x + 4.6 * u, y + 2 * u);
    ctx.moveTo(x + 4.6 * u, y - 2 * u);
    ctx.lineTo(x + 2 * u, y + 2 * u);
    ctx.strokeStyle = "#e05a4f";
  } else {
    ctx.arc(x + 1, y, 2.6 * u, -Math.PI / 4, Math.PI / 4);
    ctx.moveTo(x + 1 + 4.4 * u * Math.cos(-Math.PI / 4), y + 4.4 * u * Math.sin(-Math.PI / 4));
    ctx.arc(x + 1, y, 4.4 * u, -Math.PI / 4, Math.PI / 4);
    ctx.strokeStyle = "#fff";
  }
  ctx.stroke();
  ctx.restore();
}
