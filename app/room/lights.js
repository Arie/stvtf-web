// A room's spotlight: one click lights a spot in the drawer's ink, kept until cleared; the next
// click moves it (src/rooms/room/light.rs). In TF2 it is a light on the floor there.
import { ctx, uiScale } from "../core/core.js";
import { toScreen } from "../map/view.js";
/** @typedef {import("../../types/stream").LightLine} LightLine */

// How far its glow reaches on the map, in map units (about what TF2's light brightens), and the
// least it does on the page, in pixels at the 1080p size: seen with the whole map in view too.
const RADIUS = 300,
  MIN_PX = 40;

/** @type {{ at: [number, number, number]; ink: string } | null} */
let light = null;

/** @param {LightLine} m */
export const onLight = m => void (light = { at: m.at, ink: m.ink });

// The marker's spotlight, in place of the page's.
/** @param {{ ink?: string; light?: [number, number, number] }[]} strokes */
export function lightFromMarker(strokes) {
  const k = strokes.find(s => s.light && s.ink);
  light = k?.light && k.ink ? { at: k.light, ink: k.ink } : null;
}

export const clearLight = () => void (light = null);
export const lightAt = () => light?.at ?? null;

// A glow that fades out from its spot.
export function drawLight() {
  if (!light) return;
  const [x, y] = toScreen(light.at[0], light.at[1]),
    [ex, ey] = toScreen(light.at[0] + RADIUS, light.at[1]),
    r = Math.max(Math.hypot(ex - x, ey - y), MIN_PX * uiScale());
  const glow = ctx.createRadialGradient(x, y, 0, x, y, r);
  glow.addColorStop(0, `${light.ink}e6`);
  glow.addColorStop(0.4, `${light.ink}73`);
  glow.addColorStop(1, `${light.ink}00`);
  ctx.save();
  ctx.fillStyle = glow;
  ctx.beginPath();
  ctx.arc(x, y, r, 0, 2 * Math.PI);
  ctx.fill();
  ctx.restore();
}
