// A room's notes: a short text at a spot on the map, or on a player (following their dot), in
// the drawer's ink, kept until cleared; in TF2 the game's text bubble. `n` puts one where the
// pointer is: on the player under it (a HUD card, a name in the events or chat, a dot), or else
// at that spot; then the text, Enter to place it.
import { INK, ctx, state, uiScale } from "../core/core.js";
import { env } from "../core/env.js";
import { playerAt } from "../core/input.js";
import { label, toScreen } from "../map/view.js";
import { post } from "./home.js";
import { note, worldAt } from "./tools.js";
/** @typedef {import("../../types/stream").NoteLine} NoteLine */

// The most characters a note says (src/rooms/room/note.rs).
const MAX_TEXT = 40;
// The label's text size at the 1080p size, its padding, and how far above a player's dot it is.
const TEXT_PX = 13,
  PAD_PX = 4,
  ABOVE_PX = 22;

/** @typedef {{ text: string; ink: string; at: [number, number, number] | null; player: number | null }} Note */
/** @type {Map<number, Note>} */
const notes = new Map();

/** @param {NoteLine} m */
export const onNote = m => notes.set(m.stroke, { text: m.text, ink: m.ink, at: m.at, player: m.player });

// The marker's notes, in place of those the page had.
/** @param {{ stroke: number; ink?: string; points: [number, number, number][]; note?: { text: string; player?: number } }[]} strokes */
export function notesFromMarker(strokes) {
  notes.clear();
  for (const k of strokes)
    if (k.note && k.ink) {
      const player = k.note.player ?? null;
      notes.set(k.stroke, { text: k.note.text, ink: k.ink, at: player == null ? k.points[0] : null, player });
    }
}

export const clearNotes = () => notes.clear();
export const noteCount = () => notes.size;

export function drawNotes() {
  if (!notes.size) return;
  const s = uiScale();
  ctx.save();
  for (const n of notes.values()) {
    const p = n.player == null ? null : state.lastPlayers.find(q => q.id === n.player);
    const where = n.at ?? p?.pos;
    if (!where) continue;
    const [x, y] = toScreen(where[0], where[1]);
    tag(n, x, p ? y - ABOVE_PX * s : y, s);
  }
  ctx.restore();
}

// A note's text in a dark box edged in its ink, centred on x, y.
/** @param {Note} n @param {number} x @param {number} y @param {number} s */
function tag(n, x, y, s) {
  const size = TEXT_PX * s,
    pad = PAD_PX * s;
  ctx.font = `700 ${Math.round(size)}px system-ui`;
  const w = ctx.measureText(n.text).width + 2 * pad,
    h = size + 2 * pad;
  ctx.fillStyle = INK;
  ctx.strokeStyle = n.ink;
  ctx.lineWidth = 2 * s;
  ctx.beginPath();
  ctx.roundRect(x - w / 2, y - h / 2, w, h, pad);
  ctx.fill();
  ctx.stroke();
  label(n.text, x, y, size, { colour: "#fff", weight: "700" });
}

// The pointer, for `n`: where it is and over what.
/** @type {{ x: number; y: number; target: Element | null }} */
const pointer = { x: 0, y: 0, target: null };

// `n` places a note, for admins (`allowed`), where the pointer is.
/** @param {() => boolean} allowed */
export function noteKey(allowed) {
  env.root.addEventListener(
    "pointermove",
    e => {
      const m = /** @type {PointerEvent} */ (e);
      Object.assign(pointer, { x: m.clientX, y: m.clientY, target: /** @type {Element} */ (m.composedPath()[0]) });
    },
    { capture: true, passive: true },
  );
  env.keys.addEventListener("keydown", e => {
    const target = /** @type {Element} */ (e.composedPath()[0]);
    if (e.key !== "n" || !allowed() || e.ctrlKey || e.metaKey || e.altKey) return;
    if (target.closest?.("input, select, textarea")) return;
    const player = playerAt(pointer.x, pointer.y, pointer.target);
    const onMap = pointer.target?.closest?.("canvas");
    if (player == null && !onMap) return;
    e.preventDefault();
    ask(player == null ? { at: worldAt({ clientX: pointer.x, clientY: pointer.y }) } : { player });
  });
}

// A text box at the pointer; Enter asks the room for the note `where` says, Esc lets it go.
/** @param {{ at: [number, number] } | { player: number }} where */
function ask(where) {
  const box = document.createElement("input");
  const frame = env.el.getBoundingClientRect();
  Object.assign(box, { className: "note-input", maxLength: MAX_TEXT, placeholder: "Note, then Enter" });
  Object.assign(box.style, { left: `${pointer.x - frame.left}px`, top: `${pointer.y - frame.top}px` });
  const done = () => box.remove();
  box.addEventListener("keydown", e => {
    e.stopPropagation();
    if (e.key === "Escape") done();
    if (e.key !== "Enter") return;
    const text = box.value.trim();
    done();
    if (text)
      post("note", { text, ...where }).then(
        async r => r.ok || note(await r.text()),
        () => {},
      );
  });
  box.addEventListener("blur", done);
  env.el.append(box);
  box.focus();
}
