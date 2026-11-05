import { msPerTick, TEAM, OVERLAY, overlayUnit, uiScale, ctx, state, INK, NEUTRAL, objPos } from "./core.js";
import { toScreen, px, playerRad, label } from "./view.js";
import { drawBomb } from "./payload.js";
/** @typedef {import("../types/stream").PlacedObjective} PlacedObjective */
/** @typedef {import("../types/stream").Player} Player */
/** @typedef {import("../types/stream").LivePlayer} LivePlayer */
/** @typedef {import("../types/stream").Snapshot} Snapshot */

// What players carry: flags (and Mann vs. Machine's bomb), the pass time ball and player
// destruction's pickups.
// Who carries an objective, while they are alive.
/** @param {Player[]} players @param {number | null | undefined} id @returns {LivePlayer | null} */
function liveCarrier(players, id) {
  const p = id == null ? null : players.find(q => q.id === id);
  return p?.alive ? p : null;
}
// Where a carried objective sits on its carrier: up and right of their dot, by this much of it.
const ON_CARRIER = 0.9;
// Where an objective shows on screen: on its carrier, or at `pos` when nobody carries it.
/** @param {LivePlayer | null} carrier @param {number[]} pos @returns {[number, number]} */
function shownAt(carrier, pos) {
  const at = carrier ? carrier.pos : pos;
  const [x, y] = toScreen(at[0], at[1]);
  if (!carrier) return [x, y];
  const r = playerRad();
  return [x + r * ON_CARRIER, y - r * ON_CARRIER];
}

// The pass time ball: a ball in the colour of the team that has it (yellow while neutral),
// riding on its carrier.
/** @param {PlacedObjective} o @param {Player[]} players @param {number} tick */
function drawBall(o, players, tick) {
  const carrier = liveCarrier(players, o.carrier);
  const [x, y] = shownAt(carrier, objPos(o, tick));
  const r = OVERLAY ? overlayUnit() * 0.55 : px(18, 7);
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = o.team === "red" || o.team === "blue" ? TEAM[o.team] : NEUTRAL;
  ctx.fill();
  ctx.lineWidth = Math.max(1.5, r * 0.22);
  ctx.strokeStyle = "#000";
  ctx.stroke();
  ctx.fillStyle = "#ffffff88";
  ctx.beginPath();
  ctx.arc(x - r * 0.3, y - r * 0.3, r * 0.3, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}

// A player destruction stack of pickups: a disc with how many points it holds, on its carrier
// or where it was dropped (with the seconds until it goes).
/** @param {PlacedObjective} o @param {LivePlayer | null} carrier @param {Snapshot} snap @param {number} tick */
function drawPickups(o, carrier, snap, tick) {
  const [x, y] = shownAt(carrier, o.pos);
  const r = OVERLAY ? overlayUnit() * 0.6 : px(22, 8);
  ctx.save();
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = "#7bd88f";
  ctx.fill();
  ctx.lineWidth = Math.max(1.5, r * 0.18);
  ctx.strokeStyle = "#000";
  ctx.stroke();
  label(String(o.points), x, y, Math.max(9, r * 1.2), { colour: INK });
  if (o.status === "dropped" && o.returns_in != null) {
    const left = Math.max(0, Math.ceil(o.returns_in - (Math.max(0, tick - snap.tick) * msPerTick) / 1000));
    ctx.font = `700 ${Math.round(Math.max(10 * uiScale(), r * 0.9))}px "HUD Condensed", system-ui`;
    ctx.textBaseline = "top";
    ctx.lineJoin = "round";
    ctx.lineWidth = 3;
    ctx.strokeStyle = "#000";
    ctx.strokeText(String(left), x, y + r + 2);
    ctx.fillStyle = "#fff";
    ctx.fillText(String(left), x, y + r + 2);
  }
  ctx.restore();
}

// The intelligence as a briefcase in its team's colour (a neutral flag, such as the bball ball,
// in yellow; Mann vs. Machine's bomb as a bomb): at home, dropped with the seconds until it goes
// home, or riding on its carrier.
/** @param {Snapshot} snap @param {Player[]} players @param {number} tick */
export function drawFlags(snap, players, tick) {
  for (const o of snap.objectives || []) {
    if (o.kind === "ball") {
      drawBall(o, players, tick);
      continue;
    }
    if (o.kind !== "flag") continue;
    const carrier = o.status === "carried" ? liveCarrier(players, o.carrier) : null;
    if (o.points != null && o.team !== "red" && o.team !== "blue") {
      drawPickups(o, carrier, snap, tick);
      continue;
    }
    const [x, y] = shownAt(carrier, o.pos);
    const s = OVERLAY ? overlayUnit() * 1.3 : px(48, 17);
    ctx.save();
    if (state.map?.startsWith("mvm_")) drawBomb(x, y, s * 0.45, Math.max(1.5, s * 0.1));
    else {
      ctx.lineWidth = Math.max(1.5, s * 0.12);
      ctx.strokeStyle = "#000";
      ctx.beginPath();
      ctx.roundRect(x - s * 0.25, y - s * 0.62, s * 0.5, s * 0.36, s * 0.1);
      ctx.stroke();
      ctx.beginPath();
      ctx.roundRect(x - s * 0.6, y - s * 0.35, s * 1.2, s * 0.8, s * 0.14);
      ctx.fillStyle = o.team === "red" || o.team === "blue" ? TEAM[o.team] : NEUTRAL;
      ctx.fill();
      ctx.stroke();
    }
    if (o.status === "dropped" && o.returns_in != null) {
      const left = Math.max(0, Math.ceil(o.returns_in - (Math.max(0, tick - snap.tick) * msPerTick) / 1000));
      ctx.font = `700 ${Math.round(Math.max(11 * uiScale(), s * 0.7))}px "HUD Condensed", system-ui`;
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
      ctx.lineJoin = "round";
      ctx.lineWidth = 3;
      ctx.strokeText(String(left), x, y + s * 0.55);
      ctx.fillStyle = "#fff";
      ctx.fillText(String(left), x, y + s * 0.55);
    }
    ctx.restore();
  }
}
