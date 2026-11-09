// Copy buttons: a link, or a console line, copied with a check shown for a moment.
import { $ } from "../core/core.js";

// How long the link button says it copied, in milliseconds.
const COPIED_MS = 1500;

// The link button by the map's name: copies the link `url` makes (`what` it is), a check for a
// moment once it has.
/** @param {string} what @param {() => URL} url */
export function copyLink(what, url) {
  const link = $("copylink");
  Object.assign(link, { hidden: false, title: what });
  link.setAttribute("aria-label", what);
  link.onclick = () => copyText(url().href).then(ok => ok && copied(link));
}

// Copies `text`: through the clipboard API where the page may use it (HTTPS, localhost), else by
// copying a selection, the old way, which a page over plain HTTP (on a LAN) may still do.
/** @param {string} text @returns {Promise<boolean>} whether it did */
export async function copyText(text) {
  try {
    if (navigator.clipboard) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {}
  const area = Object.assign(document.createElement("textarea"), { value: text, readOnly: true });
  area.className = "offscreen";
  document.body.append(area);
  area.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {}
  area.remove();
  return ok;
}

// A copy button shows a check for a moment.
/** @param {HTMLElement} button */
export function copied(button) {
  button.classList.add("copied");
  setTimeout(() => button.classList.remove("copied"), COPIED_MS);
}
