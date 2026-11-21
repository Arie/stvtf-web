// The listed cameras' moves: on the map, and set off from here.
import { $, ctx, uiScale } from "../core/core.js";
import { toScreen } from "../map/view.js";
import { drawTrack } from "./compose.js";
import { at, fromKept, rowsOf, startOf } from "./moves.js";
import { note } from "./tools.js";
/** @typedef {[number, number, number]} Vec3 */
/** @typedef {import("./moves.js").Move} Move */
/** @typedef {{ pos: Vec3; aim: Vec3; move?: import("./moves.js").Kept }} Listed */

const MARK_PX = 9,
  NOW = "#7cff8a";
// The one set off from here, and since when the room has it (not yet: asked).
/** @type {{ key: string; secs: number; from: number | null; changed: () => void } | null} */
let off = null;
export const offKey = () => off?.key ?? null;
// The relay's camera is no longer on the move set off from here.
export function offEnded() {
  if (!off || off.from === null) return;
  const { changed } = off;
  off = null;
  changed();
}

// A listed camera's move as the page works with it, made once.
/** @type {WeakMap<object, Move>} */
const going = new WeakMap();
/** @param {Listed} c */
export function goingOf(c) {
  if (!c.move) return undefined;
  const made = going.get(c.move) ?? fromKept(c.move);
  going.set(c.move, made);
  return made;
}

// Where a move ends: a ring, with a short line the way it looks there.
/** @param {{ pos: Vec3; aim: Vec3 }} end @param {string} colour */
function endMark(end, colour) {
  const [x, y] = toScreen(end.pos[0], end.pos[1]),
    [ax, ay] = toScreen(end.aim[0], end.aim[1]),
    r = MARK_PX * uiScale(),
    far = Math.hypot(ax - x, ay - y) || 1,
    reach = Math.min(far, 3 * r);
  ctx.save();
  ctx.strokeStyle = colour;
  ctx.lineWidth = 2 * uiScale();
  ctx.beginPath();
  ctx.moveTo(x + ((ax - x) / far) * r, y + ((ay - y) / far) * r);
  ctx.lineTo(x + ((ax - x) / far) * reach, y + ((ay - y) / far) * reach);
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, 2 * Math.PI);
  ctx.stroke();
  ctx.restore();
}

// How far into the move set off from here it is, its ▶ counting down. None once there.
function offNow() {
  if (!off || off.from === null) return null;
  const t = (performance.now() - off.from) / 1000,
    b = /** @type {HTMLElement | null} */ ($("cam-list").querySelector(`[data-play="${off.key}"]`));
  if (t >= off.secs) {
    const { changed } = off;
    off = null;
    changed();
    return null;
  }
  const left = `${Math.ceil(off.secs - t)}s`;
  if (b && b.textContent !== left) b.textContent = left;
  b?.style.setProperty("--done", String(t / off.secs));
  return t;
}

// A listed camera's move on the map: its way, its end, and where it is when set off from here.
/** @param {Listed} c @param {string} key @param {string} colour */
export function drawGoing(c, key, colour) {
  const move = goingOf(c);
  if (!move) return;
  drawTrack({ pos: c.pos, aim: c.aim, move });
  endMark(at(c, move, move.secs), colour);
  const t = off?.key === key ? offNow() : null;
  if (t === null) return;
  const now = at(c, move, t).pos,
    [x, y] = toScreen(now[0], now[1]);
  ctx.save();
  ctx.fillStyle = NOW;
  ctx.beginPath();
  ctx.arc(x, y, 5 * uiScale(), 0, 2 * Math.PI);
  ctx.fill();
  ctx.restore();
}

// Sets the relay's camera off on a listed camera's move; its ▶ waits, then counts down.
/** @param {Listed} c @param {string} key @param {(body: object) => Promise<Response>} send @param {() => void} changed */
export async function setOff(c, key, send, changed) {
  const move = goingOf(c);
  if (!move) return;
  /** @type {NonNullable<typeof off>} */
  const mine = { key, secs: move.secs, from: null, changed };
  off = mine;
  changed();
  const r = await send({ ...startOf(c), path: rowsOf(c, move) }).catch(() => null);
  if (off !== mine) return;
  if (r?.ok) mine.from = performance.now();
  else {
    off = null;
    note(r ? await r.text() : "No answer");
  }
  changed();
}
