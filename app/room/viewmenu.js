// The room's view choice: a short button, and a menu of the views that opens upward over the map,
// in the bar's own colours (a browser's own menu goes off the bottom of the Steam overlay, nearly
// unreadable there).
import { $ } from "../core/core.js";
import { env } from "../core/env.js";

// The button's word for each view.
/** @type {Record<string, string>} */
const SHORT = { free: "Free", auto: "Auto", follow: "Follow", admin: "Admin" };

/** @param {(mode: string) => void} chosen @returns {(mode: string) => void} sets the view shown */
export function viewMenu(chosen) {
  const button = /** @type {HTMLButtonElement} */ ($("rb-view")),
    menu = $("rb-viewmenu");
  const open = (/** @type {boolean} */ on) => {
    menu.hidden = !on;
    button.setAttribute("aria-expanded", String(on));
  };
  button.onclick = () => open(menu.hidden !== false);
  menu.addEventListener("click", e => {
    const item = /** @type {HTMLButtonElement | null} */ (
      /** @type {Element} */ (e.target).closest("button[data-mode]")
    );
    if (!item || item.disabled) return;
    open(false);
    chosen(item.dataset.mode ?? "free");
  });
  env.root.addEventListener("pointerdown", e => {
    const at = /** @type {Element} */ (e.composedPath()[0]);
    if (!menu.hidden && !at.closest?.("#rb-viewmenu, #rb-view")) open(false);
  });
  env.keys.addEventListener("keydown", e => e.key === "Escape" && open(false));
  return mode => {
    if (button.dataset.mode === mode) return;
    button.dataset.mode = mode;
    button.textContent = `👁 ${SHORT[mode] ?? mode}`;
    for (const b of menu.querySelectorAll("button[data-mode]"))
      b.classList.toggle("on", /** @type {HTMLElement} */ (b).dataset.mode === mode);
  };
}
