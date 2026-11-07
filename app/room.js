// A room (/r/<id>/): its playback bar, which its owner drives and everyone else watches; and
// on a match's own page, the button that opens a room on it.
import { $, OVERLAY, PAGE } from "./core.js";
import { env } from "./env.js";
import { playbackListeners } from "./stream.js";
/** @typedef {import("../types/stream").PlaybackLine} PlaybackLine */

const SPEEDS = [0.25, 0.5, 1, 2, 4];
// How far the arrow keys jump, in milliseconds.
const STEP_MS = 5000;
export const IN_ROOM = /\/r\/[0-9a-f]{32}\/$/.test(PAGE.replace(/overlay\/?$/, ""));
// The room's or match's own address, which its routes are under.
const HOME = new URL(PAGE.replace(/(overlay\/?|index\.html)?$/, ""), env.base);

/** @param {number} ms */
export function clock(ms) {
  const s = Math.floor(ms / 1000),
    pad = (/** @type {number} */ n) => String(n).padStart(2, "0");
  if (s < 3600) return `${Math.floor(s / 60)}:${pad(s % 60)}`;
  return `${Math.floor(s / 3600)}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}`;
}

// The playback bar in a room, the Room button elsewhere; neither on the overlay or another site.
export function initRoom() {
  if (OVERLAY || env.component) return;
  if (IN_ROOM) roomBar();
  else roomButton();
}

// Offered once the match has a recording to play (a demo); a demo still being fetched asks again.
function roomButton() {
  const button = /** @type {HTMLButtonElement} */ ($("roomopen"));
  const ask = () =>
    fetch(new URL("room", HOME)).then(
      r => {
        if (r.ok) button.hidden = false;
        else if (r.status === 409) setTimeout(ask, 5000);
      },
      () => {},
    );
  ask();
  button.onclick = async () => {
    button.disabled = true;
    const r = await fetch(new URL("room", HOME), { method: "POST" });
    if (r.ok) location.assign(new URL((await r.json()).page, location.origin));
    else {
      button.disabled = false;
      button.title = await r.text();
    }
  };
}

function roomBar() {
  env.el.classList.add("room");
  const bar = $("roombar"),
    play = /** @type {HTMLButtonElement} */ ($("rb-play")),
    time = /** @type {HTMLInputElement} */ ($("rb-time")),
    speed = /** @type {HTMLSelectElement} */ ($("rb-speed")),
    shown = $("rb-clock"),
    track = $("rb-track");
  bar.hidden = false;
  speed.replaceChildren(...SPEEDS.map(s => new Option(`${s}×`, String(s))));
  /** @type {PlaybackLine | null} */
  let last = null;
  let heard = 0,
    dragging = false,
    admin = false;
  const at = () => {
    if (!last) return 0;
    const ran = last.paused ? 0 : (performance.now() - heard) * last.speed;
    return Math.min(last.length, last.at + ran);
  };
  /** @param {{ paused?: boolean; speed?: number; seek?: number }} c */
  const control = c =>
    fetch(new URL("control", HOME), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(c),
    });
  const show = () => {
    if (!last) return;
    if (!dragging) time.value = String(at());
    shown.textContent = `${clock(dragging ? Number(time.value) : at())} / ${clock(last.length)}`;
  };
  playbackListeners.add(m => {
    const marks = !last || last.length !== m.length || last.rounds.join() !== m.rounds.join();
    [last, heard] = [m, performance.now()];
    play.textContent = m.paused ? "▶" : "⏸";
    play.title = m.paused ? "Play (space)" : "Pause (space)";
    speed.value = String(m.speed);
    time.max = String(m.length);
    if (marks)
      track.replaceChildren(
        time,
        ...m.rounds.map(r => {
          const i = document.createElement("i");
          i.style.left = `${(100 * r) / Math.max(1, m.length)}%`;
          return i;
        }),
      );
    show();
  });
  setInterval(show, 250);
  /** @param {boolean} on */
  const allow = on => {
    admin = on;
    for (const c of [play, time, speed]) c.disabled = !on;
    bar.title = on ? "" : "The room's owner controls playback";
  };
  allow(false);
  fetch(new URL("me", HOME))
    .then(r => r.json())
    .then(
      me => allow(me.admin === true),
      () => {},
    );
  play.onclick = () => last && control({ paused: !last.paused });
  speed.onchange = () => control({ speed: Number(speed.value) });
  time.oninput = () => {
    dragging = true;
    show();
  };
  time.onchange = () => {
    dragging = false;
    control({ seek: Math.round(Number(time.value)) });
  };
  env.keys.addEventListener("keydown", e => {
    const target = /** @type {Element} */ (e.composedPath()[0]);
    if (!admin || !last || e.ctrlKey || e.metaKey || e.altKey || target.closest?.("input, select, textarea, button"))
      return;
    if (e.key === " ") control({ paused: !last.paused });
    else if (e.key === "ArrowLeft") control({ seek: Math.max(0, Math.round(at() - STEP_MS)) });
    else if (e.key === "ArrowRight") control({ seek: Math.round(at() + STEP_MS) });
    else return;
    e.preventDefault();
  });
}
