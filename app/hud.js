import { MS_PER_TICK, TEAM, CLASS, byClass, ICONS, $, OVERLAY, OPT, state } from "./core.js";
import { layerOn } from "./layers.js";

// ---------- HUD and header, refreshed a few times per second
const UBER_LABEL = { uber: "ÜBER", kritz: "KRITZ", quickfix: "QUICK-FIX", vaccinator_bullet: "VACC · BULLET", vaccinator_blast: "VACC · BLAST", vaccinator_fire: "VACC · FIRE" };
export const MEDIGUN_NAME = { medigun: "Medi Gun", kritzkrieg: "Kritzkrieg", quickfix: "Quick-Fix", vaccinator: "Vaccinator" };

// Cards and meters are kept and updated in place: rebuilding them would restart their
// animations (the über stripes) several times a second.
export const hudNodes = new Map();
function node(key, html) {
  let el = hudNodes.get(key);
  if (!el) {
    const t = document.createElement("template");
    t.innerHTML = html.trim();
    el = t.content.firstElementChild;
    hudNodes.set(key, el);
  }
  return el;
}
function setHTML(el, html) { if (el.innerHTML !== html) el.innerHTML = html; }
function setClass(el, cls) { if (el.className !== cls) el.className = cls; }
function setStyle(el, prop, value) { if (el.style[prop] !== value) el.style[prop] = value; }

let itemImages = null;
function weaponInfo(p) {
  if (!layerOn("loadout") || !p.alive || !p.weapon) return null;
  if (!itemImages) {
    itemImages = {};
    fetch("backpack/items.json").then(r => r.json()).then(j => { itemImages = j; state.lastPanel = 0; }).catch(() => {});
    return null;
  }
  const img = itemImages[p.weapon.item];
  return img ? { img, clip: p.weapon.clip } : null;
}
function card(p) {
  const el = node(`p${p.id}`, `<div class="card"><div class="fill"></div><div class="oh"></div><span class="ci"></span><div class="nm"><b></b><small><span></span><img class="wp" alt="" hidden><span class="clip"></span></small></div><div class="hp"></div></div>`);
  const f = p.alive ? Math.max(0, Math.min(1, p.hp / (p.max_hp || 1))) : 0;
  if (el.dataset.id !== String(p.id)) el.dataset.id = p.id;
  setClass(el, ["card", p.id === state.follow && "followed", !p.alive && "dead", p.alive && p.hp > p.max_hp && "over", p.alive && f < 0.35 && "low", p.ubered && "ub"].filter(Boolean).join(" "));
  setStyle(el.children[0], "clipPath", `inset(0px ${(100 - f * 100).toFixed(1)}% 0px 0px)`);
  const oc = p.alive && p.hp > p.max_hp ? Math.min(1, (p.hp - p.max_hp) / (Math.floor(p.max_hp * 1.5 / 5) * 5 - p.max_hp || 1)) : 0;
  setStyle(el.children[1], "clipPath", `inset(0px ${(100 - oc * 100).toFixed(1)}% 0px 0px)`);
  setHTML(el.children[2], ICONS[p.class] ? `<img src="icons/${p.class}.svg" alt="${p.class}">` : CLASS[p.class] || "?");
  setText2(el.querySelector(".nm b"), p.name);
  const sub = p.ping != null ? `${p.ping} ms` : "";
  const [subEl, wp, clip] = el.querySelector(".nm small").children;
  const fx = layerOn("effects") && p.alive ? [["Burning", "burn", "On fire"], ["Urine", "jar", "Jarate"], ["MadMilk", "milk", "Mad Milk"], ["Bleeding", "bleed", "Bleeding"]]
    .filter(([c]) => p.conds?.includes(c)).map(([, k, t]) => `<i class="fx ${k}" title="${t}"></i>`).join("") : "";
  setHTML(subEl, `${fx}${p.ubered ? `<span class="tag">${UBER_LABEL[p.ubered] || "ÜBER"}</span>` : ""}${sub}`);
  const w = weaponInfo(p);
  if (w && wp.dataset.img !== w.img) {
    wp.dataset.img = w.img;
    wp.style.display = "";
    wp.src = `backpack/${w.img}.webp`;
    wp.onerror = () => { wp.style.display = "none"; };
  }
  wp.hidden = !w;
  setText2(clip, w && w.clip != null ? String(w.clip) : "");
  setText2(el.children[4], p.alive ? String(p.hp) : "DEAD");
  return el;
}
function chargeMeter(p) {
  const el = node(`c${p.id}`, `<div class="charge"><div class="bar"></div><i class="segs"></i><span></span><span style="opacity:.75"></span><span class="pct"></span></div>`);
  const g = p.medigun || { kind: "medigun" };
  const pct = Math.round(p.uber ?? 0);
  setClass(el, ["charge", !p.alive && "dead", g.releasing && "releasing", !g.releasing && pct >= 100 && "ready"].filter(Boolean).join(" "));
  setStyle(el.children[0], "clipPath", `inset(0px ${100 - pct}% 0px 0px)`);
  // The Vaccinator charges in four 25% pips; the others in one bar.
  setHTML(el.children[1], g.kind === "vaccinator" ? [25, 50, 75].map(x => `<i class="seg" style="left:${x}%"></i>`).join("") : "");
  setText2(el.children[2], g.releasing ? "ACTIVE" : pct >= 100 ? "READY" : "ÜBERCHARGE");
  setText2(el.children[3], (MEDIGUN_NAME[g.kind] || g.kind) + (g.kind === "vaccinator" && g.resist ? ` · ${g.resist}` : ""));
  setText2(el.children[4], `${pct}%`);
  return el;
}
function setText2(el, text) { if (el.textContent !== text) el.textContent = text; }
function placeChildren(box, nodes) {
  // Only touch the DOM order when it changed: moving a node restarts its animations too.
  const same = box.children.length === nodes.length && nodes.every((n, i) => box.children[i] === n);
  if (!same) box.replaceChildren(...nodes);
}

export function updatePanel(players, snap) {
  const now = performance.now();
  if (now - state.lastPanel < 250) return;
  state.lastPanel = now;
  for (const team of ["blue", "red"]) {
    const list = players.filter(p => p.team === team).sort(byClass);
    placeChildren($("hud-" + team), [...list.map(card), ...list.filter(p => p.class === "medic").map(chargeMeter)]);
  }
  renderPoints(snap);
}

// ---------- TF2-style scoreboard: team scores, round timer, control points
const ROUND_LABEL = { pregame: "WARMUP", preround: "GET READY", team_win: "ROUND OVER", stalemate: "STALEMATE", game_over: "GAME OVER", bonus: "BONUS", between_rounds: "", restart: "", running: "" };
function clock(secs) {
  const s = Math.max(0, Math.ceil(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
export function mmss(secs) { return `${Math.floor(secs / 60)}:${String(Math.floor(secs % 60)).padStart(2, "0")}`; }
function setText(id, text) { const el = $(id); if (el.textContent !== text) el.textContent = text; }
export function renderScoreboard(snap, tick) {
  $("scoreboard").hidden = OVERLAY && OPT.get("score") !== "1";
  // CTF counts the round in captured intelligence, like TF2's CTF HUD; maps with a neutral flag
  // (bball, special delivery) keep the team score.
  const ctf = snap.captures && (snap.objectives || []).some(o => o.kind === "flag" && (o.team === "red" || o.team === "blue"));
  const scores = ctf ? snap.captures : snap.score;
  setText("score-blue", String(scores?.blue ?? 0));
  setText("score-red", String(scores?.red ?? 0));
  const timers = snap.timers || [];
  const left = t => t.paused ? t.remaining : t.remaining - Math.max(0, tick - snap.tick) * MS_PER_TICK / 1000;
  const main = timers.find(t => t.team === "none") || (timers.length === 1 ? timers[0] : null);
  const byTeam = team => timers.find(t => t.team === team && t !== main);
  // Like TF2's HUD: the round timer when one is active, else the map's mp_timelimit; with
  // both, the map time goes underneath.
  const mapLeft = snap.time_left == null ? null : snap.time_left - Math.max(0, tick - snap.tick) * MS_PER_TICK / 1000;
  setText("timer-main", main ? clock(left(main)) : mapLeft != null ? clock(mapLeft) : "");
  setText("timer-map", main && mapLeft != null ? `MAP ${clock(mapLeft)}` : "");
  // Events are stamped with time into the round, which (unlike the round timer) never jumps.
  const into = snap.round_time == null ? null : snap.round_time + (snap.round === "running" ? Math.max(0, tick - snap.tick) * MS_PER_TICK / 1000 : 0);
  state.clock = into == null ? "" : mmss(into);
  setText("timer-blue", byTeam("blue") ? clock(left(byTeam("blue"))) : "");
  setText("timer-red", byTeam("red") ? clock(left(byTeam("red"))) : "");
  const label = main?.setup ? "SETUP" : ROUND_LABEL[snap.round] ?? "";
  // A paused round timer is not a paused game (KotH, no match started): it just stands still.
  setText("round-label", label || (main && !main.paused && left(main) <= 0 ? "OVERTIME" : ""));
  const shown = main && !main.paused ? left(main) : !main ? mapLeft : null;
  $("timer-main").parentElement.classList.toggle("low", shown != null && shown <= 30);
}
function renderPoints(snap) {
  const html = (snap.objectives || []).filter(o => o.kind === "control_point").map(o => {
    const capping = o.capping_team !== "none" && o.cap_progress > 0;
    const style = `--own:${TEAM[o.owner] || "#8a8a8a"};--cap:${capping ? TEAM[o.capping_team] : "transparent"};--p:${Math.round((capping ? o.cap_progress : 0) * 100)}%`;
    return `<div class="cp${o.locked ? " locked" : ""}${capping ? " capping" : ""}" style="${style}">${o.index + 1}</div>`;
  }).join("");
  if ($("cps").innerHTML !== html) $("cps").innerHTML = html;
}
