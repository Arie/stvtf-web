// A room's timeline: rounds in their winner's colour with the score, ubers and drops, and fight
// heat below. Hover says what a mark is; an admin's click jumps just before it.
import { TEAM } from "./core.js";
/** @typedef {{ at: number; kind: "win" | "uber" | "drop" | "kill"; team: string; who?: string; score?: [number, number] }} Mark */

// A click on a mark lands this far before it.
const BEFORE_MS = 3000;
// Fight strip bins in pixels, and kills per bin for full colour.
const HEAT_PX = 4,
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
    heat = /** @type {HTMLCanvasElement} */ (el("rb-heat", "canvas"));
  t.track.prepend(rounds);
  t.track.append(lane, heat);
  /** @type {Mark[]} */
  const marks = [];
  /** @type {number[]} */
  let starts = [];
  let fetching = false;

  // The marks the room has that this page does not yet.
  /** @param {number} all */
  const more = async all => {
    if (fetching || all <= marks.length) return;
    fetching = true;
    try {
      const r = await fetch(new URL(`marks?from=${marks.length}`, t.home));
      marks.push(...(await r.json()).marks);
    } catch {}
    fetching = false;
    draw();
  };

  /** @param {number} at */
  const left = at => `${(100 * at) / Math.max(1, t.length())}%`;
  /** @param {HTMLElement} e @param {Mark} m @param {string} what */
  const mark = (e, m, what) => {
    e.style.left = left(m.at);
    e.style.setProperty("--team", TEAM[m.team] ?? TEAM.none);
    e.title = `${what} · ${t.clock(m.at)}${t.jump ? " · click to watch it" : ""}`;
    const jump = t.jump;
    if (jump) e.onclick = () => jump(Math.max(0, m.at - BEFORE_MS));
    return e;
  };

  // A mark's element, or none for a kill (the strip shows those).
  /** @param {Mark} m */
  function make(m) {
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

  // Marks are made once (so clicks land) and only moved as a live match grows.
  let drawn = 0,
    laid = "";
  function draw(again = false) {
    if (again) {
      lane.replaceChildren();
      drawn = 0;
    }
    const before = drawn;
    for (; drawn < marks.length; drawn++) {
      const e = make(marks[drawn]);
      if (e) lane.append(Object.assign(e, { at: marks[drawn].at }));
    }
    const now = [Math.round(t.length()), starts.join()].join();
    if (!again && now === laid && drawn === before) return;
    laid = now;
    for (const e of /** @type {HTMLCollectionOf<HTMLElement & { at: number }>} */ (lane.children))
      e.style.left = left(e.at);
    rounds.replaceChildren(
      ...starts.map((s, i) => {
        // The winner's colour; grey for a stalemate, none while unplayed.
        const end = starts[i + 1] ?? Infinity,
          won = marks.find(m => m.kind === "win" && m.at >= s && m.at < end);
        const r = el(won ? `rb-round won ${won.team}` : "rb-round");
        r.style.left = left(s);
        r.style.width = left(Math.min(end, t.length()) - s);
        return r;
      }),
    );
    drawHeat(heat, marks, t.length());
  }

  return {
    /** A playback line: the rounds so far, and how many marks there are. @param {{ rounds: number[]; marks?: number }} m */
    heard(m) {
      starts = m.rounds;
      more(m.marks ?? 0);
      draw();
    },
    /** Whether a viewer may jump: an admin. @param {((ms: number) => void) | null} jump */
    allow(jump) {
      t.jump = jump;
      draw(true);
    },
  };
}

// Kills binned across the strip, in the colour of the team with more.
/** @param {HTMLCanvasElement} heat @param {Mark[]} marks @param {number} length */
function drawHeat(heat, marks, length) {
  const w = Math.max(1, Math.round(heat.clientWidth)),
    bins = Math.ceil(w / HEAT_PX),
    len = Math.max(1, length);
  heat.width = w;
  heat.height = 1;
  const g = heat.getContext("2d");
  if (!g) return;
  const count = Array.from({ length: bins }, () => ({ blue: 0, red: 0 }));
  for (const m of marks) {
    const b = count[Math.min(bins - 1, Math.floor((m.at / len) * bins))];
    if (m.kind === "kill" && (m.team === "blue" || m.team === "red")) b[m.team]++;
  }
  g.clearRect(0, 0, w, 1);
  count.forEach((c, i) => {
    const all = c.blue + c.red;
    if (!all) return;
    g.globalAlpha = Math.min(1, all / HEAT_FULL);
    g.fillStyle = c.blue >= c.red ? TEAM.blue : TEAM.red;
    g.fillRect(i * HEAT_PX, 0, HEAT_PX, 1);
  });
}
