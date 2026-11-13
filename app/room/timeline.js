// A room's timeline: rounds in their winner's colour with the score, ubers and drops, and fight
// heat below. Hover says what a mark is; a click (in a room, an admin's) jumps 3 s before it.
import { TEAM } from "../core/core.js";
/** @typedef {{ at: number; kind: "win" | "uber" | "drop" | "kill"; team: string; who?: string; score?: [number, number] }} Mark */

// A click on a mark lands this far before it.
const BEFORE_MS = 3000;
// The fight strip is redrawn this often (ms) as a live match's grows.
const HEAT_MS = 5000;
// Its bins (ms), and kills in one for full colour.
const HEAT_BIN_MS = 4000,
  HEAT_FULL = 3;
const NAMES = /** @type {Record<string, string>} */ ({ blue: "BLU", red: "RED" });
// A drop: a skull in the team's colour.
const SKULL = `<svg viewBox="0 0 12 12" aria-hidden="true"><path fill="currentColor" fill-rule="evenodd" d="M6 .5C2.9.5.8 2.6.8 5.3c0 1.6.8 2.8 1.9 3.5v1.9c0 .4.3.8.8.8h.9v-1.1h.9v1.1h1.4v-1.1h.9v1.1h.9c.5 0 .8-.4.8-.8V8.8c1.1-.7 1.9-1.9 1.9-3.5C11.2 2.6 9.1.5 6 .5zM4 4.2a1.3 1.3 0 1 1 0 2.6 1.3 1.3 0 0 1 0-2.6zm4 0a1.3 1.3 0 1 1 0 2.6 1.3 1.3 0 0 1 0-2.6zM6 7l.7 1.2H5.3z"/></svg>`;

/** @param {string} cls @param {string} [tag] */
const el = (cls, tag = "div") => Object.assign(document.createElement(tag), { className: cls });

/**
 * @typedef {{
 *   track: HTMLElement; home: URL; clock: (ms: number) => string;
 *   length: () => number; jump: ((ms: number) => void) | null;
 * }} Options `jump` is null for a viewer, who cannot.
 */

/** @param {Options} t */
export function timeline(t) {
  return new Timeline(t);
}

class Timeline {
  /** @param {Options} t */
  constructor(t) {
    this.t = t;
    const { lane, span, heat } = parts(t.track);
    this.lane = lane;
    this.span = span;
    this.heat = heat;
    /** @type {Mark[]} */
    this.marks = [];
    /** @type {number[]} */
    this.starts = [];
    // The marks fetched (by the room's count, which goes on past those dropped), what is kept
    // (from `start`) of the map that began at `origin`, and how far it shows what happened
    // (`upTo`, with the marks `shown`): a live match only as far as this viewer watched, no
    // spoilers.
    this.fetched = 0;
    this.fetching = false;
    this.map = 0;
    // A fetch belongs to one visit of a map: this counts every change of map, A to B to A too.
    this.visit = 0;
    this.start = 0;
    this.origin = 0;
    this.upTo = Infinity;
    this.shown = 0;
    // Marks are made once (so clicks land) and moved, never laid out again, as a live match's
    // timeline grows (place): a transform glides between pixels at the same width. The rounds and
    // the fight strip are laid out for one stretch of time and scaled to the next.
    /** @type {[HTMLElement, number][]} */
    this.placed = [];
    this.spanTo = 1;
    this.heatFrom = 0;
    this.heatTo = 1;
    this.at = { len: 0, width: 0 };
    this.drawn = 0;
    this.laid = "";
    this.heated = 0;
    this.heatShown = 0;
  }

  /** The marks the room has that this page does not yet. @param {number} all */
  async more(all) {
    if (this.fetching || all <= this.fetched) return;
    this.fetching = true;
    const visit = this.visit;
    try {
      const r = await (await fetch(new URL(`marks?from=${this.fetched}`, this.t.home))).json();
      if (visit === this.visit) {
        this.marks.push(...r.marks);
        [this.fetched, this.shown] = [r.all, until(this.marks, this.upTo)];
      }
    } catch {}
    if (visit === this.visit) this.fetching = false;
    this.draw();
  }

  /** @param {number} len @param {number} width */
  placeNow(len, width) {
    this.at = { len, width };
    const all = Math.max(1, len - this.start),
      px = (/** @type {number} */ ms) => ((ms - this.start) / all) * width;
    for (const [e, ms] of this.placed) e.style.transform = `translate3d(${px(ms)}px, 0, 0)`;
    this.span.style.transform = `scaleX(${(this.spanTo - this.start) / all})`;
    this.heat.style.transform = `translate3d(${px(this.heatFrom)}px, 0, 0) scaleX(${(this.heatTo - this.heatFrom) / all})`;
  }

  /** @param {HTMLElement} e @param {Mark} m @param {string} what */
  mark(e, m, what) {
    const t = this.t;
    this.placed.push([e, m.at]);
    e.style.visibility = m.at > this.upTo ? "hidden" : "";
    e.style.setProperty("--team", TEAM[m.team] ?? TEAM.none);
    e.title = `${what} · ${t.clock(m.at - this.origin)}${t.jump ? " · click to watch it" : ""}`;
    const jump = t.jump;
    if (jump) e.onclick = () => jump(Math.max(this.start, m.at - BEFORE_MS));
    return e;
  }

  draw(again = false) {
    if (again) {
      this.lane.replaceChildren();
      this.placed = [];
      this.drawn = 0;
    }
    const before = this.drawn,
      seen = this.marks.slice(0, this.shown),
      all = joined(
        this.starts.filter(st => st <= this.upTo),
        seen,
        this.start,
      );
    const mark = /** @type {(e: HTMLElement, m: Mark, what: string) => HTMLElement} */ (e, m, what) =>
      this.mark(e, m, what);
    for (; this.drawn < this.marks.length; this.drawn++) {
      const m = this.marks[this.drawn],
        e = m.at >= this.start ? make(m, all, mark) : null;
      if (e) this.lane.append(e);
    }
    const len = Math.max(this.start + 1, this.t.length());
    this.layOut(len, all, seen, again || this.drawn !== before);
    this.placeNow(len, this.at.width);
  }

  // The rounds again when one starts or is won; the fight strip when there are new kills, or
  // every HEAT_MS as it stretches.
  /** @param {number} len @param {number[]} all @param {Mark[]} seen @param {boolean} changed */
  layOut(len, all, seen, changed) {
    const now = [all.join(), this.drawn, this.shown].join();
    if (changed || now !== this.laid) {
      this.laid = now;
      this.spanTo = len;
      this.span.style.setProperty("--from", String(this.start));
      this.span.style.setProperty("--len", String(len - this.start));
      drawRounds(this.span, all, seen);
    }
    if (changed || this.heatShown !== this.shown || performance.now() - this.heated > HEAT_MS) {
      [this.heated, this.heatShown] = [performance.now(), this.shown];
      [this.heatFrom, this.heatTo] = drawHeat(this.heat, seen, this.start, len);
    }
  }

  /** Show what happened up to `ms` (Infinity: all). @param {number} ms */
  reveal(ms) {
    if (ms === this.upTo) return;
    [this.upTo, this.shown] = [ms, until(this.marks, ms)];
    hideAfter(this.placed, ms);
    this.draw();
  }

  /** Fit the timeline to its length and width (px) now. @param {number} len @param {number} width */
  place(len, width) {
    if (len !== this.at.len || width !== this.at.width) this.placeNow(len, width);
  }

  /**
   * A playback line: the rounds so far, how many marks there are, and what is kept of which map.
   * @param {{ rounds: number[]; marks?: number; start?: number; origin?: number; map?: number }} m
   */
  heard(m) {
    if ((m.map ?? 0) !== this.map) this.nextMap(m.map ?? 0);
    this.starts = m.rounds;
    this.origin = m.origin ?? 0;
    const moved = (m.start ?? 0) !== this.start;
    this.start = m.start ?? 0;
    this.more(m.marks ?? 0);
    this.draw(moved);
  }

  // Another map: its marks are fetched from the first.
  /** @param {number} map */
  nextMap(map) {
    [this.map, this.marks, this.fetched, this.shown, this.fetching] = [map, [], 0, 0, false];
    this.visit++;
  }

  /** Whether a viewer may jump: an admin. @param {((ms: number) => void) | null} jump */
  allow(jump) {
    this.t.jump = jump;
    this.draw(true);
  }
}

// The timeline's parts on `track`: rounds (in a span scaled as a whole) under the marks' lane,
// and the fight strip below.
/** @param {HTMLElement} track */
function parts(track) {
  const lane = el("rb-lane"),
    rounds = el("rb-rounds"),
    span = el("rb-span"),
    strip = el("rb-heat-wrap"),
    heat = /** @type {HTMLCanvasElement} */ (el("rb-heat", "canvas"));
  rounds.append(span);
  strip.append(heat);
  track.prepend(rounds);
  track.append(lane, strip);
  return { lane, span, heat };
}

// How many of `marks` (in time order) are at `ms` or before.
/** @param {Mark[]} marks @param {number} ms */
function until(marks, ms) {
  const n = marks.findIndex(m => m.at > ms);
  return n < 0 ? marks.length : n;
}

// Hides the marks after `ms`.
/** @param {[HTMLElement, number][]} placed @param {number} ms */
function hideAfter(placed, ms) {
  for (const [e, at] of placed) {
    const hide = at > ms ? "hidden" : "";
    if (e.style.visibility !== hide) e.style.visibility = hide;
  }
}

// A mark's element, made by `mark`, or none for a kill (the strip shows those).
/** @param {Mark} m @param {number[]} starts @param {(e: HTMLElement, m: Mark, what: string) => HTMLElement} mark */
function make(m, starts, mark) {
  const team = NAMES[m.team] ?? "";
  if (m.kind === "uber") return mark(el("rb-uber"), m, `${team} uber${m.who ? `, ${m.who}` : ""}`);
  if (m.kind === "drop")
    return Object.assign(mark(el("rb-drop"), m, `${team} drop${m.who ? `, ${m.who}` : ""}`), { innerHTML: SKULL });
  if (m.kind !== "win") return null;
  const round = starts.filter(s => s <= m.at).length;
  const score = m.score ? ` ${m.score[0]}–${m.score[1]}` : "";
  const what = team ? `${team} won round ${round}` : `Stalemate, round ${round}`;
  return Object.assign(mark(el("rb-win"), m, `${what}${score && `,${score} (BLU–RED)`}`), {
    textContent: score.trim(),
  });
}

// The rounds' starts, and the timeline's start when it was in a round (one joined live) that has
// been won before the first start.
/** @param {number[]} starts @param {Mark[]} marks @param {number} start */
function joined(starts, marks, start) {
  const first = starts[0] ?? Infinity;
  return first > start && marks.some(m => m.kind === "win" && m.at >= start && m.at < first)
    ? [start, ...starts]
    : starts;
}

// The rounds in their winner's colour; grey for a stalemate, none while unplayed.
/** @param {HTMLElement} span @param {number[]} starts @param {Mark[]} marks */
function drawRounds(span, starts, marks) {
  span.replaceChildren(
    ...starts.map((s, i) => {
      const end = starts[i + 1],
        won = marks.find(m => m.kind === "win" && m.at >= s && m.at < (end ?? Infinity));
      const r = el(`rb-round${won ? ` won ${won.team}` : ""}${end == null ? " last" : ""}`);
      r.style.setProperty("--at", String(s));
      if (end != null) r.style.setProperty("--to", String(end));
      return r;
    }),
  );
}

// Kills binned across the strip, in the colour of the team with more; the time it covers.
/** @param {HTMLCanvasElement} heat @param {Mark[]} marks @param {number} start @param {number} length @returns {[number, number]} */
function drawHeat(heat, marks, start, length) {
  // Fixed bins of time, a pixel each: a bin once counted stays put as a live match grows.
  const first = Math.floor(start / HEAT_BIN_MS),
    bins = Math.max(1, Math.ceil(length / HEAT_BIN_MS) - first),
    covers = /** @type {[number, number]} */ ([first * HEAT_BIN_MS, (first + bins) * HEAT_BIN_MS]);
  heat.width = bins;
  heat.height = 1;
  const g = heat.getContext("2d");
  if (!g) return covers;
  const count = Array.from({ length: bins }, () => ({ blue: 0, red: 0 }));
  for (const m of marks) {
    const b = count[Math.floor(m.at / HEAT_BIN_MS) - first];
    if (b && m.kind === "kill" && (m.team === "blue" || m.team === "red")) b[m.team]++;
  }
  g.clearRect(0, 0, bins, 1);
  count.forEach((c, i) => {
    const all = c.blue + c.red;
    if (!all) return;
    g.globalAlpha = Math.min(1, all / HEAT_FULL);
    g.fillStyle = c.blue >= c.red ? TEAM.blue : TEAM.red;
    g.fillRect(i, 0, 1, 1);
  });
  return covers;
}
