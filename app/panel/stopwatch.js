// The stopwatch (tournament attack/defend and payload), as TF2's stopwatch panel shows it to a
// spectator: the time and points the first half's attackers set, then the time left to beat it.
import { msPerTick, $, clock } from "../core/core.js";
/** @typedef {import("../../types/stream").Snapshot} Snapshot */
/** @typedef {import("../../types/stream").Stopwatch} Stopwatch */

/** @type {{ [team: string]: string }} */
const NAME = { red: "RED", blue: "BLU" };

/** @param {number} n */
const points = n => `${n} POINT${n === 1 ? "" : "S"}`;

// What the panel says, `since` seconds after its snapshot: a label, the time, the points.
/** @param {Stopwatch} sw @param {number} since */
export function stopwatchParts(sw, since) {
  if (sw.state === "unset") return { label: "CAPTURE TIME NOT YET SET", time: "", points: "" };
  if (sw.state === "overtime") return { label: `ATTACKERS NEED ${points(sw.points)} TO WIN`, time: "", points: "" };
  const defenders = sw.attackers === "red" ? "blue" : "red";
  const time = clock(sw.paused ? sw.time : sw.time - since);
  return { label: sw.setting ? "" : `${NAME[defenders]}'S TIME`, time, points: points(sw.points) };
}

/** @param {Snapshot} snap @param {number} tick */
export function renderStopwatch(snap, tick) {
  const el = $("stopwatch");
  const sw = snap.stopwatch;
  el.hidden = !sw;
  if (!sw) return;
  const p = stopwatchParts(sw, (Math.max(0, tick - snap.tick) * msPerTick) / 1000);
  const html = [p.label && `<span>${p.label}</span>`, p.time && `<b>${p.time}</b>`, p.points && `<i>${p.points}</i>`]
    .filter(Boolean)
    .join("");
  if (el.innerHTML !== html) el.innerHTML = html;
}
