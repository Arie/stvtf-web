import { TEAM, CLASS, icon, OVERLAY, OPT, overlayUnit, uiScale, ctx, state } from "./core.js";
import { layerOn } from "./layers.js";
import { toScreen, screenAngle, px, playerRad } from "./view.js";

export function drawProjectiles(list) {
  // The overlay sizes projectiles from its panel like the players, a little larger than their
  // usual minimum, so they read at minimap scale.
  const size = (units, min) => (OVERLAY ? (min * overlayUnit()) / 9 : px(units, min));
  for (const p of list || []) {
    const [x, y] = toScreen(p.pos[0], p.pos[1]);
    const col = TEAM[p.team] || "#ccc";
    // Rockets and arrows face their flight (the networked yaw is exact); grenades tumble.
    const deg = p.kind === "Rocket" || p.kind.endsWith("Arrow") ? p.yaw : p.heading ?? p.yaw ?? 0;
    const a = screenAngle(deg);
    ctx.save();
    ctx.translate(x, y);
    ctx.lineJoin = "round";
    ctx.strokeStyle = "#000"; ctx.lineWidth = 1.2;
    if (p.crit) { ctx.shadowColor = col; ctx.shadowBlur = 10; }
    switch (p.kind) {
      case "Rocket": {
        ctx.rotate(a);
        const l = size(26, 12), w = l * 0.32;
        const trail = ctx.createLinearGradient(-l * 3.2, 0, -l * 0.5, 0);
        trail.addColorStop(0, "#ffffff00"); trail.addColorStop(1, "#ffd9a0aa");
        ctx.fillStyle = trail; ctx.fillRect(-l * 3.2, -w * 0.35, l * 2.7, w * 0.7);
        ctx.beginPath(); ctx.moveTo(l * 0.6, 0); ctx.lineTo(-l * 0.5, -w); ctx.lineTo(-l * 0.5, w); ctx.closePath();
        ctx.fillStyle = p.crit ? "#fff" : col; ctx.fill(); ctx.stroke();
        break;
      }
      case "Pipe": case "LooseCannon": {
        const r = size(9, 4.5);
        ctx.beginPath(); ctx.arc(0, 0, r, 0, Math.PI * 2);
        ctx.fillStyle = p.crit ? "#fff" : col; ctx.fill(); ctx.stroke();
        ctx.beginPath(); ctx.arc(0, 0, r * 0.45, 0, Math.PI * 2); ctx.fillStyle = "#0008"; ctx.fill();
        break;
      }
      case "Sticky": {
        const r = size(10, 6);
        ctx.beginPath();
        for (let i = 0; i < 16; i++) {
          const rr = i % 2 ? r * 0.55 : r * 1.15, t = i / 16 * Math.PI * 2;
          (i ? ctx.lineTo : ctx.moveTo).call(ctx, Math.cos(t) * rr, Math.sin(t) * rr);
        }
        ctx.closePath(); ctx.fillStyle = p.crit ? "#fff" : col; ctx.fill(); ctx.stroke();
        ctx.beginPath(); ctx.arc(0, 0, r * 0.25, 0, Math.PI * 2); ctx.fillStyle = "#fff"; ctx.fill();
        break;
      }
      case "Arrow": case "HealingArrow": {
        ctx.rotate(a);
        const l = size(34, 18);
        ctx.beginPath(); ctx.moveTo(-l * 0.6, 0); ctx.lineTo(l * 0.4, 0);
        ctx.strokeStyle = "#000"; ctx.lineWidth = 3.5; ctx.stroke();
        ctx.strokeStyle = col; ctx.lineWidth = 2; ctx.stroke();
        ctx.beginPath(); ctx.moveTo(l * 0.6, 0); ctx.lineTo(l * 0.3, -l * 0.16); ctx.lineTo(l * 0.3, l * 0.16); ctx.closePath();
        ctx.fillStyle = "#fff"; ctx.fill(); ctx.strokeStyle = "#000"; ctx.lineWidth = 1; ctx.stroke();
        if (p.kind === "HealingArrow") {
          ctx.rotate(-a); ctx.fillStyle = "#7dff8a";
          ctx.fillRect(-l * 0.32, -1.5, l * 0.3, 3); ctx.fillRect(-l * 0.32 + l * 0.15 - 1.5, -l * 0.15, 3, l * 0.3);
        }
        break;
      }
      case "Flare": case "Fireball": {
        ctx.beginPath(); ctx.arc(0, 0, size(9, 3.5), 0, Math.PI * 2);
        ctx.fillStyle = "#ffb347"; ctx.shadowColor = "#ff7a00"; ctx.shadowBlur = 12; ctx.fill();
        break;
      }
      case "EnergyBall": case "EnergyRing": case "ShortCircuitOrb": {
        ctx.beginPath(); ctx.arc(0, 0, size(12, 4.5), 0, Math.PI * 2);
        ctx.strokeStyle = col; ctx.lineWidth = 3; ctx.shadowColor = col; ctx.shadowBlur = 12; ctx.stroke();
        break;
      }
      case "Jarate": case "MadMilk": case "GasPasser": case "Cleaver": {
        const r = size(9, 4);
        ctx.rotate(Math.PI / 4); ctx.fillStyle = p.kind === "Jarate" ? "#ffe14a" : p.kind === "MadMilk" ? "#f4f4f4" : p.kind === "GasPasser" ? "#ff9a3c" : "#bbb";
        ctx.fillRect(-r, -r, r * 2, r * 2); ctx.strokeRect(-r, -r, r * 2, r * 2);
        break;
      }
      default: {
        ctx.beginPath(); ctx.arc(0, 0, size(8, 3), 0, Math.PI * 2);
        ctx.fillStyle = col; ctx.fill(); ctx.stroke();
      }
    }
    ctx.restore();
  }
}

// Each charge looks different: über a candy-cane team ring with a dark edge and a soft pulse, kritz crackling, quick-fix
// pulsing rings, vaccinator a shield with its resistance.
function drawUberRing(p, x, y, rad, col) {
  const t = performance.now() / 1000;
  ctx.save();
  if (p.ubered === "uber") {
    const base = TEAM[p.team], pulse = 0.5 + 0.5 * Math.sin(t * 5);
    const tint = "#" + [1, 3, 5].map((i) => Math.round(parseInt(base.substr(i, 2), 16) * 0.5 + 128).toString(16).padStart(2, "0")).join("");
    const lw = OVERLAY ? Math.max(3, rad * 0.3) : 5, r0 = rad + 3 + lw / 2, ri = r0 - lw / 2, ro = r0 + lw / 2;
    ctx.beginPath(); ctx.arc(x, y, r0, 0, Math.PI * 2);
    ctx.shadowColor = base; ctx.shadowBlur = 10 + 8 * pulse;
    ctx.strokeStyle = "#0b0c0e"; ctx.lineWidth = lw + 3; ctx.stroke();
    ctx.shadowBlur = 0; ctx.strokeStyle = base; ctx.lineWidth = lw; ctx.stroke();
    // Candy cane like the HUD card: tint stripes slanted by their outer edge, turning one stripe pair a second.
    const n = 12, step = Math.PI * 2 / n, slant = step * 0.8, rot = t * step;
    ctx.fillStyle = tint; ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const a = rot + i * step, w = step / 2;
      ctx.moveTo(x + Math.cos(a) * ri, y + Math.sin(a) * ri);
      ctx.lineTo(x + Math.cos(a + w) * ri, y + Math.sin(a + w) * ri);
      ctx.lineTo(x + Math.cos(a + w + slant) * ro, y + Math.sin(a + w + slant) * ro);
      ctx.lineTo(x + Math.cos(a + slant) * ro, y + Math.sin(a + slant) * ro);
      ctx.closePath();
    }
    ctx.fill();
    ctx.beginPath(); ctx.arc(x, y, ri, 0, Math.PI * 2); ctx.strokeStyle = "rgba(11,12,14,0.7)"; ctx.lineWidth = 1; ctx.stroke();
  } else if (p.ubered === "kritz") {
    ctx.beginPath();
    for (let i = 0; i <= 24; i++) {
      const a = i / 24 * Math.PI * 2 + t * 3, r = rad + 6 + (i % 2 ? 4 : -1) + Math.sin(t * 20 + i) * 1.5;
      (i ? ctx.lineTo : ctx.moveTo).call(ctx, x + Math.cos(a) * r, y + Math.sin(a) * r);
    }
    ctx.strokeStyle = "#fff"; ctx.shadowColor = col; ctx.shadowBlur = 10; ctx.lineWidth = 2; ctx.stroke();
  } else if (p.ubered === "quickfix") {
    for (let i = 0; i < 2; i++) {
      const k = (t * 1.2 + i / 2) % 1;
      ctx.beginPath(); ctx.arc(x, y, rad + 4 + k * 16, 0, Math.PI * 2);
      ctx.strokeStyle = col; ctx.globalAlpha = 1 - k; ctx.lineWidth = 3; ctx.stroke();
    }
  } else {
    ctx.beginPath(); ctx.arc(x, y, rad + 7, -Math.PI * 0.8, -Math.PI * 0.2);
    ctx.strokeStyle = "#fff"; ctx.lineWidth = 4; ctx.shadowColor = col; ctx.shadowBlur = 12; ctx.stroke();
    ctx.shadowBlur = 0; ctx.font = `bold ${Math.round(10 * uiScale())}px system-ui`; ctx.textAlign = "center"; ctx.textBaseline = "bottom";
    ctx.fillStyle = "#fff"; ctx.fillText(p.ubered.split("_")[1].toUpperCase(), x, y - rad - 9);
  }
  ctx.restore();
}

export function drawHealBeams(players, byId) {
  for (const m of players) {
    const g = m.medigun;
    if (!m.alive || !g || g.target == null) continue;
    const t = byId.get(g.target);
    if (!t || !t.alive) continue;
    const [x0, y0] = toScreen(m.pos[0], m.pos[1]), [x1, y1] = toScreen(t.pos[0], t.pos[1]);
    ctx.save();
    ctx.beginPath(); ctx.moveTo(x0, y0); ctx.lineTo(x1, y1);
    ctx.strokeStyle = TEAM[m.team]; ctx.globalAlpha = g.releasing ? 0.95 : 0.6;
    ctx.lineWidth = g.releasing ? 6 : 3; ctx.lineCap = "round";
    if (g.releasing) { ctx.shadowColor = TEAM[m.team]; ctx.shadowBlur = 14; }
    ctx.setLineDash([10, 6]); ctx.lineDashOffset = -performance.now() / 25;
    ctx.stroke();
    ctx.restore();
  }
}

// The class icons are SVGs, which Firefox rasterizes again on every draw: each size is drawn once.
const iconSprites = new Map();
function iconSprite(cls, size) {
  const img = icon(cls);
  if (!img) return null;
  const n = Math.max(1, Math.round(size * (window.devicePixelRatio || 1))), key = `${cls} ${n}`;
  let c = iconSprites.get(key);
  if (!c) {
    if (iconSprites.size > 64) iconSprites.clear();
    c = document.createElement("canvas");
    c.width = c.height = n;
    c.getContext("2d").drawImage(img, 0, 0, n, n);
    iconSprites.set(key, c);
  }
  return c;
}

export function drawPlayers(players) {
  const alive = [];
  // The followed player is drawn last, so their dot and name are on top.
  players = [...players].sort((a, b) => (a.id === state.follow) - (b.id === state.follow));
  for (const p of players) {
    if (p.team !== "red" && p.team !== "blue") continue;
    if (!p.alive) {
      // The overlay keeps an X where each dead player fell until they respawn, like a CS radar.
      if (OVERLAY && p.pos) {
        const [x, y] = toScreen(p.pos[0], p.pos[1]), s = overlayUnit() * 0.6;
        ctx.save(); ctx.globalAlpha = 0.75; ctx.lineCap = "round";
        ctx.strokeStyle = "#000"; ctx.lineWidth = 5;
        ctx.beginPath(); ctx.moveTo(x - s, y - s); ctx.lineTo(x + s, y + s); ctx.moveTo(x + s, y - s); ctx.lineTo(x - s, y + s); ctx.stroke();
        ctx.strokeStyle = TEAM[p.team]; ctx.lineWidth = 2.5; ctx.stroke();
        ctx.restore();
      }
      continue;
    }
    const [x, y] = toScreen(p.pos[0], p.pos[1]);
    alive.push({ p, x, y, rad: playerRad(), col: TEAM[p.team], cloaked: p.conds?.includes("Stealthed") });
  }
  // In passes, so in a clump every class icon sits on top of the other players' discs.
  for (const { p, x, y, rad, col, cloaked } of alive) {
    ctx.globalAlpha = cloaked ? 0.35 : 1;
    // A pointer on the disc where they look; how wide they see is up to their client. demos.tf's
    // dart (0.8 of the disc's radius long, 0.6 of that to each side, notched base) from the health ring.
    const a = screenAngle(p.yaw), base = rad + 3, len = rad * 0.8, half = len * 0.6;
    const at = (f, l) => [x + Math.cos(a) * f - Math.sin(a) * l, y + Math.sin(a) * f + Math.cos(a) * l];
    ctx.beginPath();
    for (const [f, l] of [[base, -half], [base + len * 0.2, 0], [base, half], [base + len, 0]]) ctx.lineTo(...at(f, l));
    ctx.closePath();
    ctx.fillStyle = col; ctx.fill();
    ctx.lineWidth = OVERLAY ? Math.max(1.5, rad * 0.16) : 1.5; ctx.strokeStyle = "#000"; ctx.stroke();
  }
  for (const { p, x, y, rad, col, cloaked } of alive) {
    ctx.globalAlpha = cloaked ? 0.35 : 1;
    if (p.ubered) drawUberRing(p, x, y, rad, col);
    ctx.beginPath(); ctx.arc(x, y, rad, 0, Math.PI * 2);
    ctx.fillStyle = col; ctx.fill();
    ctx.lineWidth = OVERLAY ? Math.max(1.5, rad * 0.16) : 1.5; ctx.strokeStyle = "#000"; ctx.stroke();
    const max = p.max_hp || 1, frac = Math.max(0, Math.min(1, p.hp / max));
    const ring = (from, to, color) => {
      ctx.beginPath(); ctx.arc(x, y, rad + 2.5, -Math.PI / 2 + Math.PI * 2 * from, -Math.PI / 2 + Math.PI * 2 * to);
      ctx.strokeStyle = color; ctx.stroke();
    };
    ctx.lineWidth = OVERLAY ? Math.max(2.5, rad * 0.26) : 2.5;
    ring(0, frac, frac > 0.5 ? "#9fd27f" : frac > 0.25 ? "#e0c040" : "#e05a40");
    // Overheal laps the green ring in blue, full at the medigun cap: +50%, rounded down to 5.
    const over = Math.min(1, (p.hp - max) / (Math.floor(max * 1.5 / 5) * 5 - max || 1));
    if (over > 0) ring(0, over, "#1f4fe0");
  }
  // Flames over the dots, so they show in a fight, but under the class icons.
  for (const { p, x, y, rad } of alive) if (p.flames) drawFlames(p, x, y, rad, screenAngle(p.yaw));
  for (const { p, x, y, rad, cloaked } of alive) {
    ctx.globalAlpha = cloaked ? 0.35 : 1;
    const img = iconSprite(p.class, rad * 1.65);
    if (img) {
      const s = rad * 1.65;
      ctx.drawImage(img, x - s / 2, y - s / 2, s, s);
    } else {
      ctx.fillStyle = "#fff"; ctx.font = `bold ${Math.round(Math.max(9, rad * 0.9))}px system-ui`;
      ctx.textAlign = "center"; ctx.textBaseline = "middle";
      ctx.fillText(CLASS[p.class] || "?", x, y + 0.5);
    }
    if (p.id === state.follow) {
      ctx.beginPath(); ctx.arc(x, y, rad + 7, 0, Math.PI * 2); ctx.strokeStyle = "#fff"; ctx.lineWidth = 2; ctx.stroke();
    }
  }
  if (layerOn("effects")) for (const { p, x, y, rad, cloaked } of alive) {
    if (!p.conds?.length) continue;
    ctx.globalAlpha = cloaked ? 0.35 : 1;
    drawStatus(p.conds, p.id, x, y, rad);
  }
  for (const { p, x, y, rad, cloaked } of alive) {
    ctx.globalAlpha = cloaked ? 0.35 : 1;
    if (!OVERLAY || OPT.get("names") === "1") {
      const size = OVERLAY ? Math.round(Math.max(11, rad * 0.95)) : Math.round(12 * uiScale());
      ctx.font = `700 ${size}px system-ui`; ctx.textBaseline = "top"; ctx.textAlign = "center";
      const label = p.name.slice(0, OVERLAY ? 12 : 18), ly = y + rad + 4;
      ctx.lineWidth = 3; ctx.strokeStyle = "#0b0c0e"; ctx.strokeText(label, x, ly);
      ctx.fillStyle = "#e8e8e8"; ctx.fillText(label, x, ly);
    }
  }
  ctx.globalAlpha = 1;
}

// Small badges on the dot's top-right arc, one per effect; marked-for-death sits above the dot and
// stun stars orbit it. Nothing moves the dot or covers its icon and ring.
function drawStatus(conds, id, x, y, rad) {
  const t = performance.now() / 1000, r = Math.max(3, rad * 0.4), lw = Math.max(1, r * 0.2);
  const has = n => conds.includes(n);
  const drop = (cx, cy, k, fill) => {
    ctx.beginPath(); ctx.moveTo(cx, cy - k * 1.3);
    ctx.bezierCurveTo(cx + k * 0.95, cy - k * 0.1, cx + k * 0.9, cy + k, cx, cy + k);
    ctx.bezierCurveTo(cx - k * 0.9, cy + k, cx - k * 0.95, cy - k * 0.1, cx, cy - k * 1.3);
    ctx.fillStyle = fill; ctx.fill(); ctx.stroke();
  };
  const badges = [];
  if (has("Burning")) badges.push((cx, cy) => {
    const f = 1 + 0.18 * Math.sin(t * 17 + id), sway = Math.sin(t * 11 + id) * r * 0.15;
    drop(cx + sway, cy, r * f, "#ff7a1a"); drop(cx + sway, cy + r * 0.25, r * 0.5 * f, "#ffd23a");
  });
  if (has("Urine")) badges.push((cx, cy) => drop(cx, cy, r, "#f2d627"));
  if (has("MadMilk")) badges.push((cx, cy) => drop(cx, cy, r, "#fff"));
  if (has("Bleeding")) badges.push((cx, cy) => { drop(cx - r * 0.45, cy - r * 0.2, r * 0.6, "#d22"); drop(cx + r * 0.5, cy + r * 0.3, r * 0.6, "#d22"); });
  ctx.save(); ctx.lineWidth = lw; ctx.strokeStyle = "#000";
  const d = rad + r * 0.9 + 3;
  badges.forEach((draw, i) => {
    const a = -Math.PI / 4 + i * 0.85;
    draw(x + Math.cos(a) * d, y + Math.sin(a) * d);
  });
  if (has("MarkedForDeath") || has("MarkedForDeathSilent")) {
    const cx = x, cy = y - rad - r * 1.3 - 3;
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.95, 0, Math.PI * 2); ctx.fillStyle = "#fff"; ctx.fill(); ctx.stroke();
    ctx.fillStyle = "#000"; ctx.beginPath();
    ctx.arc(cx - r * 0.35, cy - r * 0.1, r * 0.25, 0, Math.PI * 2); ctx.arc(cx + r * 0.35, cy - r * 0.1, r * 0.25, 0, Math.PI * 2); ctx.fill();
  }
  if (has("Stunned")) for (let i = 0; i < 3; i++) {
    const a = t * 4 + id + i * Math.PI * 2 / 3, cx = x + Math.cos(a) * rad * 0.9, cy = y - rad * 0.8 + Math.sin(a) * rad * 0.28, k = r * 0.55;
    ctx.beginPath(); ctx.moveTo(cx, cy - k); ctx.lineTo(cx + k * 0.3, cy - k * 0.3); ctx.lineTo(cx + k, cy); ctx.lineTo(cx + k * 0.3, cy + k * 0.3);
    ctx.lineTo(cx, cy + k); ctx.lineTo(cx - k * 0.3, cy + k * 0.3); ctx.lineTo(cx - k, cy); ctx.lineTo(cx - k * 0.3, cy - k * 0.3); ctx.closePath();
    ctx.fillStyle = "#ffe14a"; ctx.fill(); ctx.stroke();
  }
  ctx.restore();
}

// A firing flamethrower: a flickering cone about as long as the flames reach. Crit flames take
// the team's colour, and burn brighter while they hit someone.
function drawFlames(p, x, y, rad, a) {
  const t = performance.now() / 1000, len = Math.max(350 * state.view.scale, rad * (OVERLAY ? 4 : 3)) * (0.92 + 0.08 * Math.sin(t * 23 + p.id));
  const hot = p.flames.crit ? (p.team === "red" ? "255,90,60" : "90,160,255") : "255,150,40";
  const g = ctx.createRadialGradient(x, y, rad + 3, x, y, len);
  g.addColorStop(0, `rgba(${hot},${p.flames.hit ? 0.85 : 0.6})`);
  g.addColorStop(1, `rgba(${hot},0)`);
  ctx.beginPath(); ctx.arc(x, y, rad + 3, a + 0.3, a - 0.3, true); ctx.arc(x, y, len, a - 0.3, a + 0.3); ctx.closePath();
  ctx.fillStyle = g; ctx.fill();
}

export function drawEffects(byId) {
  const now = performance.now();
  state.effects = state.effects.filter(e => now - e.t0 < e.life);
  for (const e of state.effects) {
    const k = (now - e.t0) / e.life;
    ctx.globalAlpha = 1 - k;
    if (e.kind === "shot") {
      const [x, y] = toScreen(e.ev.origin[0], e.ev.origin[1]);
      const a = screenAngle(e.ev.yaw), len = 900 * Math.cos(e.ev.pitch * Math.PI / 180) * state.view.scale;
      ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(x + Math.cos(a) * len, y + Math.sin(a) * len);
      ctx.strokeStyle = e.team === "red" ? "#ffb3a8" : e.team === "blue" ? "#b3dcff" : "#ffe9a8"; ctx.lineWidth = 1.2; ctx.stroke();
    } else if (e.kind === "sentryshot") {
      const [x, y] = toScreen(e.from[0], e.from[1]);
      const t = byId.get(e.target);
      let [tx, ty] = [0, 0];
      if (t?.pos) [tx, ty] = toScreen(t.pos[0], t.pos[1]);
      else { const a = screenAngle(e.aim || 0); [tx, ty] = [x + Math.cos(a) * 1100 * state.view.scale, y + Math.sin(a) * 1100 * state.view.scale]; }
      ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(tx, ty);
      ctx.strokeStyle = "#ffe9a8"; ctx.lineWidth = 1.5; ctx.stroke();
      ctx.beginPath(); ctx.arc(x, y, px(14, 5), 0, Math.PI * 2); ctx.fillStyle = "#ffd36a"; ctx.fill();
    } else if (e.kind === "boom") {
      const [x, y] = toScreen(e.ev.origin[0], e.ev.origin[1]);
      ctx.beginPath(); ctx.arc(x, y, Math.max(px(146, 6), OVERLAY ? overlayUnit() * 1.2 : 0) * (0.35 + 0.65 * k), 0, Math.PI * 2);
      ctx.fillStyle = "#ffa04040"; ctx.fill(); ctx.strokeStyle = "#ffb050"; ctx.lineWidth = 2; ctx.stroke();
    } else if (e.kind === "dmg" && !OVERLAY) {
      const pos = byId.get(e.ev.victim)?.pos || e.pos;
      if (!pos) continue;
      const [x, y] = toScreen(pos[0], pos[1]);
      ctx.font = `bold ${Math.round((e.ev.crit ? 15 : 12) * uiScale())}px system-ui`; ctx.textAlign = "center"; ctx.textBaseline = "bottom";
      ctx.fillStyle = e.ev.crit ? "#7dff6a" : e.ev.minicrit ? "#ffd04a" : "#ff5a4a";
      ctx.strokeStyle = "#000"; ctx.lineWidth = 3;
      const ty = y - playerRad() - 4 - k * 22;
      ctx.strokeText("-" + e.ev.amount, x, ty); ctx.fillText("-" + e.ev.amount, x, ty);
    } else if (e.kind === "kill" && !OVERLAY) {
      const v = e.ev.victim_pos;
      if (!v) continue;
      const [x, y] = toScreen(v[0], v[1]), s = 7;
      if (e.ev.attacker_pos && e.ev.attacker !== e.ev.victim) {
        const [ax, ay] = toScreen(e.ev.attacker_pos[0], e.ev.attacker_pos[1]);
        ctx.setLineDash([4, 4]); ctx.beginPath(); ctx.moveTo(ax, ay); ctx.lineTo(x, y);
        ctx.strokeStyle = "#fff8"; ctx.lineWidth = 1; ctx.stroke(); ctx.setLineDash([]);
      }
      ctx.strokeStyle = TEAM[e.team] || "#fff"; ctx.lineWidth = 3;
      ctx.beginPath(); ctx.moveTo(x - s, y - s); ctx.lineTo(x + s, y + s); ctx.moveTo(x + s, y - s); ctx.lineTo(x - s, y + s); ctx.stroke();
    } else if (e.kind === "spawn" && e.pos) {
      const [x, y] = toScreen(e.pos[0], e.pos[1]);
      ctx.beginPath(); ctx.arc(x, y, playerRad() + k * 18, 0, Math.PI * 2); ctx.strokeStyle = TEAM[e.team] || "#fff"; ctx.lineWidth = 2; ctx.stroke();
    } else if (e.kind === "airblast") {
      // A puff of air widening out in front of the pyro.
      const pl = byId.get(e.id); if (!pl?.pos) continue;
      const [x, y] = toScreen(pl.pos[0], pl.pos[1]), a = screenAngle(pl.yaw);
      const rad = playerRad(), r = Math.max((40 + 220 * k) * state.view.scale, rad * (1.4 + 2 * k));
      ctx.beginPath(); ctx.arc(x, y, rad + 3, a + 0.7, a - 0.7, true); ctx.arc(x, y, r, a - 0.7, a + 0.7); ctx.closePath();
      ctx.fillStyle = "#e8f4ff30"; ctx.fill();
      ctx.beginPath(); ctx.arc(x, y, r, a - 0.7, a + 0.7);
      ctx.strokeStyle = "#e8f4ff"; ctx.lineWidth = Math.max(3, rad * 0.35); ctx.lineCap = "round"; ctx.stroke(); ctx.lineCap = "butt";
    } else if (e.kind === "heal") {
      const pos = byId.get(e.id)?.pos || e.ev.pos;
      if (!pos) continue;
      const [x, y] = toScreen(pos[0], pos[1]);
      ctx.font = `bold ${Math.round(12 * uiScale())}px system-ui`; ctx.textAlign = "center"; ctx.textBaseline = "bottom";
      ctx.fillStyle = "#6fe08a"; ctx.strokeStyle = "#000"; ctx.lineWidth = 3;
      const ty = y - playerRad() - 4 - k * 22, tx = x + playerRad() * 0.9, txt = "+" + e.ev.amount;
      ctx.strokeText(txt, tx, ty); ctx.fillText(txt, tx, ty);
    } else if (e.kind === "hit") {
      const [x, y] = toScreen(e.ev.pos[0], e.ev.pos[1]);
      const seed = Math.abs(Math.round(e.ev.pos[0] * 7 + e.ev.pos[1] * 13)), r = Math.max(5 * uiScale(), px(40, 5));
      const blood = e.ev.kind === "blood";
      ctx.strokeStyle = blood ? "#c4302b" : e.ev.kind === "sparks" ? "#ffd36a" : "#e8f4ff"; ctx.fillStyle = ctx.strokeStyle;
      ctx.lineWidth = 1.5; ctx.lineCap = "round";
      for (let i = 0; i < (blood ? 6 : 4); i++) {
        const a = ((seed * (i + 3)) % 628) / 100, d = r * (blood ? 0.4 + k : 0.5 + 1.2 * k);
        if (blood) { ctx.beginPath(); ctx.arc(x + Math.cos(a) * d, y + Math.sin(a) * d, Math.max(1.5, r * 0.18 * (1 - k)), 0, Math.PI * 2); ctx.fill(); }
        else { ctx.beginPath(); ctx.moveTo(x + Math.cos(a) * d, y + Math.sin(a) * d); ctx.lineTo(x + Math.cos(a) * (d + r * 0.4), y + Math.sin(a) * (d + r * 0.4)); ctx.stroke(); }
      }
      ctx.lineCap = "butt";
    } else if (e.kind === "packflash") {
      const [x, y] = toScreen(e.pos[0], e.pos[1]);
      ctx.beginPath(); ctx.arc(x, y, px(40, 8) * (1 + 1.5 * k), 0, Math.PI * 2); ctx.strokeStyle = "#fff"; ctx.lineWidth = 2.5; ctx.stroke();
    } else if (e.kind === "capbreak") {
      const [x, y] = toScreen(e.pos[0], e.pos[1]), half = OVERLAY ? overlayUnit() * 1.2 : px(110, 11);
      ctx.beginPath(); ctx.arc(x, y, half * (1.1 + 1.2 * k), 0, Math.PI * 2); ctx.strokeStyle = "#ffb050"; ctx.lineWidth = 3; ctx.stroke();
    } else if (e.kind === "uber") {
      const m = byId.get(e.id); if (!m?.pos) continue;
      const [x, y] = toScreen(m.pos[0], m.pos[1]);
      ctx.beginPath(); ctx.arc(x, y, playerRad() + 6 + k * 50, 0, Math.PI * 2); ctx.strokeStyle = TEAM[e.team]; ctx.lineWidth = 4; ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;
}
