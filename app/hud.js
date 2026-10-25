import { msPerTick, TEAM, CLASS, byClass, ICONS, $, OVERLAY, OPT, state } from "./core.js";
import { asset } from "./env.js";
import { layerOn } from "./layers.js";
import { esc } from "./feed.js";

// The player panels redraw at most this often.
const PANEL_MS = 250;
// A full medigun charge, in percent.
const FULL_CHARGE = 100;
// A timer turns red with this many seconds left, like TF2's HUD.
const LOW_SECONDS = 30;

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
function weaponImage(p) {
  if (!layerOn("loadout") || !p.alive || !p.weapon) return null;
  if (!itemImages) {
    itemImages = {};
    fetch(asset("backpack/items.json")).then(r => r.json()).then(j => { itemImages = j; state.lastPanel = 0; }).catch(() => {});
    return null;
  }
  return itemImages[p.weapon.item] || null;
}
function card(p) {
  const el = node(`p${p.id}`, `<div class="card"><div class="fill"></div><div class="oh"></div><span class="ci"></span><div class="nm"><b></b><small><span></span><img class="wp" alt="" hidden></small></div><span class="fx"></span><div class="hp"></div></div>`);
  const f = p.alive ? Math.max(0, Math.min(1, p.hp / (p.max_hp || 1))) : 0;
  if (el.dataset.id !== String(p.id)) el.dataset.id = p.id;
  setClass(el, ["card", p.id === state.follow && "followed", !p.alive && "dead", p.alive && p.hp > p.max_hp && "over", p.alive && f < 0.35 && "low", p.ubered && "ub"].filter(Boolean).join(" "));
  setStyle(el.children[0], "clipPath", `inset(0px ${(100 - f * 100).toFixed(1)}% 0px 0px)`);
  const oc = p.alive && p.hp > p.max_hp ? Math.min(1, (p.hp - p.max_hp) / (Math.floor(p.max_hp * 1.5 / 5) * 5 - p.max_hp || 1)) : 0;
  setStyle(el.children[1], "clipPath", `inset(0px ${(100 - oc * 100).toFixed(1)}% 0px 0px)`);
  setHTML(el.children[2], ICONS[p.class] ? `<img src="${asset(`icons/${p.class}.svg`)}" alt="${p.class}">` : CLASS[p.class] || "?");
  setText2(el.querySelector(".nm b"), p.name);
  const sub = p.ping != null ? `${p.ping} ms` : "";
  const [subEl, wp] = el.querySelector(".nm small").children;
  const fx = layerOn("effects") && p.alive ? [["Burning", "burn", "On fire"], ["Urine", "jar", "Jarate"], ["MadMilk", "milk", "Mad Milk"], ["Bleeding", "bleed", "Bleeding"]]
    .filter(([c]) => p.conds?.includes(c)).map(([, k, t]) => `<i class="${k}" title="${t}"></i>`).join("") : "";
  setHTML(el.children[4], fx);
  setHTML(subEl, `${p.ubered ? `<span class="tag">${UBER_LABEL[p.ubered] || "ÜBER"}</span>` : ""}${sub}`);
  const w = weaponImage(p);
  if (w && wp.dataset.img !== w) {
    wp.dataset.img = w;
    wp.style.display = "";
    wp.src = asset(`backpack/${w}.webp`);
    wp.onerror = () => { wp.style.display = "none"; };
  }
  wp.hidden = !w;
  setText2(el.children[5], p.alive ? String(p.hp) : p.respawn > 0 ? `${Math.ceil(p.respawn)}s` : "DEAD");
  setClass(el.children[5], !p.alive && p.respawn > 0 ? "hp rs" : "hp");
  return el;
}
function chargeMeter(p) {
  const el = node(`c${p.id}`, `<div class="charge"><div class="bar"></div><i class="segs"></i><span></span><span style="opacity:.75"></span><span class="pct"></span></div>`);
  const g = p.medigun || { kind: "medigun" };
  const pct = Math.round(p.uber ?? 0);
  setClass(el, ["charge", !p.alive && "dead", g.releasing && "releasing", !g.releasing && pct >= FULL_CHARGE && "ready"].filter(Boolean).join(" "));
  setStyle(el.children[0], "clipPath", `inset(0px ${100 - pct}% 0px 0px)`);
  // The Vaccinator charges in four 25% pips; the others in one bar.
  setHTML(el.children[1], g.kind === "vaccinator" ? [25, 50, 75].map(x => `<i class="seg" style="left:${x}%"></i>`).join("") : "");
  setText2(el.children[2], g.releasing ? "ACTIVE" : pct >= FULL_CHARGE ? "READY" : "ÜBERCHARGE");
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
  if (now - state.lastPanel < PANEL_MS) return;
  state.lastPanel = now;
  for (const team of ["blue", "red"]) {
    const list = players.filter(p => p.team === team).sort(byClass);
    placeChildren($("hud-" + team), [...list.map(card), ...list.filter(p => p.class === "medic").map(chargeMeter)]);
  }
  renderPoints(snap);
  if (!$("board").hidden) renderBoard(players, snap);
}

// Time added to a round timer: "+M:SS" floating down under the scoreboard (on king of the hill,
// under the team's side).
export function showTimeAdded(ev) {
  const el = document.createElement("div");
  el.className = `added ${$("scoreboard").classList.contains("koth") && (ev.team === "red" || ev.team === "blue") ? ev.team : ""}`;
  el.textContent = `+${clock(ev.seconds)}`;
  el.addEventListener("animationend", () => el.remove());
  $("scoreboard").append(el);
}

// The scoreboard while Tab is held: each team by points, like TF2's.
const BOARD_COLUMNS = [["points", "Points"], ["kills", "K"], ["deaths", "D"], ["assists", "A"], ["damage", "Damage"], ["healing", "Healing"]];
export function renderBoard(players, snap) {
  const team = (t, label) => {
    const rows = players.filter(p => p.team === t).sort((a, b) => (b.score?.points ?? 0) - (a.score?.points ?? 0) || (b.score?.kills ?? 0) - (a.score?.kills ?? 0));
    const body = rows.map(p => {
      const s = p.score || {}, status = !p.alive && p.respawn > 0 ? ` <small>${Math.ceil(p.respawn)}s</small>` : "";
      const icon = ICONS[p.class] ? `<img src="${asset(`icons/${p.class}.svg`)}" alt="">` : "";
      return `<tr class="${p.alive ? "" : "dead"}"><td class="bc">${icon}</td><td class="bn">${esc(p.name)}${status}</td>${BOARD_COLUMNS.map(([k]) => `<td>${s[k] ?? ""}</td>`).join("")}<td>${p.ping ?? ""}</td></tr>`;
    }).join("");
    const score = (snap.score || {})[t] ?? 0;
    return `<div class="bteam ${t}"><div class="bhead"><b>${label}</b><span>${score}</span><i>${rows.length} players</i></div>` +
      `<table><thead><tr><th></th><th class="bn">Name</th>${BOARD_COLUMNS.map(([, h]) => `<th>${h}</th>`).join("")}<th>Ping</th></tr></thead><tbody>${body}</tbody></table></div>`;
  };
  setHTML($("board"), team("blue", "BLU") + team("red", "RED"));
}

// ---------- TF2-style scoreboard: team scores, round timer, control points
const ROUND_LABEL = { pregame: "WARMUP", preround: "GET READY", team_win: "ROUND OVER", stalemate: "STALEMATE", game_over: "GAME OVER", bonus: "BONUS", between_rounds: "", restart: "", running: "" };
function clock(secs) {
  const s = Math.max(0, Math.ceil(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
export function mmss(secs) { return `${Math.floor(secs / 60)}:${String(Math.floor(secs % 60)).padStart(2, "0")}`; }
function setText(id, text) { const el = $(id); if (el.textContent !== text) el.textContent = text; }
// CTF counts the round in captured intelligence, like TF2's CTF HUD, and pass time in goals;
// maps with a neutral flag (bball, special delivery) keep the team score. Robot and player
// destruction count points (other maps that reuse their logic count something else with it).
export function shownScore(snap) {
  const objectives = snap.objectives || [];
  const ctf = snap.captures && objectives.some(o => (o.kind === "flag" && (o.team === "red" || o.team === "blue")) || o.kind === "goal");
  const destruction = snap.points && /^(rd|pd)_/.test(state.map || "");
  return destruction ? snap.points : ctf ? snap.captures : snap.score;
}

export function renderScoreboard(snap, tick) {
  $("scoreboard").hidden = OVERLAY && OPT.get("score") !== "1";
  const scores = shownScore(snap);
  setText("score-blue", String(scores?.blue ?? 0));
  setText("score-red", String(scores?.red ?? 0));
  const timers = snap.timers || [];
  const left = t => t.paused ? t.remaining : t.remaining - Math.max(0, tick - snap.tick) * msPerTick / 1000;
  const main = timers.find(t => t.team === "none") || (timers.length === 1 ? timers[0] : null);
  const byTeam = team => timers.find(t => t.team === team && t !== main);
  // Like TF2's HUD: the round timer when one is active, else the map's mp_timelimit; with
  // both, the map time goes underneath.
  const mapLeft = snap.time_left == null ? null : snap.time_left - Math.max(0, tick - snap.tick) * msPerTick / 1000;
  // King of the hill: each team's timer, side by side in the middle like TF2's HUD; the one
  // ticking (the team holding the point) bright, the other dimmed.
  const koth = !main && byTeam("blue") && byTeam("red");
  if (koth) {
    const running = ["blue", "red"].some(t => !byTeam(t).paused);
    setHTML($("timer-main"), ["blue", "red"].map(t => {
      const tm = byTeam(t), s = left(tm);
      return `<em class="kt ${t}${running && tm.paused ? " idle" : ""}${!tm.paused && s <= LOW_SECONDS ? " low" : ""}">${clock(s)}</em>`;
    }).join(""));
  } else setText("timer-main", main ? clock(left(main)) : mapLeft != null ? clock(mapLeft) : "");
  $("scoreboard").classList.toggle("koth", Boolean(koth));
  // Mann vs. Machine: the robots left in the wave where the map time would go.
  const robots = snap.mvm ? (snap.mvm.classes || []).filter(c => !c.support).reduce((n, c) => n + c.count, 0) : 0;
  setText("timer-map", (main || koth) && mapLeft != null ? `MAP ${clock(mapLeft)}` : robots > 0 ? `${robots} ROBOT${robots === 1 ? "" : "S"}` : "");
  // Events are stamped with time into the round, which (unlike the round timer) never jumps.
  const into = snap.round_time == null ? null : snap.round_time + (snap.round === "running" ? Math.max(0, tick - snap.tick) * msPerTick / 1000 : 0);
  state.clock = into == null ? "" : mmss(into);
  setText("timer-blue", !koth && byTeam("blue") ? clock(left(byTeam("blue"))) : "");
  setText("timer-red", !koth && byTeam("red") ? clock(left(byTeam("red"))) : "");
  // Mann vs. Machine: the wave, like its HUD.
  const label = main?.setup ? "SETUP" : snap.mvm ? `WAVE ${snap.mvm.wave} / ${snap.mvm.waves}` : ROUND_LABEL[snap.round] ?? "";
  // A paused round timer is not a paused game (KotH, no match started): it just stands still.
  setText("round-label", label || (main && !main.paused && left(main) <= 0 ? "OVERTIME" : ""));
  const shown = main && !main.paused ? left(main) : !main ? mapLeft : null;
  $("timer-main").parentElement.classList.toggle("low", shown != null && shown <= LOW_SECONDS);
}
function renderPoints(snap) {
  const html = (snap.objectives || []).filter(o => o.kind === "control_point").map(o => {
    const capping = o.capping_team !== "none" && o.cap_progress > 0;
    const style = `--own:${TEAM[o.owner] || "#8a8a8a"};--cap:${capping ? TEAM[o.capping_team] : "transparent"};--p:${Math.round((capping ? o.cap_progress : 0) * 100)}%`;
    // Like TF2's HUD: blank, but the cap rate (×2 for two cappers) while it is being capped.
    const n = capping && o.on_point ? o.on_point[o.capping_team] : 0;
    return `<div class="cp${o.locked ? " locked" : ""}${capping ? " capping" : ""}" style="${style}">${n > 0 ? `\u00d7${n}` : ""}</div>`;
  }).join("");
  if ($("cps").innerHTML !== html) $("cps").innerHTML = html;
}
