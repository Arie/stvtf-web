import { drawCart } from "./payload.js";
import { drawPoint } from "./points.js";
import { drawGoal, drawRobot, drawCore, drawRevive, drawMoney, drawTank } from "./modes.js";
/** @typedef {import("../../../types/stream").Snapshot} Snapshot */

// Each objective in a snapshot to the module that draws it; flags and the ball go on top of the
// players (flags.js).
/** @param {Snapshot} snap @param {number} tick */
export function drawObjectives(snap, tick) {
  for (const o of snap.objectives || []) {
    if (o.kind === "cart") {
      drawCart(o, tick);
      continue;
    }
    if (o.kind === "goal") {
      drawGoal(o);
      continue;
    }
    if (o.kind === "robot") {
      drawRobot(o, tick);
      continue;
    }
    if (o.kind === "core") {
      drawCore(o);
      continue;
    }
    if (o.kind === "tank") {
      drawTank(o, tick);
      continue;
    }
    if (o.kind === "revive") {
      drawRevive(o);
      continue;
    }
    if (o.kind === "money") {
      drawMoney(o);
      continue;
    }
    if (o.kind === "control_point") drawPoint(o, snap, tick);
  }
}
