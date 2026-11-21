// Steering the camera being placed from its picture: a drag looks round, a click aims at what is
// under it, the wheel and WASD move it (Q down, E or Space up), Shift faster. With the pointer
// held by the picture (a flight being flown) the mouse looks round as it moves, as in the game.
import { $ } from "../core/core.js";
import { pointAt } from "./preview.js";
/** @typedef {[number, number, number]} Vec3 */
/** @typedef {{ pos: Vec3; aim: Vec3 }} Spot */

// Radians a pixel of drag turns it, map units a second and a wheel notch move it, how much faster
// with Shift, how far a press may stray and still be a click, and how far the pointer held by the
// picture moves at once at most, in pixels.
const TURN = 0.004,
  SPEED = 500,
  NOTCH = 100,
  FAST = 3,
  CLICK_PX = 4,
  LEAP_PX = 400,
  STEEP = 1.55;
// A key's way: forward, right, up.
/** @type {Record<string, Vec3>} */
const WAYS = { w: [1, 0, 0], s: [-1, 0, 0], d: [0, 1, 0], a: [0, -1, 0], e: [0, 0, 1], " ": [0, 0, 1], q: [0, 0, -1] };

/** @param {Spot} c @returns {[Vec3, number]} */
function facing({ pos, aim }) {
  const d = [aim[0] - pos[0], aim[1] - pos[1], aim[2] - pos[2]],
    far = Math.hypot(d[0], d[1], d[2]) || 1;
  return [[d[0] / far, d[1] / far, d[2] / far], far];
}
// Where it looks once turned right and down by these, as far off as before.
/** @param {Spot} c @param {number} right @param {number} down @returns {Vec3} */
function turned(c, right, down) {
  const [f, far] = facing(c),
    yaw = Math.atan2(f[1], f[0]) - right,
    pitch = Math.max(-STEEP, Math.min(STEEP, Math.asin(f[2]) - down)),
    flat = Math.cos(pitch) * far;
  return [
    Math.round(c.pos[0] + Math.cos(yaw) * flat),
    Math.round(c.pos[1] + Math.sin(yaw) * flat),
    Math.round(c.pos[2] + Math.sin(pitch) * far),
  ];
}
// It and what it looks at, moved forward, right and up by these.
/** @param {Spot} c @param {Vec3} by @returns {Spot} */
function moved(c, [fwd, right, up]) {
  const [f] = facing(c),
    side = Math.hypot(f[0], f[1]) || 1,
    r = [f[1] / side, -f[0] / side],
    d = [f[0] * fwd + r[0] * right, f[1] * fwd + r[1] * right, f[2] * fwd + up];
  return {
    pos: [c.pos[0] + d[0], c.pos[1] + d[1], c.pos[2] + d[2]],
    aim: [c.aim[0] + d[0], c.aim[1] + d[1], c.aim[2] + d[2]],
  };
}
/** @param {Spot} c @param {Spot} to */
function put(c, to) {
  c.pos = /** @type {Vec3} */ (to.pos.map(Math.round));
  c.aim = /** @type {Vec3} */ (to.aim.map(Math.round));
}

// The keys held move it every frame, the way it looks then: the mouse may turn it meanwhile. Where
// it is is kept unrounded from frame to frame, unless something else moved it.
/** @param {() => Spot | null} spot @param {() => void} changed @param {() => void} done */
function keys(spot, changed, done) {
  const view = $("cam-view"),
    /** @type {Set<string>} */
    held = new Set();
  /** @type {Vec3 | null} */
  let at = null;
  let [last, fast] = [0, false];
  const frame = (/** @type {number} */ now) => {
    const c = spot();
    if (!c || !held.size) {
      [at, last] = [null, 0];
      return done();
    }
    const far = (last ? (now - last) / 1000 : 0) * SPEED * (fast ? FAST : 1),
      by = /** @type {Vec3} */ ([0, 1, 2].map(i => [...held].reduce((sum, k) => sum + WAYS[k][i], 0) * far));
    const near = at?.every((v, i) => Math.abs(v - c.pos[i]) <= 1) ? at : c.pos,
      off = /** @type {Vec3} */ (near.map((v, i) => v - c.pos[i])),
      to = moved({ pos: near, aim: /** @type {Vec3} */ (c.aim.map((v, i) => v + off[i])) }, by);
    at = to.pos;
    put(c, to);
    last = now;
    changed();
    requestAnimationFrame(frame);
  };
  view.addEventListener("keydown", e => {
    const key = e.key.toLowerCase();
    fast = e.shiftKey;
    if (!(key in WAYS) || e.ctrlKey || e.metaKey) return;
    e.preventDefault();
    e.stopPropagation();
    if (!held.size) requestAnimationFrame(frame);
    held.add(key);
  });
  view.addEventListener("keyup", e => {
    fast = e.shiftKey;
    held.delete(e.key.toLowerCase());
  });
  view.addEventListener("blur", () => held.clear());
}

/** @param {() => Spot | null} spot @param {() => void} changed @param {() => void} done */
export function flyTool(spot, changed, done) {
  const view = $("cam-view");
  /** @type {{ x: number; y: number; from: Spot; dragged: boolean } | null} */
  let press = null;
  const held = () => document.pointerLockElement === view;
  view.addEventListener("mousemove", e => {
    const c = spot();
    // Not a leap: a browser may say one as it takes hold of the pointer.
    if (!c || !held() || Math.hypot(e.movementX, e.movementY) > LEAP_PX) return;
    c.aim = turned(c, e.movementX * TURN, e.movementY * TURN);
    changed();
  });
  view.addEventListener("pointerdown", e => {
    const c = spot();
    if (!c || e.button !== 0 || held()) return;
    view.focus();
    view.setPointerCapture(e.pointerId);
    press = { x: e.clientX, y: e.clientY, from: { pos: [...c.pos], aim: [...c.aim] }, dragged: false };
  });
  view.addEventListener("pointermove", e => {
    const c = spot();
    if (!c || !press) return;
    const [dx, dy] = [e.clientX - press.x, e.clientY - press.y];
    press.dragged ||= Math.hypot(dx, dy) > CLICK_PX;
    if (!press.dragged) return;
    // From where it is now: the keys may be moving it meanwhile.
    const { from } = press,
      to = turned(from, dx * TURN, dy * TURN);
    c.aim = /** @type {Vec3} */ (to.map((v, i) => v - from.pos[i] + c.pos[i]));
    changed();
  });
  view.addEventListener("pointerup", e => {
    const [c, was] = [spot(), press];
    press = null;
    if (!c || !was) return;
    // A click aims at what is under it.
    if (!was.dragged) {
      const hit = pointAt(e.offsetX / view.clientWidth, e.offsetY / view.clientHeight);
      if (hit) c.aim = hit;
    }
    changed();
    done();
  });
  view.addEventListener(
    "wheel",
    e => {
      const c = spot();
      if (!c) return;
      e.preventDefault();
      put(c, moved(c, [-Math.sign(e.deltaY) * NOTCH * (e.shiftKey ? FAST : 1), 0, 0]));
      changed();
      done();
    },
    { passive: false },
  );
  keys(spot, changed, done);
}
