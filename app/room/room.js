// A room (/r/<id>/): its playback bar, which its owner drives and everyone else watches; and
// on a match's own page, the button that opens a room on it.
import { $, OVERLAY, canvas, setClockRate, state } from "../core/core.js";
import { EMBEDDED } from "../embed/embed.js";
import { clock, moment, playbar } from "./playbar.js";
import { env } from "../core/env.js";
import { setAutoZoom, setFollow } from "../core/input.js";
import { EVENTS_URL, openStream, playbackListeners, streamParams, viewListeners } from "../core/stream.js";
import { drawOwn, markerFull, trimOwn } from "./telestrator.js";
import { pushMirror } from "../map/camera.js";
import { HOME, IN_ROOM, me, names, post } from "./home.js";
import { copyLink, copyText } from "./copy.js";
import { people, signedIn } from "./people.js";
import { inTf2 } from "./intf2.js";
import { RoomView, latestView, morePoints, sender } from "./roomview.js";
import { focusOf, screenOffset, viewArea } from "../map/view.js";
/** @typedef {import("../../types/stream").PlaybackLine} PlaybackLine */

// The closest a stroke's points are, in pixels.
const MIN_STEP_PX = 3;
// How often a match page looks where its shared playback is, in milliseconds.
const SHARED_MS = 1000;

// The playback bar in a room, the Room button and a viewer's own timeline on a match's page;
// neither on the overlay or another site.
export function initRoom() {
  if (env.component) return;
  if (IN_ROOM) roomView();
  if (OVERLAY) return;
  if (IN_ROOM) roomBar();
  else {
    roomButton();
    if (!EMBEDDED) watchBar();
  }
}

// How often the Watch together button asks again whether a room can open.
const ASK_AGAIN_MS = 5000;
// Offered once the match has a recording to play; greyed out, spinning, while a demo is fetched.
function roomButton() {
  const button = /** @type {HTMLButtonElement} */ ($("roomopen"));
  const [idle, title] = [button.textContent, button.title];
  /** @param {boolean} on */
  const waiting = on => {
    Object.assign(button, { hidden: false, disabled: on, title: on ? "Getting the timeline ready…" : title });
    button.classList.toggle("busy", on);
  };
  const ask = () =>
    fetch(new URL("room", HOME)).then(
      r => {
        if (r.ok) waiting(false);
        else if (r.status === 409) {
          waiting(true);
          setTimeout(ask, ASK_AGAIN_MS);
        } else button.hidden = true;
      },
      () => {},
    );
  ask();
  // Opening can take a few seconds: say so.
  button.onclick = async () => {
    button.disabled = true;
    button.classList.add("busy");
    button.textContent = "Opening…";
    const r = await fetch(new URL("room", HOME), { method: "POST" }).catch(() => null);
    if (r?.ok) {
      const page = new URL((await r.json()).page, location.origin);
      // Its link to send, if the browser still lets this click copy.
      await copyText(page.href);
      return location.assign(page);
    }
    button.classList.remove("busy");
    // Why not, on the button for a moment.
    button.textContent = r ? await r.text() : "Could not reach the server";
    setTimeout(() => {
      button.disabled = false;
      button.textContent = idle;
    }, NOTE_MS);
  };
}

function roomBar() {
  env.el.classList.add("room");
  // A room's link, to send.
  // The room's link, to send, by its name.
  copyLink("Copy the room's link, to send", () => new URL(location.pathname, location.origin));
  const pen = /** @type {HTMLButtonElement} */ ($("rb-pen")),
    marker = /** @type {HTMLButtonElement} */ ($("rb-marker")),
    clear = /** @type {HTMLButtonElement} */ ($("rb-clear"));
  const control = (/** @type {import("./playbar.js").Control} */ c) => post("control", c);
  const bar = playbar({ home: HOME, send: control, live: end => control({ seek: end, paused: false }), room: true });
  playbackListeners.add(m => bar.heard(m));
  /** @param {boolean} on */
  const allow = on => {
    me.admin = on;
    bar.allow(on);
    for (const c of [pen, marker, clear, $("rb-view")]) /** @type {HTMLButtonElement} */ (c).disabled = !on;
    $("roombar").title = on ? "" : "The room's admins control playback";
    for (const t of [pen, marker]) if (!on && t.classList.contains("on")) t.click();
    if (on) $("claim").hidden = true;
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
  const showPeople = people(allow, showLead);
  inTf2();
  drawing(pen, marker);
  clear.onclick = () => post("clear", {});
  // d takes the pen, m the marker.
  /** @type {Record<string, HTMLButtonElement>} */
  const tools = { d: pen, m: marker };
  env.keys.addEventListener("keydown", e => {
    const target = /** @type {Element} */ (e.composedPath()[0]);
    const tool = tools[e.key];
    if (!tool || !me.admin || e.ctrlKey || e.metaKey || e.altKey || target.closest?.("input, select, textarea")) return;
    tool.click();
    e.preventDefault();
  });
}

// This page's address at `since` into the map (?t=), kept in the address bar too.
/** @param {number} since */
function momentLink(since) {
  const url = new URL(location.href);
  url.search = `?t=${clock(Math.max(0, since))}`;
  history.replaceState(null, "", url);
  return url;
}

// A match page's timeline: by default the shared playback (live, for a live match); moving it,
// pausing or a speed gives this viewer a playback of their own, which a live match's Live leaves.
function watchBar() {
  env.el.classList.add("room", "watch");
  /** @type {URL | null} */
  let own = null;
  /** @type {Promise<unknown> | null} */
  let starting = null;
  const back = () => {
    own = null;
    setClockRate(1);
    openStream(EVENTS_URL);
    shared();
  };
  /** @param {import("./playbar.js").Control} c */
  const send = async c => {
    if (starting) await starting;
    if (own) return post("control", c, own);
    const now = bar.state();
    const body = { at: c.seek ?? Math.round(bar.at()), speed: c.speed ?? now.speed, paused: c.paused ?? now.paused };
    starting = post("rewind", body)
      .then(r => (r.ok ? r.json() : null))
      .then(r => {
        if (!r) return;
        own = new URL(r.page, location.origin);
        openStream(new URL("events?overlay=1", own).href);
      })
      .catch(() => {})
      .finally(() => (starting = null));
    return starting;
  };
  const bar = playbar({
    home: HOME,
    send,
    live: back,
    // Live for a live match; on a demo, back to the shared playback from one's own.
    // One's own playback is a replay, unless at a live match's live.
    replay: (behind, p) => Boolean(own) && (behind || !p.live),
  });
  bar.allow(true);
  playbackListeners.add(m => own && bar.heard(m));
  // A link to a moment (?t=12:34, from the map's start) watches from there.
  let wanted = moment(new URLSearchParams(location.search).get("t"));
  copyLink("Copy a link to this moment", () => momentLink(bar.since()));
  // Where the shared playback is, while this viewer watches along.
  const shared = () =>
    !own &&
    fetch(new URL("timeline", HOME))
      .then(r => (r.status === 200 ? r.json() : null))
      .then(
        t => {
          if (!t || own) return;
          // Heard as at the moment linked to, if one was: a live match shows no more of itself.
          const at = wanted == null ? t.at : (t.origin ?? 0) + wanted;
          bar.heard({ type: "playback", paused: false, speed: 1, ...t, at });
          if (wanted != null) send({ seek: at });
          wanted = null;
        },
        () => {},
      );
  shared();
  setInterval(shared, SHARED_MS);
}

// Pen and marker: while one is on, dragging draws instead of panning. Pen fades; marker stays.
/** @param {HTMLButtonElement} pen @param {HTMLButtonElement} marker */
function drawing(pen, marker) {
  /** @type {{ id: number; last: [number, number] | null; began: number; keep: boolean } | null} */
  let stroke = null;
  for (const [tool, other] of [
    [pen, marker],
    [marker, pen],
  ])
    tool.onclick = () => {
      other.classList.remove("on");
      env.el.classList.toggle("drawing", tool.classList.toggle("on"));
    };
  // Points the room took of each stroke, and strokes it refused (the map full, or more than TF2 shows).
  /** @type {Map<number, number>} */
  const taken = new Map();
  /** @type {Set<number>} */
  const refused = new Set();
  const send = sender(async (/** @type {import("./roomview.js").Strokes} */ s) => {
    if (refused.has(s.stroke)) return;
    const r = await post("draw", s);
    if (r.ok) return taken.set(s.stroke, (taken.get(s.stroke) ?? 0) + s.points.length);
    refused.add(s.stroke);
    trimOwn(s.stroke, taken.get(s.stroke) ?? 0);
    if (stroke?.id === s.stroke) stroke = null;
    note(await r.text());
  }, morePoints);
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
    const keep = stroke.keep || undefined;
    drawOwn({ stroke: stroke.id, points: [point], by: me.id, keep });
    send({ stroke: stroke.id, points: [point], keep });
  };
  canvas.addEventListener(
    "pointerdown",
    e => {
      const keep = marker.classList.contains("on");
      if (!(keep || pen.classList.contains("on")) || e.button !== 0) return;
      // Before the view's own handlers: no panning, following or mouse events while drawing.
      e.preventDefault();
      e.stopImmediatePropagation();
      if (keep && markerFull()) return note("The map is full of marker: clear it first.");
      canvas.setPointerCapture(e.pointerId);
      stroke = { id: Math.floor(Math.random() * 2 ** 31), last: null, began: performance.now(), keep };
      add(e);
    },
    { capture: true },
  );
  canvas.addEventListener("pointermove", add);
  const end = () => {
    if (stroke) send({ stroke: stroke.id, points: [], keep: stroke.keep || undefined, end: true });
    stroke = null;
  };
  for (const type of ["pointerup", "pointercancel"]) canvas.addEventListener(type, end);
}

// Why something did not happen, in the room bar for a moment.
/** @param {string} text */
function note(text) {
  const el = $("rb-note");
  Object.assign(el, { textContent: text, hidden: false });
  clearTimeout(noteTimer);
  noteTimer = setTimeout(() => (el.hidden = true), NOTE_MS);
}
const NOTE_MS = 5000;
/** @type {ReturnType<typeof setTimeout> | undefined} */
let noteTimer;

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
  if (fx.locked !== undefined) env.el.classList.toggle("locked", (state.locked = fx.locked));
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
