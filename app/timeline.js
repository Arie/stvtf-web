// A room's timeline: rounds in their winner's colour with the score, ubers and drops, and fight
// heat below. Hover says what a mark is; an admin's click jumps just before it.
import { TEAM } from "./core.js";
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
 * @param {{
 *   track: HTMLElement; home: URL; clock: (ms: number) => string;
 *   length: () => number; jump: ((ms: number) => void) | null;
 * }} t `jump` is null for a viewer, who cannot.
 */
export function timeline(t) {
  const lane = el("rb-lane"),
    rounds = el("rb-rounds"),
    span = el("rb-span"),
    heat = /** @type {HTMLCanvasElement} */ (el("rb-heat", "canvas"));
  rounds.append(span);
  t.track.prepend(rounds);
  const strip = el("rb-heat-wrap");
  strip.append(heat);
  t.track.append(lane, strip);
  /** @type {Mark[]} */
  const marks = [];
  /** @type {number[]} */
  let starts = [];
  // The marks fetched (by the room's count, which goes on past those dropped), and what is kept
  // (from `start`) of the map that began at `origin`.
  let fetched = 0,
    fetching = false,
    start = 0,
    origin = 0;

  // The marks the room has that this page does not yet.
  /** @param {number} all */
  const more = async all => {
    if (fetching || all <= fetched) return;
    fetching = true;
    try {
      const r = await (await fetch(new URL(`marks?from=${fetched}`, t.home))).json();
      marks.push(...r.marks);
      fetched = r.all;
    } catch {}
    fetching = false;
    draw();
  };

  // Marks are moved, never laid out again, as a live match's timeline grows (place): a transform
  // glides between pixels at the same width. The rounds and the fight strip are laid out for one
  // stretch of time and scaled to the next.
  /** @type {[HTMLElement, number][]} */
  let placed = [];
  let spanTo = 1,
    heatFrom = 0,
    heatTo = 1,
    at = { len: 0, width: 0 };
  /** @param {number} len @param {number} width */
  const place = (len, width) => {
    at = { len, width };
    const all = Math.max(1, len - start),
      px = (/** @type {number} */ ms) => ((ms - start) / all) * width;
    for (const [e, ms] of placed) e.style.transform = `translate3d(${px(ms)}px, 0, 0)`;
    span.style.transform = `scaleX(${(spanTo - start) / all})`;
    heat.style.transform = `translate3d(${px(heatFrom)}px, 0, 0) scaleX(${(heatTo - heatFrom) / all})`;
  };

  /** @param {HTMLElement} e @param {Mark} m @param {string} what */
  const mark = (e, m, what) => {
    placed.push([e, m.at]);
    e.style.setProperty("--team", TEAM[m.team] ?? TEAM.none);
    e.title = `${what} · ${t.clock(m.at - origin)}${t.jump ? " · click to watch it" : ""}`;
    const jump = t.jump;
    if (jump) e.onclick = () => jump(Math.max(start, m.at - BEFORE_MS));
    return e;
  };

  // Marks are made once (so clicks land) and only moved as a live match grows.
  let drawn = 0,
    laid = "",
    heated = 0;
  function draw(again = false) {
    if (again) {
      lane.replaceChildren();
      placed = [];
      drawn = 0;
    }
    const before = drawn,
      all = joined(starts, marks, start);
    for (; drawn < marks.length; drawn++) {
      const m = marks[drawn],
        e = m.at >= start ? make(m, all, mark) : null;
      if (e) lane.append(e);
    }
    // The rounds again when one starts or is won; the fight strip when there are new kills, or
    // every HEAT_MS as it stretches.
    const len = Math.max(start + 1, t.length()),
      now = [all.join(), drawn].join();
    if (again || now !== laid) {
      laid = now;
      spanTo = len;
      span.style.setProperty("--from", String(start));
      span.style.setProperty("--len", String(len - start));
      drawRounds(span, all, marks);
    }
    if (again || drawn !== before || performance.now() - heated > HEAT_MS) {
      heated = performance.now();
      [heatFrom, heatTo] = drawHeat(heat, marks, start, len);
    }
    place(len, at.width);
  }

  return {
    /** Fit the timeline to its length and width (px) now. @param {number} len @param {number} width */
    place(len, width) {
      if (len !== at.len || width !== at.width) place(len, width);
    },
    /**
     * A playback line: the rounds so far, how many marks there are, and what is kept of which map.
     * @param {{ rounds: number[]; marks?: number; start?: number; origin?: number }} m
     */
    heard(m) {
      starts = m.rounds;
      origin = m.origin ?? 0;
      const moved = (m.start ?? 0) !== start;
      start = m.start ?? 0;
      more(m.marks ?? 0);
      draw(moved);
    },
    /** Whether a viewer may jump: an admin. @param {((ms: number) => void) | null} jump */
    allow(jump) {
      t.jump = jump;
      draw(true);
    },
  };
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
