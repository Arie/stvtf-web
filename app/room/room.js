// A room (/r/<id>/): its playback bar, which its owner drives and everyone else watches; and
// on a match's own page, the button that opens a room on it.
import { $, OVERLAY, canvas, setClockRate, state } from "../core/core.js";
import { EMBEDDED } from "../embed/embed.js";
import { clock, moment, playbar } from "./playbar.js";
import { env } from "../core/env.js";
import { setAutoZoom, setFollow } from "../core/input.js";
import {
  EVENTS_URL,
  followDropListeners,
  openStream,
  playbackListeners,
  streamParams,
  viewListeners,
} from "../core/stream.js";
import { drawOwn, markerFull, trimOwn } from "./telestrator.js";
import { NOTE_MS, arcTool, circleTool, lightTool, note, worldAt } from "./tools.js";
import { noteKey, noteTool } from "./notes.js";
import { closeViewMenu, viewMenu } from "./viewmenu.js";
import { pushMirror } from "../map/camera.js";
import { HOME, IN_ROOM, me, names, post } from "./home.js";
import { copyLink, copyText } from "./copy.js";
import { inkKey, people, signedIn } from "./people.js";
import { inTf2 } from "./intf2.js";
import { RoomView, latestView, morePoints, sender } from "./roomview.js";
import { focusOf, viewArea } from "../map/view.js";
/** @typedef {import("../../types/stream").PlaybackLine} PlaybackLine */

// The closest a stroke's points are, in pixels.
const MIN_STEP_PX = 3;
// How often a match page looks where its shared playback is, in milliseconds.
const SHARED_MS = 1000;
// How often the address follows playback, in milliseconds.
const MOMENT_MS = 250;

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
  const marker = /** @type {HTMLButtonElement} */ ($("rb-marker")),
    arc = /** @type {HTMLButtonElement} */ ($("rb-arc")),
    circle = /** @type {HTMLButtonElement} */ ($("rb-circle")),
    light = /** @type {HTMLButtonElement} */ ($("rb-light")),
    text = /** @type {HTMLButtonElement} */ ($("rb-text")),
    clear = /** @type {HTMLButtonElement} */ ($("rb-clear"));
  const control = (/** @type {import("./playbar.js").Control} */ c) => post("control", c);
  const bar = playbar({ home: HOME, send: control, live: end => control({ seek: end, paused: false }), room: true });
  playbackListeners.add(m => bar.heard(m));
  /** @param {boolean} on */
  const allow = on => {
    me.admin = on;
    bar.allow(on);
    // A tool in hand is let go, while its button still takes the click.
    for (const t of [marker, arc, circle, light, text]) if (!on && t.classList.contains("on")) t.click();
    for (const c of [marker, arc, circle, light, text, clear, $("rb-view")])
      /** @type {HTMLButtonElement} */ (c).disabled = !on;
    $("roombar").title = on ? "" : "The room's admins control playback";
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
  drawing(marker);
  arcTool(arc, [marker, circle, light, text]);
  circleTool(circle, [marker, arc, light, text]);
  lightTool(light, [marker, arc, circle, text]);
  noteTool(text, [marker, arc, circle, light]);
  noteKey(() => me.admin);
  clear.onclick = () => post("clear", {});
  // d (or m) takes the marker, a a jump arc, t a circle, f the spotlight (n a note, see
  // notes.js); c clears the drawings; r, b and y pick the ink.
  /** @type {Record<string, HTMLButtonElement>} */
  const tools = { d: marker, m: marker, a: arc, t: circle, f: light, c: clear };
  env.keys.addEventListener("keydown", e => {
    const target = /** @type {Element} */ (e.composedPath()[0]);
    const tool = tools[e.key];
    if (!me.admin || e.ctrlKey || e.metaKey || e.altKey || target.closest?.("input, select, textarea")) return;
    if (tool) tool.click();
    else if (!inkKey(e.key)) return;
    e.preventDefault();
  });
}

// This page's address at `since` (?t=); null: live.
/** @param {number | null} since */
function momentLink(since) {
  const url = new URL(location.href);
  url.search = since == null ? "" : `?t=${clock(Math.max(0, since))}`;
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
  // The address follows playback.
  setInterval(() => {
    if (wanted != null || !bar.last) return;
    const url = momentLink(bar.atLive() ? null : bar.since());
    if (url.href !== location.href) history.replaceState(null, "", url);
  }, MOMENT_MS);
}

// The marker: while it is on, dragging draws instead of panning; what it draws stays until cleared.
/** @param {HTMLButtonElement} marker */
function drawing(marker) {
  /** @type {{ id: number; last: [number, number] | null; began: number } | null} */
  let stroke = null;
  marker.onclick = () => env.el.classList.toggle("drawing", marker.classList.toggle("on"));
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
    /** @type {[number, number, number]} */
    const point = [...worldAt(e), Math.round(performance.now() - stroke.began)];
    drawOwn({ stroke: stroke.id, points: [point], by: me.id });
    send({ stroke: stroke.id, points: [point] });
  };
  canvas.addEventListener(
    "pointerdown",
    e => {
      if (!marker.classList.contains("on") || e.button !== 0) return;
      // Before the view's own handlers: no panning, following or mouse events while drawing.
      e.preventDefault();
      e.stopImmediatePropagation();
      if (markerFull()) return note("The map is full of marker: clear it first.");
      canvas.setPointerCapture(e.pointerId);
      stroke = { id: Math.floor(Math.random() * 2 ** 31), last: null, began: performance.now() };
      add(e);
    },
    { capture: true },
  );
  canvas.addEventListener("pointermove", add);
  const end = () => {
    if (stroke) send({ stroke: stroke.id, points: [], end: true });
    stroke = null;
  };
  for (const type of ["pointerup", "pointercancel"]) canvas.addEventListener(type, end);
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
  if (fx.locked !== undefined) env.el.classList.toggle("locked", (state.locked = fx.locked));
  if (state.locked) $("autozoom").hidden = true;
  if (fx.mirror === false) state.mirror = null;
  if (fx.zoom !== undefined) state.followZoom = fx.zoom;
  if (fx.follow !== undefined && state.follow !== fx.follow) setFollow(fx.follow);
  if (fx.auto !== undefined) setAutoZoom(fx.auto);
}

// Sets the view the room bar shows (see viewmenu.js), once it is there.
/** @type {(mode: string) => void} */
let showView = () => {};

// Who leads a view of someone's player or camera, and whom it follows. Only what changed: a
// camera view comes in many times a second.
function showLead() {
  const v = views.line,
    who = $("rb-lead");
  /** @param {HTMLElement} el @param {string} text */
  const say = (el, text) => el.textContent !== text && (el.textContent = text);
  who.hidden = v.mode !== "follow" && v.mode !== "admin";
  say(who, `by ${v.by === me.id ? "you" : (names.get(v.by) ?? "an admin")}`);
  const following = v.mode === "follow" ? (state.players.get(v.player)?.name ?? "a player") : "";
  say($("rb-follow"), `Following ${following}`);
  $("rb-follow").hidden = v.mode !== "follow";
  const view = $("rb-view"),
    title = `How everyone sees the map${who.hidden ? "" : `: ${who.textContent}`}`;
  if (view.title !== title) view.title = title;
  showView(v.mode);
}

/** @param {boolean} on */
function setViewAdmin(on) {
  if (!on) closeViewMenu();
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
  followDropListeners.add(() => views.dropped());
  viewListeners.add(m => {
    const fx = views.hear(m, seen());
    if (fx.mirror && m.mode === "admin") pushMirror(m);
    act(fx);
    showLead();
  });
  showView = viewMenu(mode => {
    const chosen = views.choose(mode, camera);
    if (!chosen) return;
    act(chosen.fx);
    send(chosen.send);
  });
  setInterval(() => {
    const s = views.tick(seen(), camera);
    if (!s) return;
    // Moving it by hand: this admin's camera, not another's replayed.
    if (!s.take) state.mirror = null;
    send(s);
  }, VIEW_MS);
}
