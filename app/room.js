// A room (/r/<id>/): its playback bar, which its owner drives and everyone else watches; and
// on a match's own page, the button that opens a room on it.
import { $, OVERLAY, PAGE, canvas, msPerTick, state } from "./core.js";
import { env } from "./env.js";
import { setAutoZoom, setFollow } from "./input.js";
import { peopleListeners, playbackListeners, streamParams, viewListeners } from "./stream.js";
import { drawOwn, inkOf, setInks } from "./telestrator.js";
import { timeline } from "./timeline.js";
import { pushMirror } from "./camera.js";
import { RoomView, atTick, latestView, morePoints, sender } from "./roomview.js";
import { focusOf, screenOffset, viewArea } from "./view.js";
/** @typedef {import("../types/stream").PlaybackLine} PlaybackLine */

const SPEEDS = [0.25, 0.5, 1, 2, 4];
// The closest a stroke's points are, in pixels.
const MIN_STEP_PX = 3;
// A jump to an event in the feed or chat lands this long before it, in milliseconds.
const LEAD_MS = 2000;
// Playback this close to a live match's end is at live, in milliseconds.
const LIVE_MS = 2500;
// How far the arrow keys jump, in milliseconds.
const STEP_MS = 5000;
export const IN_ROOM = /\/r\/[0-9a-f]{32}\/$/.test(PAGE.replace(/overlay\/?$/, ""));
// The room's or match's own address, which its routes are under.
const HOME = new URL(PAGE.replace(/(overlay\/?|index\.html)?$/, ""), env.base);

// Who this page is in the room: its number, and whether it is an admin.
const me = { id: 0, admin: false };
// Everyone's names, by number.
/** @type {Map<number, string>} */
const names = new Map();
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
    live = /** @type {HTMLButtonElement} */ ($("rb-live")),
    pen = /** @type {HTMLButtonElement} */ ($("rb-pen")),
    track = $("rb-track");
  bar.hidden = false;
  speed.replaceChildren(...SPEEDS.map(s => new Option(`${s}×`, String(s))));
  /** @type {PlaybackLine | null} */
  let last = null;
  let heard = 0,
    dragging = false,
    admin = false;
  // Where playback is, and how long the recording is, by now: a live match's grows as it goes.
  // Between two playback lines, a second apart: no further, when a match sends little.
  const length = () => (last ? last.length + (last.live ? Math.min(performance.now() - heard, LIVE_MS / 2) : 0) : 0);
  const at = () => {
    if (!last) return 0;
    const ran = last.paused ? 0 : (performance.now() - heard) * last.speed;
    return Math.min(length(), last.at + ran);
  };
  /** @param {{ paused?: boolean; speed?: number; seek?: number }} c */
  const control = c => post("control", c);
  const show = () => {
    if (!last) return;
    time.max = String(length());
    if (!dragging) time.value = String(at());
    track.style.setProperty("--played", `${(100 * Number(time.value)) / Math.max(1, length())}%`);
    shown.textContent = `${clock(dragging ? Number(time.value) : at())} / ${clock(length())}`;
    // At live: within a couple of seconds of its end, and playing.
    live.hidden = !last.live;
    live.classList.toggle("at", !last.paused && length() - at() < LIVE_MS);
  };
  playbackListeners.add(m => {
    [last, heard] = [m, performance.now()];
    play.textContent = m.paused ? "▶" : "⏸";
    play.title = m.paused ? "Play (space)" : "Pause (space)";
    speed.value = String(m.speed);
    time.max = String(m.length);
    marks.heard(m);
    show();
  });
  setInterval(show, 250);
  const marks = timeline({ track, home: HOME, clock, length, jump: null });
  /** @param {boolean} on */
  const allow = on => {
    admin = me.admin = on;
    for (const c of [play, time, speed, pen, live, $("rb-view")]) /** @type {HTMLButtonElement} */ (c).disabled = !on;
    bar.title = on ? "" : "The room's admins control playback";
    if (!on && pen.classList.contains("on")) pen.click();
    if (on) $("claim").hidden = true;
    env.el.classList.toggle("seekable", on);
    marks.allow(on ? ms => control({ seek: ms }) : null);
    setViewAdmin(on);
  };
  allow(false);
  fetch(new URL("me", HOME))
    .then(r => r.json())
    .then(
      m => {
        me.id = m.id;
        allow(m.admin === true);
        signedIn(m);
        showPeople();
      },
      () => {},
    );
  const showPeople = people(allow);
  play.onclick = () => last && control({ paused: !last.paused });
  live.onclick = () => control({ seek: Math.round(length()), paused: false });
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
  shortcuts(() => (admin ? last : null), control, at, pen);
  timesJump(() => admin, control, at);
}

// An admin clicking an event's or a chat line's time watches from a little before it.
/** @param {() => boolean} admin @param {(c: { seek: number }) => void} control @param {() => number} at */
function timesJump(admin, control, at) {
  for (const id of ["feed", "chat"])
    $(id).addEventListener("click", e => {
      const time = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest?.("time[data-tick]"));
      if (!admin() || !time) return;
      const when = atTick(Number(time.dataset.tick), { at: at(), now: state.lastTick }, msPerTick);
      control({ seek: Math.max(0, when - LEAD_MS) });
    });
}

// Space pauses, d takes the pen, the arrow keys jump: an admin's, while playback is known.
/**
 * @param {() => PlaybackLine | null} playing @param {(c: { paused?: boolean; seek?: number }) => void} control
 * @param {() => number} at @param {HTMLButtonElement} pen
 */
function shortcuts(playing, control, at, pen) {
  env.keys.addEventListener("keydown", e => {
    const last = playing();
    const target = /** @type {Element} */ (e.composedPath()[0]);
    if (!last || e.ctrlKey || e.metaKey || e.altKey || target.closest?.("input:not([type=range]), select, textarea"))
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
  /** @type {{ id: number; last: [number, number] | null; began: number } | null} */
  let stroke = null;
  pen.onclick = () => env.el.classList.toggle("drawing", pen.classList.toggle("on"));
  const send = sender((/** @type {import("./roomview.js").Strokes} */ s) => post("draw", s), morePoints);
  /** @param {PointerEvent} e */
  const add = e => {
    if (!stroke) return;
    const r = canvas.getBoundingClientRect(),
      x = e.clientX - r.left,
      y = e.clientY - r.top;
    if (stroke.last && Math.hypot(x - stroke.last[0], y - stroke.last[1]) < MIN_STEP_PX) return;
    stroke.last = [x, y];
    const [dx, dy] = screenOffset(x - r.width / 2, y - r.height / 2, state.view.scale);
    /** @type {[number, number, number]} */
    const point = [
      Math.round(state.view.cx + dx),
      Math.round(state.view.cy + dy),
      Math.round(performance.now() - stroke.began),
    ];
    drawOwn({ stroke: stroke.id, points: [point], by: me.id });
    send({ stroke: stroke.id, points: [point] });
  };
  canvas.addEventListener(
    "pointerdown",
    e => {
      if (!pen.classList.contains("on") || e.button !== 0) return;
      // Before the view's own handlers: no panning, following or mouse events while drawing.
      e.preventDefault();
      e.stopImmediatePropagation();
      canvas.setPointerCapture(e.pointerId);
      stroke = { id: Math.floor(Math.random() * 2 ** 31), last: null, began: performance.now() };
      add(e);
    },
    { capture: true },
  );
  canvas.addEventListener("pointermove", add);
  const end = () => (stroke = null);
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
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
    setInks(m.people);
    names.clear();
    for (const p of m.people) names.set(p.id, p.name);
    showLead();
    list.replaceChildren(
      ...m.people.map(p => {
        const li = document.createElement("li"),
          // Signed in with serveme: their name leads to their Steam profile.
          nm = /^\d+$/.test(p.steamid ?? "")
            ? steamLink(/** @type {string} */ (p.steamid))
            : document.createElement("span"),
          tag = document.createElement("span");
        nm.className = "nm";
        nm.textContent = p.name;
        if (p.signed) {
          const mark = document.createElement("span");
          mark.className = "signed";
          mark.textContent = "✓";
          mark.title = "Signed in with serveme";
          nm.append(" ", mark);
        }
        tag.className = "tag";
        tag.textContent = [p.id === me.id && "you", p.owner ? "owner" : p.admin && "admin"].filter(Boolean).join(", ");
        li.append(nm, tag);
        if (me.admin && !p.owner && p.id !== me.id) {
          const b = document.createElement("button");
          b.textContent = p.admin ? "Remove admin" : "Make admin";
          b.onclick = () => post("admin", { person: p.id, admin: !p.admin });
          li.append(b);
        }
        // Last, so names line up: an admin's pen colour, which they pick for themselves.
        if (p.admin && p.id === me.id) {
          const pick = document.createElement("input");
          pick.type = "color";
          pick.className = "ink";
          pick.title = "Your pen colour";
          pick.value = inkOf(p.id);
          pick.onchange = () => post("ink", { ink: pick.value });
          li.append(pick);
        } else if (p.admin) {
          const ink = document.createElement("i");
          ink.className = "ink";
          ink.title = "Their pen colour";
          ink.style.background = inkOf(p.id);
          li.append(ink);
        }
        return li;
      }),
    );
  }
}

// How everyone sees the map, as its admins set it (see roomview.js for the rules), on this page.
// This page among its admin's pages (two tabs, say): a page leads, not a person.
const views = new RoomView(Math.random().toString(36).slice(2, 12));
// How often an admin's clicks and camera moves are looked at, in milliseconds.
const VIEW_MS = 50;

/** @returns {import("./roomview.js").Seen} */
const seen = () => ({ follow: state.follow, handMoved: state.handMoved, now: performance.now() });

// This page's camera, as an admin's view.
function camera() {
  const v = state.view,
    { w, h } = viewArea(),
    f = focusOf(v);
  const r = (/** @type {number} */ x) => Math.round(x * 100) / 100;
  return { mode: "admin", cx: r(f.cx), cy: r(f.cy), w: r(w / v.scale), h: r(h / v.scale), rot: r(v.rot) };
}

/** What the view has this page do. @param {Partial<import("./roomview.js").Effects>} fx */
function act(fx) {
  if (fx.locked !== undefined) state.locked = fx.locked;
  if (state.locked) $("autozoom").hidden = true;
  if (fx.mirror === false) state.mirror = null;
  if (fx.zoom !== undefined) state.followZoom = fx.zoom;
  if (fx.follow !== undefined && state.follow !== fx.follow) setFollow(fx.follow);
  if (fx.auto !== undefined) setAutoZoom(fx.auto);
}

// Who leads a view of someone's player or camera, and whom it follows. Only what changed: a
// camera view comes in many times a second, and touching the menu while it is open would pull
// its highlight back.
function showLead() {
  const v = views.line,
    who = $("rb-lead");
  /** @param {HTMLElement} el @param {string} text */
  const say = (el, text) => el.textContent !== text && (el.textContent = text);
  who.hidden = v.mode !== "follow" && v.mode !== "admin";
  say(who, `by ${v.by === me.id ? "you" : (names.get(v.by) ?? "an admin")}`);
  const following = v.mode === "follow" ? (state.players.get(v.player)?.name ?? "a player") : "";
  say($("rb-follow"), `Following ${following}`);
  const choose = /** @type {HTMLSelectElement} */ ($("rb-view"));
  if (choose.value !== v.mode) choose.value = v.mode;
}

/** @param {boolean} on */
function setViewAdmin(on) {
  act(views.setAdmin(on, seen()));
  showLead();
}

function roomView() {
  // An overlay (in a stream, say) is nobody in the room.
  if (OVERLAY) streamParams.set("overlay", "1");
  const send = sender(
    (/** @type {import("./roomview.js").Send} */ s) => post("view", { ...s.view, take: s.take, page: views.page }),
    latestView,
  );
  viewListeners.add(m => {
    const fx = views.hear(m, seen());
    if (fx.mirror && m.mode === "admin") pushMirror(m);
    act(fx);
    showLead();
  });
  const choose = /** @type {HTMLSelectElement} */ ($("rb-view"));
  choose.onchange = () => {
    const chosen = views.choose(choose.value, camera);
    if (!chosen) return;
    act(chosen.fx);
    send(chosen.send);
  };
  setInterval(() => {
    const s = views.tick(seen(), camera);
    if (!s) return;
    // Moving it by hand: this admin's camera, not another's replayed.
    if (!s.take) state.mirror = null;
    send(s);
  }, VIEW_MS);
}

/** @param {string} steamid */
function steamLink(steamid) {
  const a = document.createElement("a");
  a.href = `https://steamcommunity.com/profiles/${steamid}/`;
  a.target = "_blank";
  a.rel = "noopener noreferrer";
  a.title = "Steam profile";
  return a;
}

// Signing in with serveme: the name is theirs there, and a streamer may make themselves admin.
/** @param {{ admin: boolean; signed: boolean; streamer: boolean; signin: boolean }} m */
function signedIn(m) {
  const back = encodeURIComponent(location.pathname);
  const signin = /** @type {HTMLAnchorElement} */ ($("signin")),
    signout = /** @type {HTMLAnchorElement} */ ($("signout")),
    claim = /** @type {HTMLButtonElement} */ ($("claim"));
  signin.href = new URL(`signin?return=${back}`, env.base).href;
  signout.href = new URL(`signout?return=${back}`, env.base).href;
  signin.hidden = !m.signin || m.signed;
  signout.hidden = !m.signed;
  $("myname").hidden = m.signed;
  claim.hidden = !m.streamer || m.admin;
  claim.onclick = () => post("claim", {}).then(() => (claim.hidden = true));
}
