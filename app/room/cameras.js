// A map's cameras (src/cameras.rs), behind a room's Cameras layer: an admin's click on one puts
// the relay there, again gives it back. The site's streamers place them; a room's other admins
// place the room's own.
import { $, canvas, ctx, state, uiScale } from "../core/core.js";
import { asset } from "../core/env.js";
import { setAutoZoom } from "../core/input.js";
import { layerOn } from "../core/layers.js";
import { flyTool } from "./fly.js";
import { focusOf, label, toScreen } from "../map/view.js";
import { HOME, me, post } from "./home.js";
import {
  begun,
  composeTool,
  composing,
  counted,
  drawTrack,
  escaped,
  moved,
  onEnd,
  recKey,
  shown,
  spotOf,
} from "./compose.js";
import { drawGoing, offKey, setOff } from "./going.js";
import { fromKept, startOf, toKept } from "./moves.js";
import { preview } from "./preview.js";
import { blind, floorUnder, loadSight, showsOf } from "./sight.js";
import { note, worldAt } from "./tools.js";
/** @typedef {[number, number, number]} Vec3 */
/** @typedef {import("./moves.js").Move} Move */
/** @typedef {{ id?: number; name: string; pos: Vec3; aim: Vec3; room?: boolean; move?: Move }} Camera */
// One as the site lists it.
/** @typedef {Omit<Camera, "move"> & { move?: import("./moves.js").Kept }} Listed */
/** @typedef {{ clientX: number; clientY: number }} Pointer */

// Map units: a wheel step, a new camera's height and distance, how far over the players its floor
// is, an arrow's nudge, and how far over the map the slider goes.
const STEP = 50,
  NEW_UP = 900,
  NEW_BACK = 500,
  NEW_REACH = 300,
  NUDGE = 16,
  OVER = 1500;
// A marker's size and how near a click takes it, in pixels at the 1080p size.
const MARK_PX = 9,
  PICK_PX = 14;
const COLOUR = { mark: "#ffd23f", on: "#7cff8a", bad: "#ff6b5a" };

/** @type {{ map: string; from: string; cameras: Listed[] } | null} */
let known = null;
// The room's own, by map.
/** @type {Record<string, Listed[]>} */
let own = {};
// The map asked about, and as whom: its answer may come late.
let asked = "";
// Where the relay's camera is, and whether it set off on a move from there.
/** @type {{ pos?: Vec3; aim?: Vec3; moving?: boolean }} */
let active = {};
/** @type {Camera | null} */
let editing = null;
/** @type {"pos" | "aim" | null} */
let held = null;
// The part last held: the arrows nudge it.
/** @type {"pos" | "aim"} */
let picked = "pos";
// Where the camera being placed was after each change, for Ctrl+Z: the first as it started.
/** @type {{ pos: Vec3; aim: Vec3; move?: Move }[]} */
let past = [];
/** @type {import("./sight.js").Grid | null} */
let sight = null;
/** @type {string | null} */
let look = null;
/** @param {import("../../types/stream").CameraLine} m */
export function onCamera(m) {
  active = { pos: m.pos, aim: m.aim, moving: m.moving };
  if (!editing) panel();
}
/** @param {import("../../types/stream").CamerasLine} m */
export function onCameras(m) {
  own = m.on;
  panel();
}
const ofRoom = () => (own[state.map ?? ""] ?? []).map(c => ({ ...c, room: true }));
const ofMap = () => [...(known?.cameras ?? []), ...ofRoom()];
// Those listed: the room's, and the map's for the site's streamers.
const mine = () => (me.cameras ? ofMap() : ofRoom());
// One's name in the list: the room's are numbered apart from the map's.
const keyOf = (/** @type {Camera | Listed} */ c) => `${c.room ? "r" : ""}${c.id}`;

const same = (/** @type {Vec3 | undefined} */ a, /** @type {Vec3 | undefined} */ b) =>
  a !== undefined && b !== undefined && a.every((v, i) => v === b[i]);
const isActive = (/** @type {Camera | Listed} */ c) => same(c.pos, active.pos) && same(startOf(c).aim, active.aim);
// The relay is where no camera known here is: at one taken away, or one of the map's unseen.
const elsewhere = () => active.pos !== undefined && !ofMap().some(isActive);
// Marked for the site's streamers: to others it may be one of the map's.
/** @returns {Camera[]} */
function stray() {
  const { pos, aim } = active;
  return me.cameras && pos && aim && elsewhere() ? [{ name: "", pos, aim }] : [];
}
/** @param {Camera | Listed} c */
function colourOf(c) {
  if (isActive(c)) return COLOUR.on;
  return sight && blind(sight, c) ? COLOUR.bad : COLOUR.mark;
}

// ---------- on the map
/** @param {Camera | Listed} c @param {string} colour @param {boolean} placing */
function mark(c, colour, placing) {
  const [x, y] = toScreen(c.pos[0], c.pos[1]),
    [ax, ay] = toScreen(c.aim[0], c.aim[1]),
    r = MARK_PX * uiScale(),
    far = Math.hypot(ax - x, ay - y) || 1,
    // Not being placed: a short line the way it looks.
    reach = placing ? far : Math.min(far, 3 * r);
  ctx.save();
  ctx.strokeStyle = ctx.fillStyle = colour;
  ctx.lineWidth = 2 * uiScale();
  if (placing) ctx.setLineDash([6, 4]);
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(x + ((ax - x) / far) * reach, y + ((ay - y) / far) * reach);
  ctx.stroke();
  ctx.setLineDash([]);
  if (placing) {
    ctx.beginPath();
    ctx.arc(ax, ay, r * 0.7, 0, 2 * Math.PI);
    ctx.stroke();
  }
  ctx.beginPath();
  ctx.arc(x, y, r, 0, 2 * Math.PI);
  ctx.fill();
  ctx.restore();
  if (c.name) label(c.name, x, y - r - 12 * uiScale(), 11 * uiScale(), { colour });
}
// A listed camera on the map, with where its move takes it.
/** @param {Listed} c */
function listedMark(c) {
  if (isEdited(c)) return;
  drawGoing(c, keyOf(c), colourOf(c));
  mark(c, colourOf(c), false);
}
// What the camera being placed shows of the points, said in the panel.
/** @param {{ pos: Vec3; aim: Vec3 }} c @param {import("./sight.js").Grid} grid */
function sayLook(c, grid) {
  look ??= showsOf(grid, c);
  if ($("cam-shows").textContent !== look) $("cam-shows").textContent = look;
}
const isEdited = (/** @type {Camera | Listed} */ c) =>
  c.id !== undefined && c.id === editing?.id && !c.room === !editing?.room;
export function drawCameras() {
  const map = layerOn("cameras") ? state.map : null;
  const now = editing ? shown(editing) : null;
  preview(map ? now : null, sight);
  if (!map) return;
  if (asked !== asking(map)) load(map);
  if (editing && sight) sayLook(spotOf(editing), sight);
  ofMap().forEach(listedMark);
  for (const c of stray()) mark(c, colourOf(c), false);
  if (!editing || !now) return;
  drawTrack(editing, now);
  // Both frames of a move, the one being placed with its aim.
  const end = editing.move && !editing.move.path ? editing.move.end : null,
    colour = (/** @type {{ pos: Vec3; aim: Vec3 }} */ c) => (sight && blind(sight, c) ? COLOUR.bad : COLOUR.mark);
  if (end) mark({ name: "", ...end }, colour(end), onEnd());
  mark(editing, colour(editing), !onEnd());
}

const asking = (/** @type {string} */ map) => `${map} ${me.cameras}`;
/** @param {string} map */
async function load(map) {
  const as = (asked = asking(map));
  [known, sight, look, editing] = [null, null, null, null];
  panel();
  // The map's are its streamers' to see.
  const r = me.cameras ? await fetch(asset(`cameras/${encodeURIComponent(map)}`)).catch(() => null) : null;
  if (asked !== as || (me.cameras && !r?.ok)) return;
  const said = r ? await r.json() : { map, cameras: [] };
  known = { map, from: said.map, cameras: said.cameras };
  panel();
  // Only for those who can act on a blind camera.
  if (!me.admin && !me.cameras) return;
  const grid = await loadSight(map);
  if (asked === as) [sight, look] = [grid, null];
}

// ---------- placing them
// The panel of the one being placed.
/** @param {Camera} c */
function fields(c) {
  /** @type {HTMLInputElement} */ ($("cam-name")).value = c.name;
  $("cam-remove").hidden = c.id === undefined;
  // A streamer's new one is the map's, or the room's alone.
  $("cam-own").hidden = c.id !== undefined || !me.cameras || !me.admin;
  /** @type {HTMLInputElement} */ ($("cam-room")).checked = Boolean(c.room);
  const [lo, hi] = state.geometry?.bounds ?? [
    [0, 0, -2000],
    [0, 0, 2000],
  ];
  Object.assign($("cam-z-slide"), {
    min: Math.floor(lo[2] / STEP) * STEP,
    max: Math.ceil(hi[2] / STEP) * STEP + OVER,
  });
  composing(c);
  heights();
}
function panel() {
  const box = $("cameras");
  box.hidden = !layerOn("cameras") || !(me.cameras || me.admin) || !known;
  if (box.hidden || !known) return;
  $("cam-edit").hidden = editing === null;
  $("cam-list").hidden = editing !== null;
  if (editing) return fields(editing);
  const from = Object.assign(document.createElement("div"), { className: "cfrom", textContent: `From ${known.from}` }),
    button = (/** @type {"go" | "id" | "play"} */ does, /** @type {string} */ id, /** @type {string} */ text) => {
      const b = Object.assign(document.createElement("button"), { textContent: text });
      b.dataset[does] = id;
      return b;
    },
    // A camera's name puts the relay there, ▶ sets it off on its move, Edit places it anew.
    row = (/** @type {Camera | Listed} */ c) => {
      const at = Object.assign(button("go", keyOf(c), c.name), { disabled: !me.admin }),
        div = Object.assign(document.createElement("div"), { className: "crow" });
      at.classList.toggle("on", isActive(c));
      if (c.room && me.cameras) at.title = "Room only";
      const play = Object.assign(button("play", keyOf(c), offKey() === keyOf(c) ? "…" : "▶"), { title: "Play" });
      play.classList.toggle("on", offKey() === keyOf(c));
      if (c.move && me.admin) div.append(at, play);
      else div.append(at);
      div.append(Object.assign(button("id", keyOf(c), "Edit"), { disabled: Boolean(c.room) && !me.admin }));
      return div;
    };
  $("cam-list").replaceChildren(
    ...(known.from === known.map || !me.cameras ? [] : [from]),
    ...mine().map(row),
    // No marker to give the relay back with.
    ...(elsewhere() && me.admin && !me.cameras ? [button("go", "back", "Director")] : []),
    button("id", "new", "Add"),
  );
}
// The frame of it being placed: its spot, or its move's end.
const placed = () => (editing ? spotOf(editing) : null);
function heights() {
  const c = placed();
  if (!c) return;
  /** @type {HTMLInputElement} */ ($("cam-z")).value = String(c.pos[2]);
  /** @type {HTMLInputElement} */ ($("cam-z-slide")).value = String(c.pos[2]);
  /** @type {HTMLInputElement} */ ($("cam-aim-z")).value = String(c.aim[2]);
}
// The camera being placed moved: what it shows is worked out again.
function changed() {
  look = null;
  if (editing) moved(editing);
  heights();
}
// A change to it is done: kept, to go back to.
const moveOf = (/** @type {Move | undefined} */ m) =>
  m && { ...m, end: { pos: /** @type {Vec3} */ ([...m.end.pos]), aim: /** @type {Vec3} */ ([...m.end.aim]) } };
function done() {
  const last = past[past.length - 1],
    now = editing && { pos: [...editing.pos], aim: [...editing.aim], move: moveOf(editing.move) };
  if (
    !now ||
    JSON.stringify([last?.pos, last?.aim, last?.move?.end]) === JSON.stringify([now.pos, now.aim, now.move?.end])
  )
    return;
  past.push(/** @type {typeof past[number]} */ (now));
}
function undo() {
  if (!editing || past.length < 2) return;
  past.pop();
  const was = past[past.length - 1];
  [editing.pos, editing.aim, editing.move] = [[...was.pos], [...was.aim], moveOf(was.move)];
  changed();
  panel();
}
/** @param {Camera | null} c */
function edit(c) {
  [editing, past, picked, look] = [c, [], "pos", null];
  begun();
  done();
  panel();
}
// An arrow moves the part last held a little, the way it points on the screen.
/** @param {KeyboardEvent} e */
function nudge(e) {
  const c = placed();
  if (!c) return;
  const far = e.shiftKey ? NUDGE * 4 : NUDGE,
    [dx, dy] = { ArrowLeft: [-far, 0], ArrowRight: [far, 0], ArrowUp: [0, -far], ArrowDown: [0, far] }[e.key] ?? [0, 0],
    [cos, sin] = [Math.cos(state.view.rot), Math.sin(state.view.rot)];
  c[picked][0] += Math.round(cos * dx - sin * dy);
  c[picked][1] += Math.round(-sin * dx - cos * dy);
  changed();
  done();
}
// The keys of placing one, before the page's own.
/** @param {KeyboardEvent} e */
function key(e) {
  if (!editing || e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
  // Esc ends a flight first, then the placing.
  if (e.key === "Escape") escaped() || edit(null);
  else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") undo();
  // R: a flight begun or ended.
  else if (e.key.toLowerCase() === "r" && !e.ctrlKey && !e.metaKey && !e.altKey) recKey();
  else if (e.key.startsWith("Arrow")) nudge(e);
  else return;
  e.preventDefault();
  e.stopImmediatePropagation();
}
// A camera over the middle of the view, looking down at it from one side.
/** @returns {Camera} */
function fresh() {
  const { cx, cy } = focusOf(state.view),
    [x, y] = [Math.round(cx), Math.round(cy)],
    // On the floor the players are about on, not a roof over it.
    zs = state.lastPlayers.flatMap(p => (p.pos ? [p.pos[2]] : [])),
    about = zs.length ? zs.reduce((a, b) => a + b, 0) / zs.length : 0,
    z = (sight && floorUnder(sight, x, y, about + NEW_REACH)) ?? Math.round(about);
  return { name: "", pos: [x + NEW_BACK, y, z + NEW_UP], aim: [x, y, z], room: !me.cameras };
}
// A change to the camera being placed: to the room's own, or the map's.
/** @param {object} change */
async function save(change) {
  const room = Boolean(editing?.room),
    r = await (room
      ? post("cameras", change, HOME)
      : post(asset(`cameras/${encodeURIComponent(state.map ?? "")}`), change));
  if (!r.ok) return note(await r.text());
  if (!room && state.map) return load(state.map);
  edit(null);
}
/** @param {Vec3} p @param {Pointer} e */
function near(p, e) {
  const r = canvas.getBoundingClientRect(),
    [x, y] = toScreen(p[0], p[1]);
  return Math.hypot(x - (e.clientX - r.left), y - (e.clientY - r.top)) < PICK_PX * uiScale();
}
// The part of the camera being placed a pointer is on.
/** @param {Pointer} e */
function handleAt(e) {
  const c = placed();
  if (!c) return null;
  if (near(c.pos, e)) return "pos";
  return near(c.aim, e) ? "aim" : null;
}

function wirePanel() {
  $("cam-list").addEventListener("click", e => {
    const { id, go, play } = /** @type {HTMLElement} */ (e.target).dataset;
    if (!known) return;
    const kept = mine().find(c => keyOf(c) === (id ?? go ?? play));
    if (go === "back") return put(null);
    if (go && kept) return put(kept);
    if (play && kept) return setOff(kept, keyOf(kept), body => post("camera", body, HOME), panel);
    if (!id) return;
    // The view stays put under what is dragged.
    setAutoZoom(false);
    edit(kept ? { ...kept, pos: [...kept.pos], aim: [...kept.aim], move: kept.move && fromKept(kept.move) } : fresh());
  });
  $("cam-room").addEventListener("change", e => {
    if (editing) editing.room = /** @type {HTMLInputElement} */ (e.target).checked;
  });
  composeTool({
    camera: () => editing,
    changed,
    done,
    panel,
    sight: () => sight,
    send: body => post("camera", body, HOME),
  });
  $("cam-name").addEventListener("input", e => {
    if (editing) editing.name = /** @type {HTMLInputElement} */ (e.target).value;
  });
  for (const [id, part] of /** @type {const} */ ([
    ["cam-z", "pos"],
    ["cam-z-slide", "pos"],
    ["cam-aim-z", "aim"],
  ])) {
    $(id).addEventListener("input", e => {
      const c = placed();
      if (!c) return;
      c[part][2] = Math.round(Number(/** @type {HTMLInputElement} */ (e.target).value)) || 0;
      changed();
    });
    $(id).addEventListener("change", done);
  }
  // Typing in it is not the page's keys.
  $("cameras").addEventListener("keydown", e => e.stopPropagation());
  $("cam-save").addEventListener("click", () => {
    if (!editing) return;
    const { id, name, pos, aim, move } = editing;
    save({ id, name, pos, aim, ...(move ? { move: toKept(move) } : { still: true }) });
  });
  $("cam-remove").addEventListener("click", () => editing && save({ id: editing.id, remove: true }));
  $("cam-cancel").addEventListener("click", () => edit(null));
  $("settings").addEventListener("change", () => panel());
}
// Puts the relay at a camera (again, or none: back).
/** @param {Camera | Listed | null} c */
function put(c) {
  // Off on its move, it is no longer there: its name puts it back.
  post("camera", !c || (isActive(c) && !active.moving) ? {} : startOf(c), HOME).then(
    async r => r.ok || note(await r.text()),
  );
}
// A press: takes hold of the camera being placed, or puts the relay at a marker (again: back).
/** @param {PointerEvent} e */
function press(e) {
  if (!layerOn("cameras") || e.button !== 0) return;
  const hit = editing ? null : [...ofMap(), ...stray()].find(c => near(c.pos, e));
  held = handleAt(e);
  picked = held ?? picked;
  if (!held && !hit) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  if (hit && me.admin) put(hit);
}
export function camerasTool() {
  wirePanel();
  canvas.addEventListener("pointerdown", press, { capture: true });
  addEventListener("pointermove", e => {
    const c = placed();
    if (!held || !c) return;
    const [x, y] = worldAt(e);
    [c[held][0], c[held][1]] = [x, y];
    // The aim lies on the floor under it.
    if (held === "aim" && sight) c.aim[2] = floorUnder(sight, x, y, c.pos[2]) ?? c.aim[2];
    changed();
  });
  addEventListener("pointerup", () => {
    if (held) done();
    held = null;
  });
  addEventListener("keydown", key, { capture: true });
  // Not while a flight is counted down to: it begins where the camera was put.
  flyTool(() => (counted() ? null : placed()), changed, done);
  canvas.addEventListener(
    "wheel",
    e => {
      const part = layerOn("cameras") ? handleAt(e) : null,
        c = placed();
      if (!part || !c) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      c[part][2] -= Math.sign(e.deltaY) * STEP;
      changed();
      done();
    },
    { capture: true, passive: false },
  );
}
