// The playback bar: play and pause, the timeline with its marks, the time, speed and Live. A room
// drives it with the room's playback; a match page with the shared playback, or a viewer's own.
import { $, msPerTick, setStatus, state } from "./core.js";
import { env } from "./env.js";
import { atTick } from "./roomview.js";
import { timeline } from "./timeline.js";
/** @typedef {import("../types/stream").PlaybackLine} PlaybackLine */
/** @typedef {{ paused?: boolean; speed?: number; seek?: number }} Control */

const SPEEDS = [0.25, 0.5, 1, 2, 4];
// A local jump is confirmed by playback within JUMP_MS of it, or dropped after JUMP_WAIT_MS.
const JUMP_MS = 1500,
  JUMP_WAIT_MS = 3000;
// A jump to an event in the feed or chat lands this long before it, in milliseconds.
const LEAD_MS = 2000;
// Playback this close to a live match's end is at live, in milliseconds.
const LIVE_MS = 2500;
// How far the arrow keys jump, in milliseconds.
const STEP_MS = 5000;

/** @param {number} ms */
export function clock(ms) {
  const s = Math.floor(ms / 1000),
    pad = (/** @type {number} */ n) => String(n).padStart(2, "0");
  if (s < 3600) return `${Math.floor(s / 60)}:${pad(s % 60)}`;
  return `${Math.floor(s / 3600)}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}`;
}

/**
 * @param {{
 *   home: URL; send: (c: Control) => unknown; live: (length: number) => void;
 *   liveLabel?: (p: PlaybackLine) => string | null; replay?: (behind: boolean, p: PlaybackLine) => boolean;
 * }} o `home`: where the marks are; `send`: a control; `live`: the Live button;
 * `liveLabel`: its text, or none to hide it (default: "● Live" for a live match); `replay`: whether
 * the status says replay, from whether playback is behind a live match (default: that).
 */
export function playbar(o) {
  const bar = $("roombar"),
    play = /** @type {HTMLButtonElement} */ ($("rb-play")),
    time = /** @type {HTMLInputElement} */ ($("rb-time")),
    speed = /** @type {HTMLSelectElement} */ ($("rb-speed")),
    shown = $("rb-clock"),
    live = /** @type {HTMLButtonElement} */ ($("rb-live")),
    track = $("rb-track");
  bar.hidden = false;
  speed.replaceChildren(
    ...SPEEDS.map(s => Object.assign(document.createElement("option"), { value: String(s), textContent: `${s}×` })),
  );
  /** @type {PlaybackLine | null} */
  let last = null;
  let heard = 0,
    dragging = false,
    allowed = false;
  // How long the recording is by now: a live match's grows, between playback lines no further
  // than half of LIVE_MS.
  const length = () => (last ? last.length + (last.live ? Math.min(performance.now() - heard, LIVE_MS / 2) : 0) : 0);
  // A jump made here, shown until playback confirms it.
  /** @type {{ at: number; since: number } | null} */
  let jumped = null;
  const at = () => {
    if (!last) return 0;
    const from = jumped ?? { at: last.at, since: heard };
    const ran = last.paused ? 0 : (performance.now() - from.since) * last.speed;
    return Math.min(length(), from.at + ran);
  };
  /** @param {Control} c */
  const control = c => {
    if (c.seek != null) jumped = { at: c.seek, since: performance.now() };
    return o.send(c);
  };
  const liveLabel = o.liveLabel ?? (p => (p.live ? "● Live" : null));
  const show = () => {
    if (!last) return;
    const behind = Boolean(last.live) && length() - at() >= LIVE_MS,
      atLive = Boolean(last.live) && !last.paused && !behind;
    time.max = String(length());
    track.style.setProperty("--len", String(Math.max(1, length())));
    // At live the timeline is all played and the playhead, which could only waver at its end, hides.
    const now = atLive ? length() : at();
    if (!dragging) time.value = String(now);
    track.classList.toggle("at-live", atLive && !dragging);
    track.style.setProperty("--played", `${(100 * Number(time.value)) / Math.max(1, length())}%`);
    shown.textContent = `${clock(dragging ? Number(time.value) : now)} / ${clock(length())}`;
    const label = liveLabel(last);
    live.hidden = !label;
    if (label && live.textContent !== label) live.textContent = label;
    live.classList.toggle("at", atLive);
    // Watching a moment that is not live says so.
    const status = $("status").textContent ?? "";
    const want = (o.replay ?? (b => b))(behind, last) ? "replay" : "live";
    if (!last.paused && ["live", "replay"].includes(status) && status !== want) setStatus(want, true);
  };
  // Every frame: a live match's timeline grows smoothly, and the playhead moves smoothly.
  const frame = () => {
    show();
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
  const marks = timeline({ track, home: o.home, clock, length, jump: null });
  play.onclick = () => last && control({ paused: !last.paused });
  live.onclick = () => o.live(Math.round(length()));
  speed.onchange = () => control({ speed: Number(speed.value) });
  time.oninput = () => {
    dragging = true;
    show();
  };
  time.onchange = () => {
    dragging = false;
    control({ seek: Math.round(Number(time.value)) });
  };
  shortcuts(() => (allowed ? last : null), control, at);
  timesJump(() => allowed, control, at);

  return {
    control,
    at,
    /** The speed and pause now. */
    state: () => ({ speed: last?.speed ?? 1, paused: last?.paused ?? false }),
    /** What playback is now. @param {PlaybackLine} m */
    heard(m) {
      [last, heard] = [m, performance.now()];
      if (jumped && (Math.abs(m.at - jumped.at) < JUMP_MS || heard - jumped.since > JUMP_WAIT_MS)) jumped = null;
      play.textContent = m.paused ? "▶" : "⏸";
      play.title = m.paused ? "Play (space)" : "Pause (space)";
      speed.value = String(m.speed);
      time.max = String(m.length);
      marks.heard(m);
      show();
    },
    /** Whether this viewer drives playback. @param {boolean} on */
    allow(on) {
      allowed = on;
      for (const c of [play, time, speed, live]) c.disabled = !on;
      env.el.classList.toggle("seekable", on);
      marks.allow(on ? ms => control({ seek: ms }) : null);
    },
  };
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
