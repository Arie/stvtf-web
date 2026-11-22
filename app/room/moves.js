// Where a camera's move starts: the camera's position, and for a recorded flight the direction it
// was flown in, not the camera's current aim. Takes a listed camera (path as rows) or one being placed.
/** @param {Spot & { move?: { path?: (Sample | number[])[] } }} c @returns {Spot} */
export function startOf(c) {
  const first = c.move?.path?.[0];
  if (!first) return { pos: c.pos, aim: c.aim };
  const [pos, aim] = Array.isArray(first) ? [first.slice(1, 4), first.slice(4, 7)] : [first.pos, first.aim];
  return { pos: c.pos, aim: /** @type {Vec3} */ (aim.map((v, i) => Math.round(v + c.pos[i] - pos[i]))) };
}

// A camera's move: to an end frame over some seconds, straight or in an arc round what it looks at,
// or a recorded flight. Stored with the camera in the form `toKept` makes.
/** @typedef {[number, number, number]} Vec3 */
/** @typedef {{ pos: Vec3; aim: Vec3 }} Spot */
/** @typedef {{ t: number; pos: Vec3; aim: Vec3 }} Sample */
/**
 * @typedef {object} Move
 * @property {Spot} end
 * @property {number} secs
 * @property {"straight" | "arc"} way
 * @property {number} [turn] How far an arc turns round the aim, in radians. May be more than half a turn.
 * @property {Sample[]} [path] A recorded flight, smoothed from `raw`, which took `took` seconds.
 * @property {Sample[]} [raw]
 * @property {number} [took]
 * @property {number} [even]
 */

// A move as the site stores it: a flight's path as rows (as in `rowsOf`), without the raw recording.
/** @typedef {Omit<Move, "path" | "raw"> & { path?: number[][] }} Kept */
const tenth = (/** @type {number} */ v) => Math.round(v * 10) / 10;
/** @param {Move} move @returns {Kept} */
export function toKept(move) {
  const { path, end, ...rest } = move;
  delete rest.raw;
  return {
    ...rest,
    end: { pos: /** @type {Vec3} */ (end.pos.map(Math.round)), aim: /** @type {Vec3} */ (end.aim.map(Math.round)) },
    ...(path && { path: path.map(s => [Math.round(s.t * 1000) / 1000, ...[...s.pos, ...s.aim].map(tenth)]) }),
  };
}
/** @param {Kept} kept @returns {Move} */
export function fromKept(kept) {
  const { path, ...rest } = kept,
    vec = (/** @type {number[]} */ r, /** @type {number} */ i) => /** @type {Vec3} */ (r.slice(i, i + 3));
  return {
    ...rest,
    end: copy(kept.end),
    ...(path && { path: path.map(r => ({ t: r[0], pos: vec(r, 1), aim: vec(r, 4) })) }),
  };
}

// A move as the relay takes it: one row per sample (seconds in, position, aim). With no move, one
// row for where the camera stays.
/** @param {Spot} from @param {Move} [move] */
export function rowsOf(from, move) {
  const n = move ? Math.ceil(move.secs * RATE) : 0,
    rows = [];
  for (let i = 0; i <= n; i++) {
    const t = move && n ? (move.secs * i) / n : 0,
      spot = move ? at(from, move, t) : from;
    rows.push([Math.round(t * 1000) / 1000, ...[...spot.pos, ...spot.aim].map(tenth)]);
  }
  return rows;
}

// Samples per second of a move's rows and a flight's path, and the turn one press of an orbit button adds.
const RATE = 30;
export const ORBIT = Math.PI / 4;

const mix = (/** @type {Vec3} */ a, /** @type {Vec3} */ b, /** @type {number} */ s) =>
  /** @type {Vec3} */ (a.map((v, i) => v + (b[i] - v) * s));
const within = (/** @type {number} */ v) => Math.min(1, Math.max(0, v));
const copy = (/** @type {Spot} */ c) => ({
  pos: /** @type {Vec3} */ ([...c.pos]),
  aim: /** @type {Vec3} */ ([...c.aim]),
});
// A camera's position relative to its aim: horizontal distance, angle round it, height above it.
const round = (/** @type {Spot} */ c) => ({
  far: Math.hypot(c.pos[0] - c.aim[0], c.pos[1] - c.aim[1]),
  at: Math.atan2(c.pos[1] - c.aim[1], c.pos[0] - c.aim[0]),
  up: c.pos[2] - c.aim[2],
});

// The angle from `from` round to `end`: the short way, or with whole turns added to come nearest `like`.
/** @param {Spot} from @param {Spot} end @param {number} [like] */
export function turnTo(from, end, like = 0) {
  const short = Math.atan2(Math.sin(round(end).at - round(from).at), Math.cos(round(end).at - round(from).at));
  return short + 2 * Math.PI * Math.round((like - short) / (2 * Math.PI));
}

// Where a flight is `t` seconds into its recording.
/** @param {Sample[]} path @param {number} t @returns {Spot} */
function along(path, t) {
  const i = Math.min(path.length - 2, Math.max(0, Math.floor(t * RATE))),
    [a, b] = [path[i], path[i + 1] ?? path[i]],
    s = within(t * RATE - i);
  return { pos: mix(a.pos, b.pos, s), aim: mix(a.aim, b.aim, s) };
}

// Where a camera that starts at `from` is `t` seconds into its move. A move between frames eases in and out.
/** @param {Spot} from @param {Move} move @param {number} t @returns {Spot} */
export function at(from, move, t) {
  const u = within(t / move.secs);
  // A flight is shifted to start at the camera's position and keeps the direction it was flown in.
  if (move.path) {
    const on = along(move.path, u * (move.took ?? move.secs)),
      [first] = move.path,
      by = (/** @type {Vec3} */ v) => /** @type {Vec3} */ (v.map((x, i) => x + from.pos[i] - first.pos[i]));
    return { pos: by(on.pos), aim: by(on.aim) };
  }
  const s = u * u * (3 - 2 * u),
    aim = mix(from.aim, move.end.aim, s);
  if (move.way !== "arc") return { pos: mix(from.pos, move.end.pos, s), aim };
  const [a, b] = [round(from), round(move.end)],
    turn = a.at + (move.turn ?? turnTo(from, move.end)) * s,
    far = a.far + (b.far - a.far) * s;
  return {
    pos: [aim[0] + Math.cos(turn) * far, aim[1] + Math.sin(turn) * far, aim[2] + a.up + (b.up - a.up) * s],
    aim,
  };
}

// `end` moved an eighth of a turn round its aim, one way (1) or the other (-1).
/** @param {Spot} end @param {number} way @returns {Spot} */
export function orbited(end, way) {
  const { pos, aim } = copy(end),
    d = [pos[0] - aim[0], pos[1] - aim[1]],
    [cos, sin] = [Math.cos(ORBIT * way), Math.sin(ORBIT * way)];
  return { pos: [aim[0] + d[0] * cos - d[1] * sin, aim[1] + d[0] * sin + d[1] * cos, pos[2]], aim };
}

// The recorded samples as a flight: resampled at a steady rate, then smoothed over `even` seconds.
/** @param {Sample[]} raw @param {number} even @param {number} [secs] @returns {Move | null} */
export function flight(raw, even, secs) {
  const took = raw.length ? raw[raw.length - 1].t : 0;
  if (took < 0.5) return null;
  /** @type {Sample[]} */
  const paced = [];
  let i = 0;
  for (let n = 0; n <= Math.ceil(took * RATE); n++) {
    const t = Math.min(took, n / RATE);
    while (i < raw.length - 2 && raw[i + 1].t < t) i++;
    const [a, b] = [raw[i], raw[i + 1] ?? raw[i]],
      s = b.t > a.t ? within((t - a.t) / (b.t - a.t)) : 0;
    paced.push({ t, pos: mix(a.pos, b.pos, s), aim: mix(a.aim, b.aim, s) });
  }
  const wide = Math.max(0.01, even) * RATE,
    reach = Math.ceil(wide * 2);
  const path = paced.map((e, n) => {
    /** @type {Spot} */
    const sum = { pos: [0, 0, 0], aim: [0, 0, 0] };
    let total = 0;
    for (let k = -reach; k <= reach; k++) {
      const o = paced[Math.min(paced.length - 1, Math.max(0, n + k))],
        w = Math.exp(-0.5 * (k / wide) ** 2);
      total += w;
      for (const part of /** @type {const} */ (["pos", "aim"]))
        for (let x = 0; x < 3; x++) sum[part][x] += o[part][x] * w;
    }
    const share = (/** @type {Vec3} */ v) => /** @type {Vec3} */ (v.map(x => x / total));
    return { t: e.t, pos: share(sum.pos), aim: share(sum.aim) };
  });
  return { end: copy(path[path.length - 1]), secs: secs ?? took, way: "straight", path, raw, took, even };
}
