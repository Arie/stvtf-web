// A live match's maps: what the bar says of the room's map, and the control to move between them,
// built like the view menu (viewmenu.js).
import { $ } from "../core/core.js";
import { env } from "../core/env.js";
/** @typedef {import("../../types/stream").PlaybackLine} PlaybackLine */

/**
 * What the bar says of the room's map, and the map its button goes to, if any: the newest that has
 * play to show of those after the room's.
 * @param {PlaybackLine} p @returns {{ status: string; next: number | null }}
 */
export function mapState(p) {
  if (!p.over) return { status: "", next: null };
  const next = (p.maps ?? []).findLast(m => m.load > (p.load ?? 0) && m.ready)?.load ?? null;
  return { status: next == null ? "Map over" : "Next map ready", next };
}

/** The names as the menu reads them: a map that comes again is `<name> 2`, `<name> 3`. @param {string[]} names */
export function mapNames(names) {
  const seen = /** @type {Record<string, number>} */ ({});
  return names.map(n => ((seen[n] = (seen[n] ?? 0) + 1) > 1 ? `${n} ${seen[n]}` : n));
}

/** The maps control. @param {(load: number) => void} moveToMap @returns {(p: PlaybackLine, admin: boolean) => void} heard */
export function mapsMenu(moveToMap) {
  const box = $("rb-mapbox"),
    button = /** @type {HTMLButtonElement} */ ($("rb-maps")),
    menu = $("rb-mapsmenu"),
    next = /** @type {HTMLButtonElement} */ ($("rb-nextmap"));
  const open = (/** @type {boolean} */ on) => {
    menu.hidden = !on;
    button.setAttribute("aria-expanded", String(on));
    $("roombar").classList.toggle("menu", on);
  };
  /** @type {number | null} */
  let target = null;
  button.onclick = () => open(menu.hidden !== false);
  next.onclick = () => target != null && moveToMap(target);
  menu.addEventListener("click", e => {
    const picked = /** @type {HTMLButtonElement | null} */ (
      /** @type {Element} */ (e.target).closest("button[data-load]")
    );
    if (!picked || button.disabled) return;
    open(false);
    moveToMap(Number(picked.dataset.load));
  });
  env.root.addEventListener("pointerdown", e => {
    const at = /** @type {Element} */ (e.composedPath()[0]);
    if (!menu.hidden && !at.closest?.("#rb-mapsmenu, #rb-maps")) open(false);
  });
  env.keys.addEventListener("keydown", e => e.key === "Escape" && open(false));
  let listed = "";
  return (p, admin) => {
    const maps = p.maps ?? [],
      here = p.load ?? 0;
    const shown = maps.length > 1;
    box.hidden = button.hidden = !shown;
    button.disabled = !admin;
    if (!shown || !admin) open(false);
    const names = mapNames(maps.map(m => m.map));
    const key = JSON.stringify([here, maps]);
    if (shown && key !== listed) {
      listed = key;
      button.textContent = `▤ ${names[maps.findIndex(m => m.load === here)] ?? ""}`;
      menu.replaceChildren(...maps.map((m, i) => item(m, names[i], m.load === here)));
    }
    target = mapState(p).next;
    next.hidden = !admin || target == null;
    if (target != null) next.title = names[maps.findIndex(m => m.load === target)] ?? "";
  };
}

/** @param {{ load: number; map: string; ready: boolean }} m @param {string} name @param {boolean} here */
function item(m, name, here) {
  const b = document.createElement("button");
  b.dataset.load = String(m.load);
  b.textContent = name;
  b.disabled = !m.ready && !here;
  b.classList.toggle("on", here);
  return b;
}
