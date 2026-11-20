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
import { preview } from "./preview.js";
import { blind, floorUnder, loadSight, showsOf } from "./sight.js";
import { note, worldAt } from "./tools.js";
/** @typedef {[number, number, number]} Vec3 */
/** @typedef {{ id?: number; name: string; pos: Vec3; aim: Vec3; room?: boolean }} Camera */
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

/** @type {{ map: string; from: string; cameras: Camera[] } | null} */
let known = null;
// The room's own, by map.
/** @type {Record<string, Camera[]>} */
let own = {};
// The map asked about, and as whom: its answer may come late.
let asked = "";
/** @type {{ pos?: Vec3; aim?: Vec3 }} */
let active = {};
/** @type {Camera | null} */
let editing = null;
/** @type {"pos" | "aim" | null} */
let held = null;
// The part last held: the arrows nudge it.
/** @type {"pos" | "aim"} */
let picked = "pos";
// Where the camera being placed was after each change, for Ctrl+Z: the first as it started.
/** @type {{ pos: Vec3; aim: Vec3 }[]} */
let past = [];
/** @type {import("./sight.js").Grid | null} */
let sight = null;
/** @type {string | null} */
let look = null;

/** @param {import("../../types/stream").CameraLine} m */
export function onCamera(m) {
  active = { pos: m.pos, aim: m.aim };
  if (!editing) panel();
}
/** @param {import("../../types/stream").CamerasLine} m */
export function onCameras(m) {
  own = m.on;
  panel();
}
const ofRoom = () => (own[state.map ?? ""] ?? []).map(c => ({ ...c, room: true }));
const ofMap = () => [...(known?.cameras ?? []), ...ofRoom()];
// Those this page places: the site's for its streamers, the room's for its other admins.
const mine = () => (me.cameras ? (known?.cameras ?? []) : ofRoom());

const same = (/** @type {Vec3 | undefined} */ a, /** @type {Vec3 | undefined} */ b) =>
  a !== undefined && b !== undefined && a.every((v, i) => v === b[i]);
const isActive = (/** @type {{ pos: Vec3; aim: Vec3 }} */ c) => same(c.pos, active.pos) && same(c.aim, active.aim);
// The relay is where no camera known here is: at one taken away, or one of the map's unseen.
const elsewhere = () => active.pos !== undefined && !ofMap().some(isActive);
// Marked for the site's streamers: to others it may be one of the map's.
/** @returns {Camera[]} */
function stray() {
  const { pos, aim } = active;
  return me.cameras && pos && aim && elsewhere() ? [{ name: "", pos, aim }] : [];
}
/** @param {Camera} c */
function colourOf(c) {
  if (isActive(c)) return COLOUR.on;
  return sight && blind(sight, c) ? COLOUR.bad : COLOUR.mark;
}

// ---------- on the map
/** @param {Camera} c @param {string} colour @param {boolean} placing */
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
// What the camera being placed shows of the points, said in the panel.
/** @param {Camera} c @param {import("./sight.js").Grid} grid */
function sayLook(c, grid) {
  look ??= showsOf(grid, c);
  if ($("cam-shows").textContent !== look) $("cam-shows").textContent = look;
}
const isEdited = (/** @type {Camera} */ c) => c.id !== undefined && c.id === editing?.id && !c.room === !editing?.room;
export function drawCameras() {
  const map = layerOn("cameras") ? state.map : null;
  preview(map ? editing : null, sight);
  if (!map) return;
  if (asked !== asking(map)) load(map);
  if (editing && sight) sayLook(editing, sight);
  for (const c of [...ofMap(), ...stray()]) if (!isEdited(c)) mark(c, colourOf(c), false);
  if (editing) mark(editing, sight && blind(sight, editing) ? COLOUR.bad : COLOUR.mark, true);
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
function panel() {
  const box = $("cameras");
  box.hidden = !layerOn("cameras") || !(me.cameras || me.admin) || !known;
  if (box.hidden || !known) return;
  $("cam-edit").hidden = editing === null;
  $("cam-list").hidden = editing !== null;
  if (editing) {
    /** @type {HTMLInputElement} */ ($("cam-name")).value = editing.name;
    $("cam-remove").hidden = editing.id === undefined;
    const [lo, hi] = state.geometry?.bounds ?? [
      [0, 0, -2000],
      [0, 0, 2000],
    ];
    Object.assign($("cam-z-slide"), {
      min: Math.floor(lo[2] / STEP) * STEP,
      max: Math.ceil(hi[2] / STEP) * STEP + OVER,
    });
    return heights();
  }
  const from = Object.assign(document.createElement("div"), { className: "cfrom", textContent: `From ${known.from}` }),
    button = (/** @type {"go" | "id"} */ does, /** @type {string} */ id, /** @type {string} */ text) => {
      const b = Object.assign(document.createElement("button"), { textContent: text });
      b.dataset[does] = id;
      return b;
    },
    // A camera's name puts the relay there, Edit places it anew.
    row = (/** @type {Camera} */ c) => {
      const at = Object.assign(button("go", String(c.id), c.name), { disabled: !me.admin }),
        div = Object.assign(document.createElement("div"), { className: "crow" });
      at.classList.toggle("on", isActive(c));
      div.append(at, button("id", String(c.id), "Edit"));
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
function heights() {
  if (!editing) return;
  /** @type {HTMLInputElement} */ ($("cam-z")).value = String(editing.pos[2]);
  /** @type {HTMLInputElement} */ ($("cam-z-slide")).value = String(editing.pos[2]);
  /** @type {HTMLInputElement} */ ($("cam-aim-z")).value = String(editing.aim[2]);
}
// The camera being placed moved: what it shows is worked out again.
function changed() {
  look = null;
  heights();
}
// A change to it is done: kept, to go back to.
function done() {
  const last = past[past.length - 1];
  if (!editing || (last && same(last.pos, editing.pos) && same(last.aim, editing.aim))) return;
  past.push({ pos: [...editing.pos], aim: [...editing.aim] });
}
function undo() {
  if (!editing || past.length < 2) return;
  past.pop();
  const was = past[past.length - 1];
  [editing.pos, editing.aim] = [[...was.pos], [...was.aim]];
  changed();
}
/** @param {Camera | null} c */
function edit(c) {
  [editing, past, picked, look] = [c, [], "pos", null];
  done();
  panel();
}
// An arrow moves the part last held a little, the way it points on the screen.
/** @param {KeyboardEvent} e */
function nudge(e) {
  if (!editing) return;
  const far = e.shiftKey ? NUDGE * 4 : NUDGE,
    [dx, dy] = { ArrowLeft: [-far, 0], ArrowRight: [far, 0], ArrowUp: [0, -far], ArrowDown: [0, far] }[e.key] ?? [0, 0],
    [cos, sin] = [Math.cos(state.view.rot), Math.sin(state.view.rot)];
  editing[picked][0] += Math.round(cos * dx - sin * dy);
  editing[picked][1] += Math.round(-sin * dx - cos * dy);
  changed();
  done();
}
// The keys of placing one, before the page's own.
/** @param {KeyboardEvent} e */
function key(e) {
  if (!editing || e.target instanceof HTMLInputElement) return;
  if (e.key === "Escape") edit(null);
  else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") undo();
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
  return { name: "", pos: [x + NEW_BACK, y, z + NEW_UP], aim: [x, y, z] };
}
/** @param {object} change */
async function save(change) {
  const r = await (me.cameras
    ? post(asset(`cameras/${encodeURIComponent(state.map ?? "")}`), change)
    : post("cameras", change, HOME));
  if (!r.ok) return note(await r.text());
  if (me.cameras && state.map) return load(state.map);
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
  if (!editing) return null;
  if (near(editing.pos, e)) return "pos";
  return near(editing.aim, e) ? "aim" : null;
}

function wirePanel() {
  $("cam-list").addEventListener("click", e => {
    const { id, go } = /** @type {HTMLElement} */ (e.target).dataset;
    if (!known) return;
    const kept = mine().find(c => String(c.id) === (id ?? go));
    if (go === "back") return put(null);
    if (go && kept) return put(kept);
    if (!id) return;
    // The view stays put under what is dragged.
    setAutoZoom(false);
    edit(kept ? { ...kept, pos: [...kept.pos], aim: [...kept.aim] } : fresh());
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
      if (!editing) return;
      editing[part][2] = Math.round(Number(/** @type {HTMLInputElement} */ (e.target).value)) || 0;
      changed();
    });
    $(id).addEventListener("change", done);
  }
  // Typing in it is not the page's keys.
  $("cameras").addEventListener("keydown", e => e.stopPropagation());
  $("cam-save").addEventListener("click", () => {
    if (!editing) return;
    const { id, name, pos, aim } = editing;
    save({ id, name, pos, aim });
  });
  $("cam-remove").addEventListener("click", () => editing && save({ id: editing.id, remove: true }));
  $("cam-cancel").addEventListener("click", () => edit(null));
  $("settings").addEventListener("change", () => panel());
}
// Puts the relay at a camera (again, or none: back).
/** @param {Camera | null} c */
function put(c) {
  post("camera", !c || isActive(c) ? {} : { pos: c.pos, aim: c.aim }, HOME).then(
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
    if (!held || !editing) return;
    const [x, y] = worldAt(e);
    [editing[held][0], editing[held][1]] = [x, y];
    // The aim lies on the floor under it.
    if (held === "aim" && sight) editing.aim[2] = floorUnder(sight, x, y, editing.pos[2]) ?? editing.aim[2];
    changed();
  });
  addEventListener("pointerup", () => {
    if (held) done();
    held = null;
  });
  addEventListener("keydown", key, { capture: true });
  flyTool(() => editing, changed, done);
  canvas.addEventListener(
    "wheel",
    e => {
      const part = layerOn("cameras") ? handleAt(e) : null;
      if (!part || !editing) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      editing[part][2] -= Math.sign(e.deltaY) * STEP;
      changed();
      done();
    },
    { capture: true, passive: false },
  );
}
