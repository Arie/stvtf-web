import { msPerTick, TEAM, OVERLAY, overlayUnit, uiScale, ctx, state, optOn, teamColour, INK } from "../../core/core.js";
import { toScreen, px } from "../view.js";
import { layerOn } from "../../core/layers.js";
/** @typedef {import("../../../types/stream").Objective} Objective */
/** @typedef {import("../../../types/stream").PlacedObjective} PlacedObjective */
/** @typedef {import("../../../types/stream").Snapshot} Snapshot */
/** @typedef {import("../../../types/stream").Vec3} Vec3 */
/** @typedef {NonNullable<import("../../../types/page").Geometry["cap_areas"]>[number]} CapArea */

// Control points and their capture areas.
// numbers=0 leaves the number (and the cap rate) off the control points.
const SHOW_NUMBERS = optOn("numbers");
// A capture area belongs to the point standing in it, or one at most this many units outside.
const CAP_AREA_REACH = 256;

// Points are squares like TF2's HUD, so they never read as players: the owner's colour, filled
// left to right in the capping team's colour while a capture is under way.
/** @param {Objective} o @param {Snapshot} snap @param {number} tick */
export function drawPoint(o, snap, tick) {
  if (!o.pos) return;
  const [x, y] = toScreen(o.pos[0], o.pos[1]);
  const u = OVERLAY ? overlayUnit() : 0;
  const half = OVERLAY ? u * 1.2 : px(110, 11);
  const x0 = x - half,
    y0 = y - half,
    side = half * 2,
    corner = Math.max(2, half * 0.18);
  // Advance between snapshots at the rate stvtf extrapolates from the cap times and cappers.
  const progress = Math.max(
    0,
    Math.min(1, o.cap_progress + ((o.cap_rate || 0) * Math.max(0, tick - snap.tick) * msPerTick) / 1000),
  );
  const capping = o.capping_team !== "none" && progress > 0;
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(x0, y0, side, side, corner);
  ctx.fillStyle = teamColour(o.owner) + (OVERLAY ? "e6" : "99");
  ctx.fill();
  if (capping) {
    ctx.save();
    ctx.clip();
    ctx.fillStyle = TEAM[o.capping_team];
    ctx.fillRect(x0, y0, side * progress, side);
    ctx.restore();
  }
  if (o.locked) {
    ctx.fillStyle = INK + "99";
    ctx.fill();
  }
  // Outlined in the colour of the team holding it, grey while nobody does.
  ctx.lineWidth = OVERLAY ? Math.max(2, u * 0.18) : 2;
  ctx.strokeStyle = teamColour(o.owner, "#b8bcc2");
  ctx.stroke();
  if (SHOW_NUMBERS) drawPointNumber(o, capping, x, y, half);
  ctx.restore();
}

// The point's number, or while it is capped the cap rate (×2 for two cappers, as TF2's HUD shows it).
/** @param {Objective} o @param {boolean} capping @param {number} x @param {number} y @param {number} half */
function drawPointNumber(o, capping, x, y, half) {
  const u = OVERLAY ? overlayUnit() : 0;
  ctx.fillStyle = "#fff";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = `800 ${Math.round(OVERLAY ? u * 1.3 : Math.max(12 * uiScale(), half * 0.9))}px system-ui`;
  const n = layerOn("points") && capping && o.on_point ? o.on_point[o.capping_team] : 0;
  const text = n > 0 ? `\u00d7${n}` : String(/** @type {number} */ (o.index) + 1);
  ctx.lineWidth = 3;
  ctx.strokeStyle = INK;
  ctx.strokeText(text, x, y + 1);
  ctx.fillText(text, x, y + 1);
}

// Capture areas: just an outline in the owner's colour; progress is shown on the point square.
// An area belongs to the point standing in it: point numbers in the map file don't always match
// the game's (multi-round maps number per round).
/** @param {Snapshot} snap */
export function drawCapAreas(snap) {
  const areas = state.geometry?.cap_areas;
  if (!areas?.length) return;
  const cps = (snap.objectives || []).filter(
    /** @param {Objective} o @returns {o is PlacedObjective} */ o => o.kind === "control_point" && o.pos != null,
  );
  /** @param {CapArea} a @param {Vec3} p */
  const near = (a, p) =>
    Math.hypot(Math.max(a.min[0] - p[0], 0, p[0] - a.max[0]), Math.max(a.min[1] - p[1], 0, p[1] - a.max[1]));
  for (const a of areas) {
    const cp = cps.reduce(
      (b, o) => (!b || near(a, o.pos) < near(a, b.pos) ? o : b),
      /** @type {(typeof cps)[number] | null} */ (null),
    );
    if (!cp || near(a, cp.pos) > CAP_AREA_REACH) continue;
    ctx.save();
    ctx.beginPath();
    [
      [a.min[0], a.min[1]],
      [a.max[0], a.min[1]],
      [a.max[0], a.max[1]],
      [a.min[0], a.max[1]],
    ].forEach(([x, y], i) => {
      const [sx, sy] = toScreen(x, y);
      (i ? ctx.lineTo : ctx.moveTo).call(ctx, sx, sy);
    });
    ctx.closePath();
    ctx.strokeStyle = teamColour(cp.owner) + "99";
    ctx.lineWidth = OVERLAY ? 1.5 : 2;
    ctx.stroke();
    ctx.restore();
  }
}
