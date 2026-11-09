// A room (/r/<id>/): its playback bar, which its owner drives and everyone else watches; and
// on a match's own page, the button that opens a room on it.
import { $, OVERLAY, PAGE, canvas, setClockRate, state } from "../core/core.js";
import { EMBEDDED } from "../embed/embed.js";
import { clock, moment, playbar } from "./playbar.js";
import { env } from "../core/env.js";
import { setAutoZoom, setFollow } from "../core/input.js";
import {
  EVENTS_URL,
  openStream,
  peopleListeners,
  playbackListeners,
  relayListeners,
  streamParams,
  viewListeners,
} from "../core/stream.js";
import { drawOwn, inkOf, markerFull, setInks, trimOwn } from "./telestrator.js";
import { pushMirror } from "../map/camera.js";
import { RoomView, latestView, morePoints, sender } from "./roomview.js";
import { focusOf, screenOffset, viewArea } from "../map/view.js";
/** @typedef {import("../../types/stream").PlaybackLine} PlaybackLine */

// The closest a stroke's points are, in pixels.
const MIN_STEP_PX = 3;
// How often a match page looks where its shared playback is, in milliseconds.
const SHARED_MS = 1000;
// How long the link button says it copied, in milliseconds.
const COPIED_MS = 1500;
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

// The link button by the map's name: copies the link `url` makes (`what` it is), a check for a
// moment once it has.
/** @param {string} what @param {() => URL} url */
function copyLink(what, url) {
  const link = $("copylink");
  Object.assign(link, { hidden: false, title: what });
  link.setAttribute("aria-label", what);
  link.onclick = () => copyText(url().href).then(ok => ok && copied(link));
}

// Copies `text`: through the clipboard API where the page may use it (HTTPS, localhost), else by
// copying a selection, the old way, which a page over plain HTTP (on a LAN) may still do.
/** @param {string} text @returns {Promise<boolean>} whether it did */
async function copyText(text) {
  try {
    if (navigator.clipboard) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {}
  const area = Object.assign(document.createElement("textarea"), { value: text, readOnly: true });
  area.style.cssText = "position: fixed; top: 0; left: 0; opacity: 0";
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
function copied(button) {
  button.classList.add("copied");
  setTimeout(() => button.classList.remove("copied"), COPIED_MS);
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
  const showPeople = people(allow);
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
    if (own)
      return fetch(new URL("control", own), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(c),
      });
    const now = bar.state();
    const body = { at: c.seek ?? Math.round(bar.at()), speed: c.speed ?? now.speed, paused: c.paused ?? now.paused };
    starting = fetch(new URL("rewind", HOME), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
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
  copyLink("Copy a link to this moment", () => {
    const url = new URL(location.href);
    url.search = `?t=${clock(Math.max(0, bar.since()))}`;
    history.replaceState(null, "", url);
    return url;
  });
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
  // Points the room took of each marker stroke, and strokes it refused (the map full).
  /** @type {Map<number, number>} */
  const taken = new Map();
  /** @type {Set<number>} */
  const refused = new Set();
  const send = sender(async (/** @type {import("./roomview.js").Strokes} */ s) => {
    if (refused.has(s.stroke)) return;
    const r = await post("draw", s);
    if (!s.keep) return;
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
  const end = () => (stroke = null);
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
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

// How to watch the room in TF2, when it is relayed: the console line to paste, and a link that
// has Steam connect.
function inTf2() {
  const cmd = $("tf2cmd"),
    copy = $("tf2copy"),
    why = $("tf2why");
  relayListeners.add(m => {
    $("intf2").hidden = !m.connect;
    if (!m.connect) return;
    cmd.textContent = `connect ${m.connect}; password "${m.password}"`;
    // Steam's link takes an IP address, not a name: none without one.
    const open = /** @type {HTMLAnchorElement} */ ($("tf2open"));
    open.hidden = !m.address;
    if (m.address) open.href = `steam://connect/${m.address}/${m.password}`;
    why.hidden = !m.problem;
    why.textContent = m.problem ? `Cannot be played in TF2: ${m.problem}` : "";
  });
  copy.onclick = () => copyText(cmd.textContent ?? "").then(ok => ok && copied(copy));
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
  /** @type {import("../../types/stream").PeopleLine | null} */
  let latest = null;
  peopleListeners.add(m => {
    latest = m;
    show(m);
  });
  return () => latest && show(latest);
  /** @param {import("../../types/stream").PeopleLine} m */
  function show(m) {
    const self = m.people.find(p => p.id === me.id);
    if (self && self.admin !== me.admin) allow(self.admin);
    setInks(m.people);
    names.clear();
    for (const p of m.people) names.set(p.id, p.name);
    showLead();
    // One's own name is edited in place, unless signed in; a list redrawn while typing keeps it.
    const typing = document.activeElement === name ? [name.selectionStart, name.selectionEnd] : null;
    list.replaceChildren(
      ...m.people.map(p => {
        const li = document.createElement("li"),
          own = p.id === me.id && !p.signed,
          // Signed in with serveme: their name leads to their Steam profile.
          nm = /^\d+$/.test(p.steamid ?? "")
            ? steamLink(/** @type {string} */ (p.steamid))
            : document.createElement(own ? "input" : "span"),
          tag = document.createElement("span");
        if (own) {
          name.placeholder = p.name;
          // A pen says it can be changed.
          const pen = Object.assign(document.createElement("span"), { className: "rename", textContent: "✎" });
          pen.title = "Change your name";
          pen.onclick = () => name.focus();
          li.append(name, pen);
        } else {
          nm.className = "nm";
          nm.textContent = p.name;
          li.append(nm);
        }
        if (p.signed) {
          const mark = document.createElement("span");
          mark.className = "signed";
          mark.textContent = "✓";
          mark.title = "Signed in with serveme";
          nm.append(" ", mark);
        }
        // "Arie (you) OWNER": spaces between, so it copies as it reads.
        const you = Object.assign(document.createElement("span"), { className: "you", textContent: "(you)" }),
          role = (p.owner && "owner") || (p.admin && "admin") || "";
        tag.className = "tag";
        tag.textContent = role;
        li.append(...(p.id === me.id ? [" ", you] : []), " ", tag);
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
    if (typing) {
      name.focus();
      name.setSelectionRange(typing[0], typing[1]);
    }
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
