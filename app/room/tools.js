// The room's click tools, the jump arc, the circle and the spotlight: armed by their button or key, they take
// clicks on the map and ask the room to draw, staying armed; Esc, the button again or another tool
// lets them go.
import { $, canvas, state } from "../core/core.js";
import { env } from "../core/env.js";
import { screenOffset } from "../map/view.js";
import { pendingAt, setPending } from "./arcs.js";
import { post } from "./home.js";

// A tool on `button`: `click` takes each click on the map while armed, `reset` forgets a click
// half done. Picking one of `others` lets it go, leaving the drawing mode to that one.
/** @param {HTMLButtonElement} button @param {HTMLButtonElement[]} others @param {(at: [number, number], e: PointerEvent) => void} click */
export function armTool(button, others, click, reset = () => {}) {
  const disarm = () => {
    button.classList.toggle("on", false);
    reset();
  };
  const arm = (/** @type {boolean} */ on) => {
    disarm();
    button.classList.toggle("on", on);
    env.el.classList.toggle("drawing", on);
  };
  button.onclick = () => {
    for (const o of others) if (o.classList.contains("on")) o.click();
    arm(!button.classList.contains("on"));
  };
  for (const o of others) o.addEventListener("click", disarm);
  env.keys.addEventListener("keydown", e => e.key === "Escape" && button.classList.contains("on") && arm(false));
  canvas.addEventListener(
    "pointerdown",
    e => {
      if (!button.classList.contains("on") || e.button !== 0) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      click(worldAt(e), e);
    },
    { capture: true },
  );
}

// Asks the room to draw `kind`, saying why when it does not.
/** @param {string} kind @param {object} body */
const ask = (kind, body) =>
  post(kind, body).then(
    async r => r.ok || note(await r.text()),
    () => {},
  );

// The jump arc: two clicks, the launch then the landing.
/** @param {HTMLButtonElement} button @param {HTMLButtonElement[]} others */
export function arcTool(button, others) {
  const click = (/** @type {[number, number]} */ at) => {
    const from = pendingAt();
    if (!from) return void setPending(at);
    setPending(null);
    ask("arc", { from, to: at });
  };
  armTool(button, others, click, () => setPending(null));
}

// The circle: a click puts one there.
/** @param {HTMLButtonElement} button @param {HTMLButtonElement[]} others */
export const circleTool = (button, others) => armTool(button, others, at => ask("circle", { at }));

// The spotlight: a click puts it there, from where it was.
/** @param {HTMLButtonElement} button @param {HTMLButtonElement[]} others */
export const lightTool = (button, others) => armTool(button, others, at => ask("light", { at }));

// Where on the map a pointer is.
/** @param {{ clientX: number; clientY: number }} e @returns {[number, number]} */
export function worldAt(e) {
  const r = canvas.getBoundingClientRect();
  const [dx, dy] = screenOffset(e.clientX - r.left - r.width / 2, e.clientY - r.top - r.height / 2, state.view.scale);
  return [Math.round(state.view.cx + dx), Math.round(state.view.cy + dy)];
}

// Why something did not happen, in the room bar for a moment.
/** @param {string} text */
export function note(text) {
  const el = $("rb-note");
  Object.assign(el, { textContent: text, hidden: false });
  clearTimeout(noteTimer);
  noteTimer = setTimeout(() => (el.hidden = true), NOTE_MS);
}
export const NOTE_MS = 5000;
/** @type {ReturnType<typeof setTimeout> | undefined} */
let noteTimer;
