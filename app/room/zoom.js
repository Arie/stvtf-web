// The timeline's zoom: the stretch of time it shows, of the whole it has. The wheel (or a pinch)
// zooms about the pointer, a sideways scroll (or shift and the wheel, or two fingers dragged)
// pans; zoomed all the way out it shows the whole again. Each viewer's own. See playbar.js.
/** @typedef {[number, number]} Span from and to, in ms */

// The least it shows, in milliseconds, and how much one notch of the wheel zooms.
export const LEAST_MS = 20_000;
const NOTCH = 1.25;
// Where playback is put in a view that it left: this share of the view in from the left.
const FOLLOW_AT = 0.15;

// `view` kept within `whole`, no shorter than the least; null when it is all of it.
/** @param {Span} view @param {Span} whole @returns {Span | null} */
export function within([from, to], [start, end]) {
  const span = Math.max(Math.min(LEAST_MS, end - start), to - from);
  if (span >= end - start) return null;
  const at = Math.min(Math.max(from, start), end - span);
  return [at, at + span];
}

// `view` (null: the whole) zoomed by `factor` (above 1: in) about time `at`, which stays where
// it is on the track.
/** @param {Span | null} view @param {Span} whole @param {number} at @param {number} factor */
export function zoomed(view, whole, at, factor) {
  const [from, to] = view ?? whole;
  return within([at - (at - from) / factor, at + (to - at) / factor], whole);
}

// `view` moved by `ms`.
/** @param {Span | null} view @param {Span} whole @param {number} ms */
export function panned(view, whole, ms) {
  return view && within([view[0] + ms, view[1] + ms], whole);
}

// `view` with playback at `now` in it: moved, when it is not, so that it is near its left.
/** @param {Span | null} view @param {Span} whole @param {number} now */
export function following(view, whole, now) {
  if (!view || (now >= view[0] && now <= view[1])) return view;
  const span = view[1] - view[0],
    from = now - span * FOLLOW_AT;
  return within([from, from + span], whole);
}

// How much a wheel event zooms (above 1: in), or 0 for one that pans; and how far that pans, as a
// share of what shows. A wheel that reports lines (Firefox) is scaled to match one that reports pixels.
/** @param {{ deltaX: number; deltaY: number; shiftKey: boolean; deltaMode?: number }} e */
export function wheel(e) {
  const px = e.deltaMode === 1 ? 100 / 3 : 1,
    [dx, dy] = [e.deltaX * px, e.deltaY * px];
  const sideways = Math.abs(dx) > Math.abs(dy);
  if (sideways || e.shiftKey) return { factor: 0, pan: (sideways ? dx : dy) / 600 };
  return { factor: NOTCH ** (-dy / 100), pan: 0 };
}

// A pinch: two fingers at `began` (each a share of the track's width) over `view` when it began,
// now at `now`. Returns what it shows then: the time between the fingers stays between them.
/** @param {Span} view @param {Span} whole @param {[number, number]} began @param {[number, number]} now */
export function pinched(view, whole, began, now) {
  const span = view[1] - view[0],
    apart = Math.max(0.02, Math.abs(now[0] - now[1])),
    wide = (span * Math.max(0.02, Math.abs(began[0] - began[1]))) / apart,
    at = view[0] + (span * (began[0] + began[1])) / 2,
    from = at - (wide * (now[0] + now[1])) / 2;
  return within([from, from + wide], whole);
}
