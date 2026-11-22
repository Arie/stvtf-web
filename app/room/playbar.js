// The playback bar: play and pause, the timeline with its marks, the time, speed and Live. A room
// drives it with the room's playback; a match page with the shared playback, or a viewer's own.
// The timeline zooms (room/zoom.js): the wheel or a pinch, each viewer's own.
import { $, afterDraw, msPerTick, playbackStatus, setStatus, state } from "../core/core.js";
import { env } from "../core/env.js";
import { atTick } from "./roomview.js";
import { mapState } from "./maps.js";
import { timeline } from "./timeline.js";
import { following, panned, pinched, wheel, within, zoomed } from "./zoom.js";
/** @typedef {import("../../types/stream").PlaybackLine} PlaybackLine */
/** @typedef {{ paused?: boolean; speed?: number; seek?: number }} Control */

// No slow motion in rooms: TF2 shows it as a stutter.
const SPEEDS = [0.25, 0.5, 1, 2, 4],
  ROOM_SPEEDS = [1, 2, 4];
// A local jump is confirmed by playback within JUMP_MS of it, or dropped after JUMP_WAIT_MS.
const JUMP_MS = 1500,
  JUMP_WAIT_MS = 3000;
// A jump to an event in the feed or chat lands this long before it, in milliseconds.
const LEAD_MS = 2000;
// Playback this close to a live match's end is at live, in milliseconds.
const LIVE_MS = 2500;
// How far the arrow keys jump, in milliseconds.
const STEP_MS = 5000;
// A zoomed timeline follows playback that left it once its viewer has not moved it for this long,
// and lifting the fingers of a pinch does not seek for this long, in milliseconds.
const FOLLOW_AFTER_MS = 4000,
  PINCH_MS = 500;

/** @param {number} ms */
export function clock(ms) {
  const s = Math.floor(ms / 1000),
    pad = (/** @type {number} */ n) => String(n).padStart(2, "0");
  if (s < 3600) return `${Math.floor(s / 60)}:${pad(s % 60)}`;
  return `${Math.floor(s / 3600)}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}`;
}

/** A time as `clock` writes it (or seconds) in ms; null if it is not one. @param {string | null} s */
export function moment(s) {
  if (!s || !/^\d+(:\d\d){0,2}$/.test(s)) return null;
  return s.split(":").reduce((all, n) => all * 60 + Number(n), 0) * 1000;
}

/**
 * @typedef {{
 *   home: URL; send: (c: Control) => unknown; live: (length: number) => void;
 *   replay?: (behind: boolean, p: PlaybackLine) => boolean; room?: boolean; moveToMap?: (load: number) => void;
 * }} Options `home`: where the marks are; `send`: a control; `live`: the Live button (a live
 * match's); `replay`: whether the status says replay, from whether playback is behind a live match
 * (default: that); `room`: a room's, with no slow motion; `moveToMap`: Live on a map that is over,
 * going to the next (without it, Live there is `live` too).
 */

/** @param {Options} o */
export function playbar(o) {
  return new Playbar(o);
}

// The speed after `now` among `speeds`, back to the first after the last.
/** @param {number[]} speeds @param {number} now */
export const nextSpeed = (speeds, now) => speeds.find(s => s > now) ?? speeds[0];

// The speed a click steps from at `now`: the one asked for a moment ago, until playback has it (a
// second click comes before the first is heard), or else the one `heard`.
/** @param {number} heard @param {{ speed: number; since: number } | null} asked @param {number} now */
export const speedNow = (heard, asked, now) => (asked && now - asked.since < JUMP_WAIT_MS ? asked.speed : heard);

/** @param {number} s */
export const speedLabel = s => `${{ 0.25: "¼", 0.5: "½" }[s] ?? s}×`;

class Playbar {
  /** @param {Options} o */
  constructor(o) {
    this.o = o;
    this.play = /** @type {HTMLButtonElement} */ ($("rb-play"));
    this.time = /** @type {HTMLInputElement} */ ($("rb-time"));
    this.speed = /** @type {HTMLButtonElement} */ ($("rb-speed"));
    this.speeds = o.room ? ROOM_SPEEDS : SPEEDS;
    this.shown = $("rb-clock");
    this.live = /** @type {HTMLButtonElement} */ ($("rb-live"));
    this.track = $("rb-track");
    $("roombar").hidden = false;
    this.width = widthOf(this.track);
    /** @type {PlaybackLine | null} The latest playback line, heard at `heardAt`. */
    this.last = null;
    this.heardAt = 0;
    this.dragging = false;
    this.allowed = false;
    // How far a live match's timeline shows: as far as this viewer has watched.
    this.seen = 0;
    /** @type {[number, number] | null} What a zoomed timeline shows, moved by its viewer at `viewedAt`. */
    this.view = null;
    this.viewedAt = 0;
    this.pinchedAt = -Infinity;
    /** @type {{ at: number; since: number } | null} A jump made here, shown until playback confirms it. */
    this.jumped = null;
    /** @type {{ speed: number; since: number } | null} A speed asked for here, until playback has it. */
    this.asked = null;
    this.marks = timeline({ track: this.track, home: o.home, clock, length: () => this.length(), jump: null });
    // Every frame, once the map is drawn: a live match's timeline grows smoothly, and the playhead
    // moves smoothly.
    afterDraw.add(() => this.show());
    this.wire();
  }

  wire() {
    const { play, live, speed, time, o } = this;
    play.onclick = () => this.last && this.control({ paused: !this.last.paused });
    live.onclick = () => {
      this.dragging = false;
      const l = this.last;
      // A bar with no map to go to (a viewer's own playback) goes back to the shared one.
      if (!l?.over || !o.moveToMap) return o.live(Math.round(this.length()));
      // The next map, or with none yet the room's own: follow again.
      o.moveToMap(mapState(l).next ?? l.load ?? 0);
    };
    speed.onclick = () => {
      const now = performance.now();
      const next = nextSpeed(this.speeds, speedNow(this.last?.speed ?? 1, this.asked, now));
      this.asked = { speed: next, since: now };
      this.control({ speed: next });
    };
    const pinching = () => performance.now() - this.pinchedAt < PINCH_MS;
    time.oninput = () => {
      this.dragging = !pinching();
      this.show();
    };
    const seek = () => {
      this.dragging = false;
      if (pinching()) return this.show();
      this.control({ seek: Math.round(Number(time.value)) });
    };
    time.onchange = seek;
    // A drag back to where it began fires no change event, so letting go ends the drag here.
    time.onpointerup = time.onpointercancel = time.onblur = () => setTimeout(() => this.dragging && seek());
    const at = () => this.at(),
      control = (/** @type {Control} */ c) => this.control(c);
    this.zooms();
    shortcuts(() => (this.allowed ? this.last : null), control, at);
    timesJump(() => this.allowed, control, at);
  }

  // The wheel zooms the timeline about the pointer, or pans it; two fingers pinch and drag it.
  zooms() {
    const { track } = this;
    const share = (/** @type {number} */ x) => {
      const box = track.getBoundingClientRect();
      return Math.min(1, Math.max(0, (x - box.left) / Math.max(1, box.width)));
    };
    const moved = (/** @type {[number, number] | null} */ view) => {
      [this.view, this.viewedAt] = [view, performance.now()];
      this.show();
    };
    track.addEventListener(
      "wheel",
      e => {
        if (!this.last) return;
        e.preventDefault();
        const whole = this.whole(),
          [from, to] = this.view ?? whole,
          w = wheel(e);
        const at = from + (to - from) * share(e.clientX);
        moved(w.factor ? zoomed(this.view, whole, at, w.factor) : panned(this.view, whole, w.pan * (to - from)));
      },
      { passive: false },
    );
    /** @type {Map<number, number>} Fingers on the track, each where it is (a share of its width). */
    const fingers = new Map();
    /** @type {{ view: [number, number]; at: [number, number] } | null} */
    let pinch = null;
    const two = () => /** @type {[number, number]} */ ([...fingers.values()].slice(0, 2));
    const touch = (/** @type {PointerEvent} */ e) => {
      if (e.pointerType !== "touch" || !this.last) return;
      if (e.type === "pointerdown" || fingers.has(e.pointerId)) fingers.set(e.pointerId, share(e.clientX));
      if (fingers.size < 2) return;
      // Two fingers are no drag of the playhead.
      [this.dragging, this.pinchedAt] = [false, performance.now()];
      pinch ??= { view: this.view ?? this.whole(), at: two() };
      moved(pinched(pinch.view, this.whole(), pinch.at, two()));
    };
    const lift = (/** @type {PointerEvent} */ e) => {
      if (fingers.delete(e.pointerId) && pinch) this.pinchedAt = performance.now();
      pinch = null;
    };
    for (const type of ["pointerdown", "pointermove"])
      track.addEventListener(type, e => touch(/** @type {PointerEvent} */ (e)), { capture: true });
    for (const type of ["pointerup", "pointercancel"])
      track.addEventListener(type, e => lift(/** @type {PointerEvent} */ (e)), { capture: true });
  }

  // The whole timeline: from the start of what is kept to its length now.
  /** @returns {[number, number]} */
  whole() {
    return [this.start(), Math.max(this.start() + 1, this.length())];
  }

  // How long the recording is by now: a live match's grows, between playback lines no further
  // than half of LIVE_MS.
  length() {
    const l = this.last;
    return l ? l.length + (l.live ? Math.min(performance.now() - this.heardAt, LIVE_MS / 2) : 0) : 0;
  }

  // Where what is kept of the map begins. The clock counts from `origin`, where the map began.
  start() {
    return this.last?.start ?? 0;
  }

  /** The clock at `ms`, from the map's start. @param {number} ms */
  clockAt(ms) {
    return clock(Math.max(0, ms - (this.last?.origin ?? 0)));
  }

  /** Where playback is now. */
  at() {
    const l = this.last;
    if (!l) return 0;
    const from = this.jumped ?? { at: l.at, since: this.heardAt };
    const ran = l.paused ? 0 : (performance.now() - from.since) * l.speed;
    return Math.min(this.length(), Math.max(this.start(), from.at + ran));
  }

  /** Whether playback is behind a live match. */
  behind() {
    return Boolean(this.last?.live) && this.length() - this.at() >= LIVE_MS;
  }

  /** Whether playback is at a live match's live. */
  atLive() {
    return Boolean(this.last?.live) && !this.last?.paused && !this.behind();
  }

  /** @param {Control} c */
  control(c) {
    if (c.seek != null) this.jumped = { at: c.seek, since: performance.now() };
    return this.o.send(c);
  }

  show() {
    const l = this.last;
    if (!l) return;
    const behind = this.behind(),
      atLive = this.atLive();
    const len = Math.max(this.start() + 1, this.length());
    // At live the timeline is all played and the playhead, which could only waver at its end, hides.
    const now = atLive ? this.length() : this.at();
    this.showTrack(len, now, atLive);
    // The clock is as wide as it will get, so the timeline does not shift as it counts.
    this.shown.textContent = `${this.clockAt(this.dragging ? Number(this.time.value) : now)} / ${this.clockAt(len)}`;
    const widest = `${this.clockAt(len).length * 2 + 3}ch`;
    if (this.shown.style.minWidth !== widest) this.shown.style.minWidth = widest;
    const over = mapState(l);
    this.live.hidden = !(l.live || l.over);
    this.live.disabled = !this.allowed;
    this.live.title = "Back to live";
    if (l.over && this.o.moveToMap) this.live.title = over.next == null ? "Follow live" : "Next map";
    this.live.classList.toggle("at", atLive);
    say(this.o.replay ?? (b => b), behind, l, this.length() - now);
  }

  /** The timeline `len` long, its marks and the playhead at `now`. @param {number} len @param {number} now @param {boolean} atLive */
  showTrack(len, now, atLive) {
    const { time, track } = this,
      l = /** @type {PlaybackLine} */ (this.last);
    // Zoomed in, it shows a stretch of the timeline, and follows playback again a while after its viewer
    // moved it.
    /** @type {[number, number]} */
    const whole = [this.start(), len];
    if (this.view) this.view = within(this.view, whole);
    if (!this.dragging && performance.now() - this.viewedAt > FOLLOW_AFTER_MS)
      this.view = following(this.view, whole, now);
    const [from, to] = this.view ?? whole;
    time.min = String(from);
    time.max = String(to);
    this.marks.place(len, this.width(), this.view);
    // A live match's timeline shows only as far as this viewer has watched; a demo's, all of it.
    if (l.live) this.seen = Math.max(this.seen, atLive ? len : this.at());
    this.marks.reveal(l.live ? this.seen : Infinity);
    if (!this.dragging) time.value = String(now);
    const at = this.dragging ? Number(time.value) : now,
      away = at < from || at > to;
    // No playhead at live, nor while playback is outside what a zoomed timeline shows.
    track.classList.toggle("at-live", (atLive || away) && !this.dragging);
    track.classList.toggle("zoomed", Boolean(this.view));
    const pct = (/** @type {number} */ ms, /** @type {[number, number]} */ [a, b]) =>
      `${Math.min(100, Math.max(0, (100 * (ms - a)) / (b - a)))}%`;
    track.style.setProperty("--played", pct(at, [from, to]));
    // Where the stretch shown lies in the whole timeline.
    track.style.setProperty("--view-from", pct(from, whole));
    track.style.setProperty("--view-to", pct(to, whole));
  }

  /** Where playback is, from the map's start. */
  since() {
    return this.at() - (this.last?.origin ?? 0);
  }

  /** The speed and pause now. */
  state() {
    return { speed: this.last?.speed ?? 1, paused: this.last?.paused ?? false };
  }

  /** What playback is now. @param {PlaybackLine} m */
  heard(m) {
    [this.last, this.heardAt] = [m, performance.now()];
    const j = this.jumped;
    if (j && (Math.abs(m.at - j.at) < JUMP_MS || this.heardAt - j.since > JUMP_WAIT_MS)) this.jumped = null;
    if (m.speed === this.asked?.speed) this.asked = null;
    this.play.textContent = m.paused ? "▶" : "⏸";
    this.play.title = m.paused ? "Play (space)" : "Pause (space)";
    this.speed.textContent = speedLabel(m.speed);
    this.speed.title = `Speed ${speedLabel(m.speed)}: ${speedLabel(nextSpeed(this.speeds, m.speed))} next`;
    this.time.max = String(m.length);
    this.marks.heard(m);
    this.show();
  }

  /** Whether this viewer drives playback. @param {boolean} on */
  allow(on) {
    this.allowed = on;
    for (const c of [this.play, this.time, this.speed]) c.disabled = !on;
    env.el.classList.toggle("seekable", on);
    this.marks.allow(on ? ms => this.control({ seek: ms }) : null);
  }
}

// `e`'s width, kept by an observer: reading it each frame after the bar's changes would force a reflow.
/** @param {HTMLElement} e */
function widthOf(e) {
  let width = e.clientWidth;
  new ResizeObserver(([r]) => (width = r.contentRect.width)).observe(e);
  return () => width;
}

// Watching a moment that is not live says so: how far behind SourceTV, or a replay (a demo too).
/**
 * @param {(behind: boolean, p: PlaybackLine) => boolean} replay @param {boolean} behind
 * @param {PlaybackLine} p @param {number} by how far behind, in ms
 */
function say(replay, behind, p, by) {
  const status = $("status").textContent ?? "";
  // Behind is short, "−1:23", and says what it is behind on hover.
  let [want, why] = ["live", ""];
  if (p.over) want = mapState(p).status;
  else if (p.paused) want = "paused";
  else if (!p.live) want = "replay";
  else if (replay(behind, p)) [want, why] = behind ? [`−${clock(by)}`, `${clock(by)} behind SourceTV`] : ["replay", ""];
  if (playbackStatus(status) && status !== want) setStatus(want, true, why);
}

// Clicking an event's or a chat line's time watches from a little before it.
/** @param {() => boolean} allowed @param {(c: Control) => unknown} control @param {() => number} at */
function timesJump(allowed, control, at) {
  for (const id of ["feed", "chat"])
    $(id).addEventListener("click", e => {
      const time = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest?.("time[data-tick]"));
      if (!allowed() || !time) return;
      const when = atTick(Number(time.dataset.tick), { at: at(), now: state.lastTick }, msPerTick);
      control({ seek: Math.max(0, when - LEAD_MS) });
    });
}

// Space pauses, the arrow keys jump, while playback is known and this viewer drives it.
/** @param {() => PlaybackLine | null} playing @param {(c: Control) => unknown} control @param {() => number} at */
function shortcuts(playing, control, at) {
  env.keys.addEventListener("keydown", e => {
    const last = playing();
    const target = /** @type {Element} */ (e.composedPath()[0]);
    if (!last || e.ctrlKey || e.metaKey || e.altKey || target.closest?.("input:not([type=range]), select, textarea"))
      return;
    // Space on a focused button presses that button instead.
    if (e.key === " " && !target.closest?.("button")) control({ paused: !last.paused });
    else if (e.key === "ArrowLeft") control({ seek: Math.max(0, Math.round(at() - STEP_MS)) });
    else if (e.key === "ArrowRight") control({ seek: Math.round(at() + STEP_MS) });
    else return;
    e.preventDefault();
  });
}
