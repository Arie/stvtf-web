// A voice badge on a player's dot: while they talk, a waveform that is taller, wider and hotter the
// louder they are (a shout stands out in a fight), as on their HUD card (panel/speakers.js); while
// muted, a speaker crossed out (a click on the card's mutes them: the badge is too small, on a dot
// that moves). See core/voice.js.
import { ctx } from "../core/core.js";
import { isMuted, isTalking, loudnessOf, voiceOn, waveTime } from "../core/voice.js";
/** @typedef {import("../../types/stream").Player} Player */

// The badge's size against the dot's, and the least it is in pixels. It sits on the dot's right,
// a little low (half past three): clear of the vaccinator's resist (upper right), a carried
// building (upper left), marked for death and stun stars (above), and the name (below).
const SIZE = 0.62,
  MIN_PX = 8,
  RIGHT = 0.95,
  LOW = 0.35,
  // How faint a muted player's badge is while they are silent.
  QUIET = 0.4,
  // How much bigger the badge of someone at their loudest is.
  GROW = 0.45,
  // The waveform's bars: how tall each gets against the others, and how fast they move.
  BARS = [0.5, 0.85, 1, 0.7, 0.45],
  BEAT = 0.011;

// How loud someone is taken to be before anything of them played.
export const UNKNOWN = 0.35;

// How loud each player's badge shows them, eased.
/** @type {Map<number, number>} */
const eased = new Map();

// The badges of `dots`' players who talk or are muted.
/** @param {{ p: Player; x: number; y: number; rad: number }[]} dots */
export function drawVoiceBadges(dots) {
  if (!voiceOn()) return;
  for (const { p, x, y, rad } of dots) {
    const muted = Boolean(p.steamid) && isMuted(p.steamid),
      talking = isTalking(p.id);
    if (!muted && !talking) continue;
    const level = talking ? ease(eased, p.id, loudnessOf(p.id) ?? UNKNOWN) : 0;
    if (!talking) eased.delete(p.id);
    const r = Math.max(MIN_PX, rad * SIZE) * (muted ? 1 : 1 + GROW * level),
      bx = x + rad * RIGHT,
      by = y + rad * LOW;
    // Faint while a muted player is silent, lit while they talk.
    ctx.save();
    ctx.globalAlpha = talking ? 1 : QUIET;
    if (muted) badge(bx, by, r);
    else wave(bx, by, r, level, waveTime(performance.now()));
    ctx.restore();
  }
}

// `level` as shown for `id`, after what `shown` has of them last: up at once, down slowly.
/** @param {Map<number, number>} shown @param {number} id @param {number} level */
export function ease(shown, id, level) {
  const was = shown.get(id) ?? 0;
  const now = level > was ? level : was + (level - was) * 0.12;
  shown.set(id, now);
  return now;
}

// The waveform's bars at `level` (0 to 1) and time `t` (ms): each one's height, 0 to 1 of the
// disc's. Taller the louder, and never still.
/** @param {number} level @param {number} t */
export function bars(level, t) {
  return BARS.map((w, i) => 0.14 + 0.86 * level * w * (0.62 + 0.38 * Math.sin(t * BEAT * (1 + i * 0.37) + i * 1.9)));
}

// White for a murmur, through yellow to red for a shout.
/** @param {number} level */
export function heat(level) {
  const stops = [
    [255, 255, 255],
    [255, 214, 74],
    [255, 86, 54],
  ];
  const at = Math.min(1, Math.max(0, level)) * 2,
    i = Math.min(1, Math.floor(at)),
    f = at - i;
  const [r, g, b] = stops[i].map((c, k) => Math.round(c + (stops[i + 1][k] - c) * f));
  return `rgb(${r} ${g} ${b})`;
}

// A waveform on a dark disc, ringed in its colour once loud.
/** @param {number} x @param {number} y @param {number} r @param {number} level @param {number} t */
function wave(x, y, r, level, t) {
  const colour = heat(level);
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = "#0b0c0ee0";
  ctx.fill();
  ctx.lineWidth = Math.max(1, r * 0.14);
  ctx.strokeStyle = level > 0.55 ? colour : "#000";
  ctx.stroke();
  const heights = bars(level, t),
    span = r * 1.2,
    step = span / heights.length,
    w = Math.max(1, step * 0.62);
  ctx.fillStyle = colour;
  heights.forEach((h, i) => {
    const bh = Math.max(1, h * r * 1.5),
      bx = x - span / 2 + step * (i + 0.5) - w / 2;
    ctx.fillRect(bx, y - bh / 2, w, bh);
  });
  ctx.restore();
}

// A speaker on a dark disc, crossed out in red: muted.
/** @param {number} x @param {number} y @param {number} r */
function badge(x, y, r) {
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = "#0b0c0ee0";
  ctx.fill();
  ctx.lineWidth = Math.max(1, r * 0.14);
  ctx.strokeStyle = "#e05a4f";
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
  ctx.fillStyle = "#9aa0a6";
  ctx.fill();
  ctx.lineWidth = Math.max(1, u * 0.9);
  ctx.beginPath();
  ctx.moveTo(x + 2 * u, y - 2 * u);
  ctx.lineTo(x + 4.6 * u, y + 2 * u);
  ctx.moveTo(x + 4.6 * u, y - 2 * u);
  ctx.lineTo(x + 2 * u, y + 2 * u);
  ctx.stroke();
  ctx.restore();
}
