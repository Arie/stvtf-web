// A room (/r/<id>/): its playback bar, which its owner drives and everyone else watches; and
// on a match's own page, the button that opens a room on it.
import { $, OVERLAY, PAGE, canvas, state } from "./core.js";
import { env } from "./env.js";
import { setAutoZoom, setFollow } from "./input.js";
import { peopleListeners, playbackListeners, streamParams, viewListeners } from "./stream.js";
import { drawOwn, inkOf } from "./telestrator.js";
import { focusOf, screenOffset, viewArea } from "./view.js";
/** @typedef {import("../types/stream").PlaybackLine} PlaybackLine */

const SPEEDS = [0.25, 0.5, 1, 2, 4];
// How often a stroke being drawn is sent, in milliseconds, and the closest its points are, in pixels.
const SEND_MS = 50,
  MIN_STEP_PX = 3;
// How far the arrow keys jump, in milliseconds.
const STEP_MS = 5000;
export const IN_ROOM = /\/r\/[0-9a-f]{32}\/$/.test(PAGE.replace(/overlay\/?$/, ""));
// The room's or match's own address, which its routes are under.
const HOME = new URL(PAGE.replace(/(overlay\/?|index\.html)?$/, ""), env.base);

// Who this page is in the room: its number, and whether it is an admin.
const me = { id: 0, admin: false };
// Where a name is kept between visits.
const NAME_KEY = "name";

/** @param {string} path @param {object} body */
const post = (path, body) =>
  fetch(new URL(path, HOME), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });

/** @param {number} ms */
export function clock(ms) {
  const s = Math.floor(ms / 1000),
    pad = (/** @type {number} */ n) => String(n).padStart(2, "0");
  if (s < 3600) return `${Math.floor(s / 60)}:${pad(s % 60)}`;
  return `${Math.floor(s / 3600)}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}`;
}

// The playback bar in a room, the Room button elsewhere; neither on the overlay or another site.
export function initRoom() {
  if (env.component) return;
  if (IN_ROOM) roomView();
  if (OVERLAY) return;
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
    pen = /** @type {HTMLButtonElement} */ ($("rb-pen")),
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
  const control = c => post("control", c);
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
    admin = me.admin = on;
    for (const c of [play, time, speed, pen, $("rb-view")]) /** @type {HTMLButtonElement} */ (c).disabled = !on;
    bar.title = on ? "" : "The room's admins control playback";
    if (!on && pen.classList.contains("on")) pen.click();
    applyView();
  };
  allow(false);
  fetch(new URL("me", HOME))
    .then(r => r.json())
    .then(
      m => {
        me.id = m.id;
        allow(m.admin === true);
        showPeople();
      },
      () => {},
    );
  const showPeople = people(allow);
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
  drawing(pen);
  env.keys.addEventListener("keydown", e => {
    const target = /** @type {Element} */ (e.composedPath()[0]);
    if (
      !admin ||
      !last ||
      e.ctrlKey ||
      e.metaKey ||
      e.altKey ||
      target.closest?.("input:not([type=range]), select, textarea")
    )
      return;
    // Space on a focused button presses that button instead.
    if (e.key === " " && !target.closest?.("button")) control({ paused: !last.paused });
    else if (e.key === "d") pen.click();
    else if (e.key === "ArrowLeft") control({ seek: Math.max(0, Math.round(at() - STEP_MS)) });
    else if (e.key === "ArrowRight") control({ seek: Math.round(at() + STEP_MS) });
    else return;
    e.preventDefault();
  });
}

// The pen: while on, dragging on the map draws (in map coordinates) instead of moving the view.
/** @param {HTMLButtonElement} pen */
function drawing(pen) {
  /** @type {{ id: number; points: [number, number][]; last: [number, number] | null } | null} */
  let stroke = null;
  /** @type {Promise<unknown>} */
  let sent = Promise.resolve();
  pen.onclick = () => env.el.classList.toggle("drawing", pen.classList.toggle("on"));
  const send = () => {
    if (!stroke?.points.length) return;
    const body = JSON.stringify({ stroke: stroke.id, points: stroke.points.splice(0, 256) });
    // In order: a stroke's pieces join up end to end.
    sent = sent
      .then(() =>
        fetch(new URL("draw", HOME), { method: "POST", headers: { "Content-Type": "application/json" }, body }),
      )
      .catch(() => {});
  };
  /** @param {PointerEvent} e */
  const add = e => {
    if (!stroke) return;
    const r = canvas.getBoundingClientRect(),
      x = e.clientX - r.left,
      y = e.clientY - r.top;
    if (stroke.last && Math.hypot(x - stroke.last[0], y - stroke.last[1]) < MIN_STEP_PX) return;
    stroke.last = [x, y];
    const [dx, dy] = screenOffset(x - r.width / 2, y - r.height / 2, state.view.scale);
    /** @type {[number, number]} */
    const point = [Math.round(state.view.cx + dx), Math.round(state.view.cy + dy)];
    stroke.points.push(point);
    drawOwn({ stroke: stroke.id, points: [point], by: me.id });
  };
  canvas.addEventListener(
    "pointerdown",
    e => {
      if (!pen.classList.contains("on") || e.button !== 0) return;
      // Before the view's own handlers: no panning, following or mouse events while drawing.
      e.preventDefault();
      e.stopImmediatePropagation();
      canvas.setPointerCapture(e.pointerId);
      stroke = { id: Math.floor(Math.random() * 2 ** 31), points: [], last: null };
      add(e);
    },
    { capture: true },
  );
  canvas.addEventListener("pointermove", add);
  const end = () => {
    send();
    stroke = null;
  };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
  setInterval(send, SEND_MS);
}

// Who is in the room, with the viewer's own name to set; admins make others admin or not.
/** @param {(admin: boolean) => void} allow @returns {() => void} shows the list again */
function people(allow) {
  const list = $("peoplelist"),
    name = /** @type {HTMLInputElement} */ ($("myname"));
  $("people").hidden = false;
  try {
    name.value = (env.storage && localStorage.getItem(NAME_KEY)) || "";
  } catch {}
  // The stream opens with it, so the room knows the name from the start.
  if (name.value) streamParams.set("name", name.value);
  name.onchange = () => {
    post("name", { name: name.value });
    streamParams.set("name", name.value);
    try {
      if (env.storage) localStorage.setItem(NAME_KEY, name.value);
    } catch {}
  };
  name.onkeydown = e => {
    if (e.key === "Enter") name.blur();
  };
  /** @type {import("../types/stream").PeopleLine | null} */
  let latest = null;
  peopleListeners.add(m => {
    latest = m;
    show(m);
  });
  return () => latest && show(latest);
  /** @param {import("../types/stream").PeopleLine} m */
  function show(m) {
    const self = m.people.find(p => p.id === me.id);
    if (self && self.admin !== me.admin) allow(self.admin);
    list.replaceChildren(
      ...m.people.map(p => {
        const li = document.createElement("li"),
          nm = document.createElement("span"),
          tag = document.createElement("span");
        nm.className = "nm";
        nm.textContent = p.name;
        tag.className = "tag";
        tag.textContent = [p.id === me.id && "you", p.owner ? "owner" : p.admin && "admin"].filter(Boolean).join(", ");
        if (p.admin) {
          const ink = document.createElement("i");
          ink.className = "ink";
          ink.style.background = inkOf(p.id);
          li.append(ink);
        }
        li.append(nm, tag);
        if (me.admin && !p.owner && p.id !== me.id) {
          const b = document.createElement("button");
          b.textContent = p.admin ? "Remove admin" : "Make admin";
          b.onclick = () => post("admin", { person: p.id, admin: !p.admin });
          li.append(b);
        }
        return li;
      }),
    );
  }
}

// How everyone sees the map, as an admin sets it: each their own camera, auto zoom, following
// one player, or the admin's own view. The admin who set it leads with their own camera, and
// their pan, zoom or follow is sent on as it changes; everyone else is locked to it.
/** @type {import("../types/stream").ViewLine} */
let view = { type: "view", mode: "free", by: 0 };
const leading = () => me.admin && view.by === me.id;
// How often a leading admin's camera is sent on, at most, in milliseconds.
const VIEW_MS = 100;

function applyView() {
  const lead = leading();
  state.locked = view.mode !== "free" && !lead;
  state.mirror = !lead && view.mode === "admin" ? view : null;
  if (state.locked) $("autozoom").hidden = true;
  if (lead) return;
  if (view.mode === "auto") {
    if (state.follow != null) setFollow(null);
    setAutoZoom(true);
  } else if (view.mode === "follow") {
    if (state.follow !== view.player) setFollow(view.player);
  } else if (view.mode === "admin") {
    if (state.follow != null) setFollow(null);
    setAutoZoom(false);
  }
}

// What a leading admin's camera says for `mode`; null when following nobody.
/** @param {string} mode */
function viewOf(mode) {
  if (mode === "follow") return state.follow == null ? null : { mode, player: state.follow };
  if (mode !== "admin") return { mode };
  const v = state.view,
    { w, h } = viewArea(),
    f = focusOf(v);
  const r = (/** @type {number} */ x) => Math.round(x * 100) / 100;
  return { mode, cx: r(f.cx), cy: r(f.cy), w: r(w / v.scale), h: r(h / v.scale), rot: r(v.rot) };
}

function roomView() {
  // An overlay (in a stream, say) is nobody in the room.
  if (OVERLAY) streamParams.set("overlay", "1");
  const choose = /** @type {HTMLSelectElement} */ ($("rb-view"));
  let sent = "";
  /** @param {object | null} v */
  const send = v => {
    const body = JSON.stringify(v);
    if (!v || body === sent) return;
    sent = body;
    post("view", v);
  };
  viewListeners.add(m => {
    view = m;
    // Someone else leads now: what this page sent last no longer stands.
    if (m.by !== me.id) sent = "";
    choose.value = m.mode;
    applyView();
  });
  choose.onchange = () => {
    const v = viewOf(choose.value);
    choose.title = v ? "How everyone sees the map" : "Follow a player first, then pick this";
    if (v) send(v);
    else choose.value = view.mode;
  };
  setInterval(() => leading() && send(viewOf(view.mode)), VIEW_MS);
}
