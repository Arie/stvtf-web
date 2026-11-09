import { msPerTick, TEAM, CLASS, byClass, ICONS, $, OVERLAY, OPT, state, teamColour, clock } from "../core/core.js";
import { asset } from "../core/env.js";
import { layerOn } from "../core/layers.js";
import { esc } from "./feed.js";
/** @typedef {import("../../types/stream").GameEvent} GameEvent */
/** @typedef {import("../../types/stream").Player} Player */
/** @typedef {import("../../types/stream").Snapshot} Snapshot */
/** @typedef {import("../../types/stream").PlayerScore} PlayerScore */
/** @typedef {import("../../types/stream").TeamScore} TeamScore */
/** @typedef {import("../../types/stream").Timer} Timer */

// The player panels redraw at most this often.
const PANEL_MS = 250;
// A full medigun charge, in percent.
const FULL_CHARGE = 100;
// A timer turns red with this many seconds left, like TF2's HUD.
const LOW_SECONDS = 30;

// ---------- HUD and header, refreshed a few times per second
/** @type {{ [charge: string]: string }} */
const UBER_LABEL = {
  uber: "ÜBER",
  kritz: "KRITZ",
  quickfix: "QUICK-FIX",
  vaccinator_bullet: "VACC · BULLET",
  vaccinator_blast: "VACC · BLAST",
  vaccinator_fire: "VACC · FIRE",
};
/** @type {{ [kind: string]: string }} */
export const MEDIGUN_NAME = {
  medigun: "Medi Gun",
  kritzkrieg: "Kritzkrieg",
  quickfix: "Quick-Fix",
  vaccinator: "Vaccinator",
};

// Cards and meters are kept and updated in place: rebuilding them would restart their
// animations (the über stripes) several times a second.
/** @type {Map<string, HTMLElement>} */
export const hudNodes = new Map();
/** @param {string} key @param {string} html */
function node(key, html) {
  let el = hudNodes.get(key);
  if (!el) {
    const t = document.createElement("template");
    t.innerHTML = html.trim();
    el = /** @type {HTMLElement} */ (t.content.firstElementChild);
    hudNodes.set(key, el);
  }
  return el;
}
/** @param {Element} el @param {string} html */
function setHTML(el, html) {
  if (el.innerHTML !== html) el.innerHTML = html;
}
/** @param {Element} el @param {string} cls */
function setClass(el, cls) {
  if (el.className !== cls) el.className = cls;
}
/** @param {HTMLElement} el @param {"clipPath"} prop @param {string} value */
function setStyle(el, prop, value) {
  if (el.style[prop] !== value) el.style[prop] = value;
}

/** @type {{ [item: string]: string } | null} */
let itemImages = null;
/** @param {Player} p */
function weaponImage(p) {
  if (!layerOn("loadout") || !p.alive || !p.weapon) return null;
  if (!itemImages) {
    itemImages = {};
    fetch(asset("backpack/items.json"))
      .then(r => r.json())
      .then(j => {
        itemImages = j;
        state.lastPanel = 0;
      })
      .catch(() => {});
    return null;
  }
  return itemImages[p.weapon.item] || null;
}
/** @param {Player} p @param {number} f the health bar's fill */
function cardClass(p, f) {
  return [
    "card",
    p.id === state.follow && "followed",
    !p.alive && "dead",
    p.alive && p.hp > p.max_hp && "over",
    p.alive && f < LOW_HEALTH && "low",
    p.ubered && "ub",
  ]
    .filter(Boolean)
    .join(" ");
}
// A card is "low" below this much of its player's max health.
const LOW_HEALTH = 0.35,
  // The most overheal: this much of max health, rounded down to OVERHEAL_STEP.
  OVERHEAL = 1.5,
  OVERHEAL_STEP = 5;
// How far into the overheal a player is, 0 to 1, of the most there is.
/** @param {Player} p */
function overheal(p) {
  if (!(p.alive && p.hp > p.max_hp)) return 0;
  return Math.min(
    1,
    (p.hp - p.max_hp) / (Math.floor((p.max_hp * OVERHEAL) / OVERHEAL_STEP) * OVERHEAL_STEP - p.max_hp || 1),
  );
}
/** @param {Player} p */
function conditionIcons(p) {
  if (!(layerOn("effects") && p.alive)) return "";
  return [
    ["Burning", "burn", "On fire"],
    ["Urine", "jar", "Jarate"],
    ["MadMilk", "milk", "Mad Milk"],
    ["Bleeding", "bleed", "Bleeding"],
  ]
    .filter(([c]) => p.conds?.includes(c))
    .map(([, k, t]) => `<i class="${k}" title="${t}"></i>`)
    .join("");
}
/** @param {HTMLImageElement} wp @param {string | null} w */
function showWeapon(wp, w) {
  if (w && wp.dataset.img !== w) {
    wp.dataset.img = w;
    wp.style.display = "";
    wp.src = asset(`backpack/${w}.webp`);
    wp.onerror = () => {
      wp.style.display = "none";
    };
  }
  wp.hidden = !w;
}
// Whole seconds until a dead player respawns, or 0 while it is not known.
/** @param {Player} p */
const respawnIn = p => (p.alive ? 0 : Math.max(0, Math.ceil(p.respawn ?? 0)));
/** @param {Player} p */
function hpText(p) {
  if (p.alive) return String(p.hp);
  if (respawnIn(p)) return `${respawnIn(p)}s`;
  return "DEAD";
}
/** @param {Player} p */
function card(p) {
  const el = node(
    `p${p.id}`,
    `<div class="card"><div class="fill"></div><div class="oh"></div><span class="ci"></span><div class="nm"><b></b><small><span></span><img class="wp" alt="" hidden></small></div><span class="fx"></span><div class="hp"></div></div>`,
  );
  const f = p.alive ? Math.max(0, Math.min(1, p.hp / (p.max_hp || 1))) : 0;
  if (el.dataset.id !== String(p.id)) el.dataset.id = String(p.id);
  setClass(el, cardClass(p, f));
  setStyle(
    /** @type {HTMLElement} */ (el.children[0]),
    "clipPath",
    `inset(0px ${(100 - f * 100).toFixed(1)}% 0px 0px)`,
  );
  const oc = overheal(p);
  setStyle(
    /** @type {HTMLElement} */ (el.children[1]),
    "clipPath",
    `inset(0px ${(100 - oc * 100).toFixed(1)}% 0px 0px)`,
  );
  setHTML(
    el.children[2],
    ICONS[p.class] ? `<img src="${asset(`icons/${p.class}.svg`)}" alt="${p.class}">` : CLASS[p.class] || "?",
  );
  setText2(/** @type {Element} */ (el.querySelector(".nm b")), p.name);
  const sub = p.ping != null ? `${p.ping} ms` : "";
  const [subEl, wp] = /** @type {Element} */ (el.querySelector(".nm small")).children;
  setHTML(el.children[4], conditionIcons(p));
  setHTML(subEl, `${p.ubered ? `<span class="tag">${UBER_LABEL[p.ubered] || "ÜBER"}</span>` : ""}${sub}`);
  showWeapon(/** @type {HTMLImageElement} */ (wp), weaponImage(p));
  setText2(el.children[5], hpText(p));
  setClass(el.children[5], respawnIn(p) ? "hp rs" : "hp");
  return el;
}
/** @param {import("../../types/stream").Medigun} g @param {number} pct */
function chargeLabel(g, pct) {
  if (g.releasing) return "ACTIVE";
  if (pct >= FULL_CHARGE) return "READY";
  return "ÜBERCHARGE";
}
/** @param {Player} p */
function chargeMeter(p) {
  const el = node(
    `c${p.id}`,
    `<div class="charge"><div class="bar"></div><i class="segs"></i><span></span><span class="sub"></span><span class="pct"></span></div>`,
  );
  /** @type {import("../../types/stream").Medigun} */
  const g = p.medigun || { kind: "medigun", releasing: false, resist: null, target: null };
  const pct = Math.round(p.uber ?? 0);
  setClass(
    el,
    ["charge", !p.alive && "dead", g.releasing && "releasing", !g.releasing && pct >= FULL_CHARGE && "ready"]
      .filter(Boolean)
      .join(" "),
  );
  setStyle(/** @type {HTMLElement} */ (el.children[0]), "clipPath", `inset(0px ${100 - pct}% 0px 0px)`);
  // The Vaccinator charges in four 25% pips; the others in one bar.
  setHTML(el.children[1], g.kind === "vaccinator" ? [1, 2, 3].map(q => `<i class="seg q${q}"></i>`).join("") : "");
  setText2(el.children[2], chargeLabel(g, pct));
  setText2(
    el.children[3],
    (MEDIGUN_NAME[g.kind] || g.kind) + (g.kind === "vaccinator" && g.resist ? ` · ${g.resist}` : ""),
  );
  setText2(el.children[4], `${pct}%`);
  return el;
}
/** @param {Element} el @param {string} text */
function setText2(el, text) {
  if (el.textContent !== text) el.textContent = text;
}
/** @param {Element} box @param {Element[]} nodes */
function placeChildren(box, nodes) {
  // Only touch the DOM order when it changed: moving a node restarts its animations too.
  const same = box.children.length === nodes.length && nodes.every((n, i) => box.children[i] === n);
  if (!same) box.replaceChildren(...nodes);
}

/** @param {Player[]} players @param {Snapshot} snap */
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
/** @param {GameEvent} ev */
export function showTimeAdded(ev) {
  const el = document.createElement("div");
  el.className = `added ${$("scoreboard").classList.contains("koth") && (ev.team === "red" || ev.team === "blue") ? ev.team : ""}`;
  el.textContent = `+${clock(ev.seconds)}`;
  el.addEventListener("animationend", () => el.remove());
  $("scoreboard").append(el);
}

// The scoreboard while Tab is held: each team by points, like TF2's.
/** @type {[keyof PlayerScore, string][]} */
const BOARD_COLUMNS = [
  ["points", "Points"],
  ["kills", "K"],
  ["deaths", "D"],
  ["assists", "A"],
  ["damage", "Damage"],
  ["healing", "Healing"],
];
/** @param {Player[]} players @param {Snapshot} snap */
export function renderBoard(players, snap) {
  /** @param {"red" | "blue"} t @param {string} label */
  const team = (t, label) => {
    const rows = players
      .filter(p => p.team === t)
      .sort((a, b) => (b.score?.points ?? 0) - (a.score?.points ?? 0) || (b.score?.kills ?? 0) - (a.score?.kills ?? 0));
    const body = rows
      .map(p => {
        const s = /** @type {Partial<PlayerScore>} */ (p.score || {}),
          status = respawnIn(p) ? ` <small>${respawnIn(p)}s</small>` : "";
        const icon = ICONS[p.class] ? `<img src="${asset(`icons/${p.class}.svg`)}" alt="">` : "";
        return `<tr class="${p.alive ? "" : "dead"}"><td class="bc">${icon}</td><td class="bn">${esc(p.name)}${status}</td>${BOARD_COLUMNS.map(([k]) => `<td>${s[k] ?? ""}</td>`).join("")}<td>${p.ping ?? ""}</td></tr>`;
      })
      .join("");
    const score = /** @type {Partial<TeamScore>} */ (snap.score || {})[t] ?? 0;
    return (
      `<div class="bteam ${t}"><div class="bhead"><b>${label}</b><span>${score}</span><i>${rows.length} players</i></div>` +
      `<table><thead><tr><th></th><th class="bn">Name</th>${BOARD_COLUMNS.map(([, h]) => `<th>${h}</th>`).join("")}<th>Ping</th></tr></thead><tbody>${body}</tbody></table></div>`
    );
  };
  setHTML($("board"), team("blue", "BLU") + team("red", "RED"));
}

// ---------- TF2-style scoreboard: team scores, round timer, control points
/** @type {{ [round: string]: string }} */
const ROUND_LABEL = {
  pregame: "WARMUP",
  preround: "GET READY",
  team_win: "ROUND OVER",
  stalemate: "STALEMATE",
  game_over: "GAME OVER",
  bonus: "BONUS",
  between_rounds: "",
  restart: "",
  running: "",
};
/** @param {number} secs */
export function mmss(secs) {
  return `${Math.floor(secs / 60)}:${String(Math.floor(secs % 60)).padStart(2, "0")}`;
}
/** @param {string} id @param {string} text */
function setText(id, text) {
  const el = $(id);
  if (el.textContent !== text) el.textContent = text;
}
// CTF counts the round in captured intelligence, like TF2's CTF HUD, and pass time in goals;
// maps with a neutral flag (bball, special delivery) keep the team score. Robot and player
// destruction count points (other maps that reuse their logic count something else with it).
/** @param {Snapshot} snap */
export function shownScore(snap) {
  const objectives = snap.objectives || [];
  const ctf =
    snap.captures &&
    objectives.some(o => (o.kind === "flag" && (o.team === "red" || o.team === "blue")) || o.kind === "goal");
  const destruction = snap.points && /^(rd|pd)_/.test(state.map || "");
  if (destruction) return snap.points;
  return ctf ? snap.captures : snap.score;
}
// King of the hill: each team's timer, side by side in the middle like TF2's HUD; the one
// ticking (the team holding the point) bright, the other dimmed.
/** @param {(team: string) => Timer} byTeam @param {(t: Timer) => number} left */
function kothTimers(byTeam, left) {
  const running = ["blue", "red"].some(t => !byTeam(t).paused);
  return ["blue", "red"]
    .map(t => {
      const tm = byTeam(t),
        s = left(tm);
      return `<em class="kt ${t}${running && tm.paused ? " idle" : ""}${!tm.paused && s <= LOW_SECONDS ? " low" : ""}">${clock(s)}</em>`;
    })
    .join("");
}
/** @param {Timer | null} main @param {number | null} mapLeft @param {(t: Timer) => number} left */
function mainTimerText(main, mapLeft, left) {
  if (main) return clock(left(main));
  if (mapLeft != null) return clock(mapLeft);
  return "";
}
// Under the round timer, the map time; else in Mann vs. Machine the robots left in the wave, where
// the map time would go.
/** @param {Snapshot} snap @param {Timer | false | null | undefined} underRound @param {number | null} mapLeft */
function mapTimerText(snap, underRound, mapLeft) {
  const robots = snap.mvm
    ? /** @type {{ support?: boolean; count: number }[]} */ (snap.mvm.classes || [])
        .filter(c => !c.support)
        .reduce((n, c) => n + c.count, 0)
    : 0;
  if (underRound && mapLeft != null) return `MAP ${clock(mapLeft)}`;
  if (robots > 0) return `${robots} ROBOT${robots === 1 ? "" : "S"}`;
  return "";
}
// Events are stamped with time into the round, which (unlike the round timer) never jumps.
/** @param {Snapshot} snap @param {number} tick */
function timeIntoRound(snap, tick) {
  if (snap.round_time == null) return null;
  return snap.round_time + (snap.round === "running" ? (Math.max(0, tick - snap.tick) * msPerTick) / 1000 : 0);
}
/** @param {Snapshot} snap @param {Timer | null} main */
function roundLabel(snap, main) {
  if (main?.setup) return "SETUP";
  // Mann vs. Machine: the wave, like its HUD.
  if (snap.mvm) return `WAVE ${snap.mvm.wave} / ${snap.mvm.waves}`;
  return ROUND_LABEL[snap.round] ?? "";
}
// The time the low-time warning watches: a running round timer, or the map time when there is none.
/** @param {Timer | null} main @param {number | null} mapLeft @param {(t: Timer) => number} left */
function watchedTimeLeft(main, mapLeft, left) {
  if (main && !main.paused) return left(main);
  if (!main) return mapLeft;
  return null;
}

/** @param {Snapshot} snap */
function renderScores(snap) {
  $("scoreboard").hidden = OVERLAY && OPT.get("score") !== "1";
  const scores = shownScore(snap);
  setText("score-blue", String(scores?.blue ?? 0));
  setText("score-red", String(scores?.red ?? 0));
}
/** @param {Snapshot} snap @param {number} tick */
export function renderScoreboard(snap, tick) {
  renderScores(snap);
  const t = timersOf(snap, tick);
  // Like TF2's HUD: the round timer when one is active, else the map's mp_timelimit; with
  // both, the map time goes underneath.
  if (t.koth) setHTML($("timer-main"), kothTimers(/** @type {(team: string) => Timer} */ (t.byTeam), t.left));
  else setText("timer-main", mainTimerText(t.main, t.mapLeft, t.left));
  $("scoreboard").classList.toggle("koth", Boolean(t.koth));
  setText("timer-map", mapTimerText(snap, t.main || t.koth, t.mapLeft));
  const into = timeIntoRound(snap, tick);
  state.clock = into == null ? "" : mmss(into);
  const side = /** @param {string} team */ team => {
    const timer = t.byTeam(team);
    return !t.koth && timer ? clock(t.left(timer)) : "";
  };
  setText("timer-blue", side("blue"));
  setText("timer-red", side("red"));
  renderRoundLabel(snap, t);
}
// The snapshot's timers, run on to `tick`: the main one, each team's, and the map's time left.
/** @param {Snapshot} snap @param {number} tick */
function timersOf(snap, tick) {
  const timers = snap.timers || [];
  const since = (Math.max(0, tick - snap.tick) * msPerTick) / 1000;
  const left = /** @param {Timer} t */ t => (t.paused ? t.remaining : t.remaining - since);
  const main = timers.find(t => t.team === "none") || (timers.length === 1 ? timers[0] : null);
  const byTeam = /** @param {string} team */ team => timers.find(t => t.team === team && t !== main);
  const mapLeft = snap.time_left == null ? null : snap.time_left - since;
  const koth = !main && byTeam("blue") && byTeam("red");
  return { main, byTeam, left, mapLeft, koth };
}
// The round's label, OVERTIME once its timer ran out, and the timer turning red near the end.
/** @param {Snapshot} snap @param {ReturnType<typeof timersOf>} t */
function renderRoundLabel(snap, { main, mapLeft, left }) {
  const label = roundLabel(snap, main);
  // A paused round timer is not a paused game (KotH, no match started): it just stands still.
  setText("round-label", label || (main && !main.paused && left(main) <= 0 ? "OVERTIME" : ""));
  const shown = watchedTimeLeft(main, mapLeft, left);
  $("timer-main").parentElement?.classList.toggle("low", shown != null && shown <= LOW_SECONDS);
}
/** @param {Snapshot} snap */
function renderPoints(snap) {
  const html = (snap.objectives || [])
    .filter(o => o.kind === "control_point")
    .map(o => {
      const capping = o.capping_team !== "none" && o.cap_progress > 0;
      const style = `--own:${teamColour(o.owner, "#8a8a8a")};--cap:${capping ? TEAM[o.capping_team] : "transparent"};--p:${Math.round((capping ? o.cap_progress : 0) * 100)}%`;
      // Like TF2's HUD: blank, but the cap rate (×2 for two cappers) while it is being capped.
      const n = capping && o.on_point ? o.on_point[o.capping_team] : 0;
      return `<div class="cp${o.locked ? " locked" : ""}${capping ? " capping" : ""}" style="${style}">${n > 0 ? `\u00d7${n}` : ""}</div>`;
    })
    .join("");
  if ($("cps").innerHTML !== html) $("cps").innerHTML = html;
}
