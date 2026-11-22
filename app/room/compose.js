// Composes the move of the camera being placed: a start and an end frame, the way between and how
// long it takes, or a recorded flight. The move is saved with its camera.
import { $, ctx, uiScale } from "../core/core.js";
import { toScreen } from "../map/view.js";
import { ORBIT, at, flight, orbited, rowsOf, startOf, turnTo } from "./moves.js";
import { snapshot } from "./preview.js";
import { blind } from "./sight.js";
import { note } from "./tools.js";
/** @typedef {import("./moves.js").Move} Move */
/** @typedef {import("./moves.js").Spot} Spot */
/** @typedef {{ name: string; pos: Spot["pos"]; aim: Spot["aim"]; room?: boolean; move?: Move }} Camera */
/** @typedef {{ camera: () => Camera | null; changed: () => void; done: () => void; panel: () => void; sight: () => import("./sight.js").Grid | null; send: (body: object) => Promise<Response> }} Host */

// Seconds a new move takes, seconds a played move holds at its end, seconds a new flight is smoothed
// over, line segments in the drawn track, and ms between updates of the frame thumbnails.
const SECS = 8,
  HOLD = 1,
  EVEN = 0.25,
  TRACK = 48,
  THUMB_MS = 300,
  // The countdown before a flight and the longest flight, in seconds.
  COUNT = 3,
  LONGEST = 60;
const COLOUR = { track: "#ffd23f", now: "#7cff8a", bad: "#ff6b5a" };

/** @type {Host} */
let host;
// The frame being placed, when the move began playing, the second of the move scrubbed to, the
// flight being recorded, and whether the frame thumbnails are out of date.
/** @type {"start" | "end"} */
let frame = "start";
/** @type {number | null} */
let playing = null;
/** @type {number | null} */
let scrub = null;
/** @type {{ from: number; flown: import("./moves.js").Sample[] } | null} */
let flying = null;
// When the countdown before a flight started.
/** @type {number | null} */
let counting = null;
let [stale, taken] = [true, 0];
// The move playing was sent to the relay too.
let sent = false;
const input = (/** @type {string} */ id) => /** @type {HTMLInputElement} */ ($(id));

export const onEnd = () => frame === "end";
// A flight's countdown is running: the camera stays put until the flight begins.
export const counted = () => counting !== null;
// The frame being placed: the camera's own spot, or its move's end.
/** @param {Camera} c @returns {Spot} */
export const spotOf = c => (frame === "end" && c.move && !c.move.path ? c.move.end : c);
// Another camera, or none, is now being placed: back to the start frame, nothing playing.
export function begun() {
  [frame, playing, scrub, stale] = ["start", null, null, true];
  grounded();
}
// A frame was moved: an arc keeps turning the same way round, and playback stops.
/** @param {Camera} c */
export function moved(c) {
  if (c.move && !c.move.path) c.move.turn = turnTo(c, c.move.end, c.move.turn);
  // A flight keeps the direction it was flown in: its start can be moved, not turned.
  if (c.move?.path && !flying) c.aim = startOf(c).aim;
  if (!flying) [playing, scrub] = [null, null];
  stale = true;
}

// What the picture shows: the move where it is playing or scrubbed to, else the frame being placed.
/** @param {Camera} c @returns {Spot} */
export function shown(c) {
  const now = performance.now(),
    move = c.move;
  $("cam-run").classList.toggle("on", sent && playing !== null);
  if (flying || counting !== null) badge(c, now);
  if (flying) flying.flown.push({ t: (now - flying.from) / 1000, pos: [...c.pos], aim: [...c.aim] });
  if (stale && now - taken > THUMB_MS) thumbs(c, now);
  if (!move) return c;
  if (playing !== null) {
    const t = (now - playing) / 1000;
    if (t <= move.secs + HOLD) {
      input("cam-scrub").value = String(Math.round(Math.min(1, t / move.secs) * 1000));
      return at(c, move, t);
    }
    playing = null;
    host.panel();
  }
  return scrub === null ? spotOf(c) : at(c, move, scrub);
}
/** @param {Camera} c @param {number} now */
function thumbs(c, now) {
  const start = snapshot(c);
  if (!start) return;
  $("cam-start").style.backgroundImage = `url(${start})`;
  $("cam-end").style.backgroundImage = c.move ? `url(${snapshot(at(c, c.move, c.move.secs))})` : "";
  [stale, taken] = [false, now];
}

// Draws the move on the map. With `now` (the camera being placed): red where it would see nothing,
// and a dot where it is now.
/** @param {Pick<Camera, "pos" | "aim" | "move">} c @param {Spot} [now] */
export function drawTrack(c, now) {
  const move = c.move,
    sight = now ? host.sight() : null;
  if (!move) return;
  ctx.save();
  ctx.lineWidth = 2 * uiScale();
  let was = toScreen(c.pos[0], c.pos[1]);
  for (let n = 1; n <= TRACK; n++) {
    const spot = at(c, move, (move.secs * n) / TRACK),
      to = toScreen(spot.pos[0], spot.pos[1]);
    ctx.strokeStyle = sight && blind(sight, spot) ? COLOUR.bad : COLOUR.track;
    ctx.beginPath();
    ctx.moveTo(was[0], was[1]);
    ctx.lineTo(to[0], to[1]);
    ctx.stroke();
    was = to;
  }
  if (now && (playing !== null || scrub !== null)) {
    const [x, y] = toScreen(now.pos[0], now.pos[1]);
    ctx.fillStyle = COLOUR.now;
    ctx.beginPath();
    ctx.arc(x, y, 5 * uiScale(), 0, 2 * Math.PI);
    ctx.fill();
  }
  ctx.restore();
}

// Updates the move's controls in the panel and in the bar under the picture.
/** @param {Camera} c */
export function composing(c) {
  const move = c.move,
    flown = Boolean(move?.path);
  $("cam-start").classList.toggle("on", frame === "start");
  $("cam-end").classList.toggle("on", frame === "end");
  $("cam-end").textContent = move ? "End" : "+ End";
  for (const id of ["cam-play", "cam-scrub", "cam-secs", "cam-secs-s"]) $(id).hidden = !move;
  $("cam-play").textContent = playing === null ? "▶" : "■";
  const waits = counting === null ? "● Record (R)" : "Cancel (R)";
  $("cam-rec").textContent = flying ? "■ Stop (R)" : waits;
  $("cam-still").hidden = !move;
  $("cam-way-row").hidden = !move || flown;
  $("cam-moves").hidden = flown;
  // Smoothing starts from the raw recording, which a saved flight no longer has.
  $("cam-even-row").hidden = !move?.raw;
  if (!move) return;
  input("cam-secs").value = String(Math.round(move.secs * 10) / 10);
  /** @type {HTMLSelectElement} */ ($("cam-way")).value = move.way;
  input("cam-even").value = String(move.even ?? EVEN);
}

/** @param {Camera} c @returns {Move} */
function framed(c) {
  if (!c.move || c.move.path)
    c.move = { end: { pos: [...c.pos], aim: [...c.aim] }, secs: c.move?.secs ?? SECS, way: "straight", turn: 0 };
  return c.move;
}
// The Record button or R: starts the countdown, cancels it, or stops the flight being recorded.
/** @param {Camera} c */
function record(c) {
  if (flying) return landed(c);
  if (counting !== null) return grounded();
  [frame, playing, scrub, counting] = ["start", null, null, performance.now()];
  const view = $("cam-view");
  view.focus();
  // Not every browser grants pointer lock; without it a drag still looks round.
  Promise.resolve(view.requestPointerLock?.()).catch(() => {});
}
// The flight is over: it becomes the camera's move, and the camera goes back to where it began.
/** @param {Camera} c */
function landed(c) {
  const flown = flying?.flown ?? [],
    made = flight(flown, EVEN);
  grounded();
  if (!made) return;
  c.move = made;
  c.pos = /** @type {Spot["pos"]} */ (flown[0].pos.map(Math.round));
  c.aim = /** @type {Spot["aim"]} */ (flown[0].aim.map(Math.round));
  host.changed();
  host.done();
  playing = performance.now();
}
// Drops any flight being recorded and any countdown.
function grounded() {
  [flying, counting] = [null, null];
  if (document.pointerLockElement) document.exitPointerLock();
  $("cam-badge").hidden = true;
}
// The badge over the picture: the countdown, then how long the flight has run. Starts the flight when
// the countdown ends, and stops it after LONGEST seconds.
/** @param {Camera} c @param {number} now */
function badge(c, now) {
  const el = $("cam-badge"),
    left = counting === null ? 0 : COUNT - (now - counting) / 1000;
  if (counting !== null && left <= 0) {
    [counting, flying] = [null, { from: now, flown: [] }];
    host.panel();
  }
  const secs = flying ? (now - flying.from) / 1000 : 0,
    text = flying
      ? `● ${Math.floor(secs / 60)}:${String(Math.floor(secs % 60)).padStart(2, "0")} · ${document.pointerLockElement ? "Click or R" : "R"}: stop`
      : String(Math.ceil(left));
  el.hidden = !flying && counting === null;
  el.classList.toggle("rec", flying !== null);
  if (el.textContent !== text) el.textContent = text;
  if (secs >= LONGEST) {
    landed(c);
    host.panel();
  }
}
// Selects the end frame for placing. On a recorded flight, End only shows its last moment.
/** @param {Camera} c */
function toEnd(c) {
  if (c.move?.path) {
    [frame, playing, scrub] = ["start", null, c.move.secs];
    input("cam-scrub").value = "1000";
    return;
  }
  framed(c);
  [frame, playing, scrub] = ["end", null, null];
  host.changed();
}
// The R key does what the Record button does.
export function recKey() {
  const c = host.camera();
  if (!c) return;
  record(c);
  stale = true;
  host.panel();
}
// Wraps what a control does to the camera being placed. With `plays`, the move then plays here.
/** @param {(c: Camera, e: Event) => void} does @param {boolean} [plays] */
const on = (does, plays) => (/** @type {Event} */ e) => {
  const c = host.camera();
  if (!c) return;
  does(c, e);
  if (plays) [playing, scrub] = [c.move ? performance.now() : null, null];
  sent = does === run;
  stale = true;
  host.panel();
};
// Sends the move to the relay's camera; it plays here too.
/** @param {Camera} c */
function run(c) {
  const from = startOf(c);
  host.send({ pos: from.pos.map(Math.round), aim: from.aim.map(Math.round), path: rowsOf(c, c.move) }).then(
    async r => r.ok || note(await r.text()),
    () => note("No relay"),
  );
}
// Ends a flight, or a countdown when the picture does not hold the pointer. Returns whether it did.
function ended() {
  const c = host.camera();
  if (c && flying) landed(c);
  else if (counting !== null && !document.pointerLockElement) grounded();
  else return false;
  host.panel();
  return true;
}
// Esc ends a flight or its countdown. Returns whether it did.
export function escaped() {
  if (flying) return ended();
  if (counting === null) return false;
  grounded();
  host.panel();
  return true;
}
function wireRest() {
  // A flight ends on a click in the picture while it holds the pointer, or when it lets go of it (Esc).
  const end = ended;
  // Captured, so it runs before the picture takes the click as an aim.
  $("cam-view").addEventListener(
    "pointerdown",
    e => {
      if (!flying || !document.pointerLockElement) return;
      e.stopImmediatePropagation();
      end();
    },
    { capture: true },
  );
  document.addEventListener("pointerlockchange", () => document.pointerLockElement || end());
  $("cam-run").addEventListener("click", on(run, true));
  $("cam-rec").addEventListener("click", on(record));
  $("cam-still").addEventListener(
    "click",
    on(c => {
      delete c.move;
      [frame, playing, scrub] = ["start", null, null];
      host.changed();
    }),
  );
}
/** @param {Host} h */
export function composeTool(h) {
  host = h;
  const value = (/** @type {Event} */ e) => /** @type {HTMLInputElement} */ (e.target).value;
  $("cam-start").addEventListener(
    "click",
    on(() => ([frame, playing, scrub] = ["start", null, null])),
  );
  $("cam-end").addEventListener("click", on(toEnd));
  // An orbit button moves the end frame one step round its aim, adding to the arc's turn so far.
  $("cam-moves").addEventListener(
    "click",
    on((c, e) => {
      const way = Number(/** @type {HTMLElement} */ (e.target).dataset.way);
      if (!way) return;
      const move = framed(c),
        before = move.turn ?? 0;
      move.end = orbited(move.end, way);
      move.end.pos = /** @type {Spot["pos"]} */ (move.end.pos.map(Math.round));
      [move.way, move.turn] = ["arc", turnTo(c, move.end, before + ORBIT * way)];
      host.done();
    }),
  );
  $("cam-way").addEventListener(
    "change",
    on((c, e) => c.move && (c.move.way = value(e) === "arc" ? "arc" : "straight")),
  );
  $("cam-secs").addEventListener(
    "change",
    on((c, e) => c.move && (c.move.secs = Math.min(60, Math.max(1, Number(value(e)) || SECS)))),
  );
  $("cam-even").addEventListener(
    "change",
    on((c, e) => {
      const again = c.move?.raw ? flight(c.move.raw, Number(value(e)) || 0, c.move.secs) : null;
      if (again) c.move = again;
    }),
  );
  $("cam-scrub").addEventListener(
    "input",
    on((c, e) => {
      [playing, scrub] = [null, c.move ? (Number(value(e)) / 1000) * c.move.secs : null];
    }),
  );
  $("cam-play").addEventListener(
    "click",
    on(() => ([playing, scrub] = [playing === null ? performance.now() : null, null])),
  );
  wireRest();
  // Keys and presses in the bar must not reach the picture.
  $("cam-bar").addEventListener("keydown", e => e.stopPropagation());
  $("cam-bar").addEventListener("pointerdown", e => e.stopPropagation());
}
