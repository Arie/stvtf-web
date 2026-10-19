import { ctx, state } from "./core.js";
import { MARK_LIFE_MS, SCORCH_CAP, SCORCH_R } from "./events.js";
import { toScreen, px } from "./view.js";

// Flat scorch craters: pre-rendered ragged sprites, four shapes each turned four ways, so a
// crater is drawn scaled but never rotated (a rotated image is the slow path).
const scorchSprites = (() => {
  const N = 4, S = 128, c = S / 2, out = [];
  for (let k = 0; k < N * 4; k++) {
    const v = k % N, turn = Math.floor(k / N) * Math.PI / 2 + v;
    const cv = document.createElement("canvas"); cv.width = cv.height = S;
    const g = cv.getContext("2d"), K = 24, rad = [];
    for (let i = 0; i < K; i++) rad.push(0.72 + 0.28 * Math.abs(Math.sin(i * 2.399 + v * 1.7) * Math.cos(i * 0.9 + v * 3.1)) + (i % 3 === v % 3 ? 0.04 : 0));
    g.beginPath();
    rad.forEach((r, i) => { const a = i / K * Math.PI * 2 + turn; g[i ? "lineTo" : "moveTo"](c + Math.cos(a) * r * (c - 2), c + Math.sin(a) * r * (c - 2)); });
    g.closePath();
    const gr = g.createRadialGradient(c, c, 0, c, c, c);
    gr.addColorStop(0, "rgba(10,8,6,0.95)"); gr.addColorStop(0.45, "rgba(14,11,9,0.7)"); gr.addColorStop(0.8, "rgba(20,16,13,0.3)"); gr.addColorStop(1, "rgba(20,16,13,0.08)");
    g.fillStyle = gr; g.fill();
    out.push(cv);
  }
  return out;
})();
export function addScorch(pos, t0) {
  const h = Math.abs(Math.imul(Math.round(pos[0]) * 73856093 ^ Math.round(pos[1]) * 19349663, 2654435761)) >>> 0;
  state.scorches.push({ pos, t0, v: h % scorchSprites.length });
  if (state.scorches.length > SCORCH_CAP) state.scorches.splice(0, state.scorches.length - SCORCH_CAP);
  state.marks = state.marks.filter(m => !(Math.abs(m.t0 - t0) < 500 && Math.hypot(m.pos[0] - pos[0], m.pos[1] - pos[1]) < 40));
}
// The explosion's own blast decal would put a dot on its crater.
export function nearFreshScorch(pos, now) {
  for (let i = state.scorches.length - 1; i >= 0; i--) {
    const s = state.scorches[i];
    if (now - s.t0 > 500) break;
    if (Math.hypot(s.pos[0] - pos[0], s.pos[1] - pos[1]) < 40) return true;
  }
  return false;
}
function drawScorches(now) {
  while (state.scorches.length && now - state.scorches[0].t0 >= MARK_LIFE_MS) state.scorches.shift();
  const d = SCORCH_R * 2 * state.view.scale;
  if (d < 2) return;
  ctx.save();
  for (const s of state.scorches) {
    const [x, y] = toScreen(s.pos[0], s.pos[1]);
    ctx.globalAlpha = 0.9 * (1 - (now - s.t0) / MARK_LIFE_MS);
    ctx.drawImage(scorchSprites[s.v], x - d / 2, y - d / 2, d, d);
  }
  ctx.restore();
}

export function drawMarks() {
  const now = performance.now();
  drawScorches(now);
  while (state.marks.length && now - state.marks[0].t0 >= MARK_LIFE_MS) state.marks.shift();
  const r = Math.max(1.5, px(7, 1.5));
  ctx.save();
  for (const m of state.marks) {
    const [x, y] = toScreen(m.pos[0], m.pos[1]);
    ctx.globalAlpha = 0.75 * (1 - (now - m.t0) / MARK_LIFE_MS);
    ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fillStyle = "#14110f"; ctx.fill();
  }
  ctx.restore();
}

// Every pack, solid when available and a faint outline while taken. Each look is drawn once per
// size into a sprite: a pack is a dozen paths, and the map has dozens of packs.
const packSprites = new Map();
function packSprite(k, s, taken) {
  const dpr = window.devicePixelRatio || 1, q = Math.round(s * 2) / 2, key = `${k.kind} ${k.size} ${taken} ${q} ${dpr}`;
  let sp = packSprites.get(key);
  if (!sp) {
    if (packSprites.size > 64) packSprites.clear();
    const half = Math.ceil(q * 1.3 + 3), c = document.createElement("canvas");
    c.width = c.height = Math.ceil(half * 2 * dpr);
    const g = c.getContext("2d");
    g.scale(dpr, dpr); g.lineJoin = "round"; g.lineWidth = 1.5;
    drawPack(g, k, half, half, q, taken);
    packSprites.set(key, sp = { c, half });
  }
  return sp;
}
export function drawPacks() {
  const packs = state.geometry?.packs, now = performance.now();
  if (!packs) return;
  for (const i of state.packsTaken.keys()) if (!packs[i]) state.packsTaken.delete(i);
  ctx.save();
  packs.forEach((k, i) => {
    const until = state.packsTaken.get(i);
    const taken = until !== undefined && now < until;
    if (until !== undefined && !taken) state.packsTaken.delete(i);
    const [x, y] = toScreen(k.pos[0], k.pos[1]);
    const { c, half } = packSprite(k, px(k.size === "small" ? 22 : k.size === "medium" ? 30 : 40, 5), taken);
    ctx.globalAlpha = taken ? 0.3 : 1;
    ctx.drawImage(c, x - half, y - half, half * 2, half * 2);
  });
  ctx.restore();
}
function drawPack(g, k, x, y, s, taken) {
  const health = k.kind === "health", ink = taken ? "#f2f0e6" : "#0b0c0e";
  g.strokeStyle = ink;
  const shape = (bx, by, bw, bh, r, fill) => {
    g.beginPath(); g.roundRect(bx, by, bw, bh, r);
    if (!taken) { g.fillStyle = fill; g.fill(); }
    g.stroke();
  };
  const disc = (cx, cy, r, fill, lw = 1.5) => {
    g.beginPath(); g.arc(cx, cy, r, 0, Math.PI * 2);
    if (!taken) { g.fillStyle = fill; g.fill(); }
    g.lineWidth = lw; g.stroke(); g.lineWidth = 1.5;
  };
  const cross = (cx, cy, a, t, fill) => {
    g.beginPath();
    g.moveTo(cx - t, cy - a); g.lineTo(cx + t, cy - a); g.lineTo(cx + t, cy - t); g.lineTo(cx + a, cy - t);
    g.lineTo(cx + a, cy + t); g.lineTo(cx + t, cy + t); g.lineTo(cx + t, cy + a); g.lineTo(cx - t, cy + a);
    g.lineTo(cx - t, cy + t); g.lineTo(cx - a, cy + t); g.lineTo(cx - a, cy - t); g.lineTo(cx - t, cy - t);
    g.closePath();
    if (!taken) { g.fillStyle = fill; g.fill(); }
    g.lineWidth = 1; g.stroke(); g.lineWidth = 1.5;
  };
  if (!health) {
    // gray metal frame; medium and full add a tan panel with a dark bullet emblem
    const h = s * 0.8, ww = s * (k.size === "full" ? 1.05 : 0.9), small = k.size === "small";
    shape(x - ww, y - h, ww * 2, h * 2, s * 0.2, "#aeb3ba");
    if (small) {
      shape(x - ww * 0.7, y - h * 0.6, ww * 1.4, h * 1.2, s * 0.1, "#5c5f66");
      disc(x, y, s * 0.17, "#2e2a24", 1);
    } else {
      const iw = ww * 0.76, ih = h * 0.72;
      shape(x - iw, y - ih, iw * 2, ih * 2, s * 0.1, "#c4a67c");
      const r = Math.min(iw, ih) * 0.62;
      g.beginPath(); g.arc(x, y, r, 0, Math.PI * 2);
      g.lineWidth = Math.max(1.5, r * 0.28); g.strokeStyle = taken ? ink : "#3b3226"; g.stroke();
      g.lineWidth = 1.5; g.strokeStyle = ink;
      if (r > 6) for (const dx of [-0.4, 0, 0.4]) {
        g.beginPath(); g.moveTo(x + dx * r, y - r * 0.4); g.lineTo(x + dx * r, y + r * 0.4);
        g.lineWidth = Math.max(1.2, r * 0.2); g.strokeStyle = taken ? ink : "#3b3226"; g.stroke();
        g.lineWidth = 1.5; g.strokeStyle = ink;
      }
    }
  } else if (k.size === "small") {
    // pill bottle: white cap, teal body with a white label band and a red cross
    const bw = s * 0.62, ly = y + s * 0.275, lh = s * 0.28;
    shape(x - bw, y - s * 0.3, bw * 2, s * 1.15, s * 0.2, "#1f8a8f");
    shape(x - bw, ly - lh, bw * 2, lh * 2, s * 0.06, "#ecebe4");
    cross(x, ly, s * 0.22, s * 0.08, "#c0262e");
    shape(x - bw * 0.85, y - s * 0.62, bw * 1.7, s * 0.36, s * 0.1, "#ecebe4");
  } else if (k.size === "medium") {
    const h = s * 0.85;
    shape(x - h, y - h, h * 2, h * 2, s * 0.28, "#1f8a8f");
    disc(x, y, h * 0.74, "#c0262e", 1);
    cross(x, y, h * 0.52, h * 0.2, "#ecebe4");
  } else {
    // big case: teal box, light lid band, gray handle on top, red badge on the front
    const ww = s * 1.15, h = s * 0.75, ly = y - h * 0.3;
    g.beginPath();
    g.moveTo(x - ww * 0.3, y - h); g.lineTo(x - ww * 0.3, y - h - s * 0.22); g.lineTo(x + ww * 0.3, y - h - s * 0.22); g.lineTo(x + ww * 0.3, y - h);
    if (!taken) { g.fillStyle = "#8f9089"; g.fill(); }
    g.stroke();
    shape(x - ww, y - h, ww * 2, h * 2, s * 0.2, "#1f8a8f");
    if (!taken) { g.beginPath(); g.roundRect(x - ww, y - h, ww * 2, ly - (y - h), [s * 0.2, s * 0.2, 0, 0]); g.fillStyle = "#b7b7af"; g.fill(); g.beginPath(); g.roundRect(x - ww, y - h, ww * 2, h * 2, s * 0.2); g.stroke(); }
    g.beginPath(); g.moveTo(x - ww, ly); g.lineTo(x + ww, ly); g.stroke();
    disc(x, y + h * 0.42, h * 0.4, "#c0262e", 1);
    cross(x, y + h * 0.42, h * 0.27, h * 0.1, "#ecebe4");
  }
}

const TELEPORT = 250;

// Blast-jump trails from the interpolated positions: dark team shade off the ground to a bright one at the peak.
export function drawJumpTrails(players) {
  const now = performance.now(), byId = new Map(players.map(p => [p.id, p]));
  state.trails = state.trails.filter(t => !t.end || now - t.end < 2000);
  ctx.save(); ctx.lineCap = "round"; ctx.lineJoin = "round";
  for (const t of state.trails) {
    if (!t.end) {
      const p = byId.get(t.id);
      const last = t.pts.at(-1), step = last && p?.pos ? Math.hypot(p.pos[0] - last[0], p.pos[1] - last[1]) : 0;
      // A step no jump makes in one frame is a teleport (a respawn at round reset): the trail ends there.
      if (!p || !p.alive || now - t.t0 > 6000 || step > TELEPORT) t.end = now;
      else {
        if (!last || step > 8) t.pts.push([p.pos[0], p.pos[1], p.pos[2]]);
        else last[2] = p.pos[2];
        if (t.pts.length > 400) t.pts.shift();
      }
    }
    if (t.pts.length < 2) continue;
    const peak = Math.max(1, ...t.pts.map(q => q[2] - t.z0));
    ctx.globalAlpha = t.end ? 1 - (now - t.end) / 2000 : 1;
    ctx.lineWidth = Math.max(2, px(14, 2));
    const hue = t.team === "red" ? 4 : 212, segs = [];
    for (let i = 1; i < t.pts.length; i++) {
      const a = t.pts[i - 1], b = t.pts[i], h = Math.max(0, Math.min(1, (b[2] - t.z0) / peak));
      const [ax, ay] = toScreen(a[0], a[1]), [bx, by] = toScreen(b[0], b[1]);
      segs.push([ax, ay, bx, by, `hsl(${hue} 85% ${Math.round(24 + 50 * h)}%)`]);
    }
    // One stroke for the outline and one per shade, not two per segment.
    const w = ctx.lineWidth, byShade = new Map();
    ctx.beginPath();
    for (const [ax, ay, bx, by, c] of segs) {
      ctx.moveTo(ax, ay); ctx.lineTo(bx, by);
      if (!byShade.has(c)) byShade.set(c, []);
      byShade.get(c).push([ax, ay, bx, by]);
    }
    ctx.lineWidth = w + 2; ctx.strokeStyle = "rgba(0,0,0,0.55)"; ctx.stroke();
    ctx.lineWidth = w;
    for (const [c, list] of byShade) {
      ctx.beginPath();
      for (const [ax, ay, bx, by] of list) { ctx.moveTo(ax, ay); ctx.lineTo(bx, by); }
      ctx.strokeStyle = c; ctx.stroke();
    }
  }
  ctx.restore();
}
