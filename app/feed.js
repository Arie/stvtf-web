import { msPerTick, TEAM, $, state } from "./core.js";
import { MEDIGUN_NAME, mmss } from "./hud.js";
import { layerOn } from "./layers.js";

export function esc(s) { return String(s ?? "").replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])); }
function who(pl) { return pl ? `<span class="t-${esc(pl.team)}">${esc(pl.name)}</span>` : "world"; }
// Kill icons from tf2-icons, named like the weapons in server logs and kill events: where each is
// in the sprite.
export const KILLICONS = new Map();
export const killiconsLoaded = fetch("killicons.css").then(r => r.text()).then(css => {
  for (const [, name, rule] of css.matchAll(/\.killicon-([A-Za-z0-9_]+)\s*\{([^}]*)\}/g)) {
    const px = prop => [...(rule.match(new RegExp(`${prop}:([^;]*)`))?.[1] ?? "").matchAll(/-?\d+/g)].map(Number);
    const [w] = px("width"), [h] = px("height"), [x = 0, y = 0] = px("background-position");
    KILLICONS.set(name, { x: -x, y: -y, w, h });
  }
});
const killIcon = ev => {
  const w = ev.weapon || "";
  return [ev.headshot && `${w}_headshot`, ev.headshot && "headshot", ev.backstab && `${w}_backstab`, ev.backstab && "backstab", w]
    .find(n => n && KILLICONS.has(n)) || null;
};
function weaponIcon(ev, icon) {
  const crit = ev.crit ? " crit" : "";
  return icon ? `<span class="killicon killicon-${icon}${crit}" title="${esc(ev.weapon)}"></span>` : `<span class="w">${esc(ev.weapon)}</span>`;
}
const BUILDING = { sentry: "Sentry", dispenser: "Dispenser", teleporter: "Teleporter", sapper: "Sapper" };
const BANNER = { buff_banner: "Buff Banner", battalions_backup: "Battalion's Backup", concheror: "Concheror" };
const PROJECTILE_WEAPON = /rocket|quake_rl|directhit|liberty|blackbox|airstrike|mangler|dumpster|pipe|bomb|loch|cannon|resistance|huntsman|arrow|crossbow|flare|detonator|scorch/;
export const teamName = t => t === "red" ? "RED" : t === "blue" ? "BLU" : "";
export const pointName = ev => ev.name && !ev.name.startsWith("#") ? ev.name : "point " + (ev.point + 1);
// A team's flag is its intelligence (Mann vs. Machine's bomb is the robots'); a neutral one is special delivery's Australium, player
// destruction's pickups (each player carries their own) or a ball.
export const flagName = team => state.map?.startsWith("mvm_") ? "the bomb"
  : team === "red" || team === "blue" ? `the ${teamName(team)} intelligence`
  : state.map?.startsWith("sd_") ? "the Australium" : state.map?.startsWith("pd_") ? "pickups" : "the ball";
const chip = (cls, text) => `<span class="chip ${cls}">${text}</span>`;

export const CHAT_TYPES = new Set(["chat", "voice_command", "server_text"]);
const VOICE = [
  ["MEDIC!", "Thanks!", "Go! Go! Go!", "Move up!", "Go left!", "Go right!", "Yes", "No"],
  ["Incoming!", "Spy!", "Sentry ahead!", "Teleporter here", "Dispenser here", "Sentry here", "Activate charge!", "ÜberCharge ready!"],
  ["Help!", "Battle cry", "Cheers", "Jeers", "Positive", "Negative", "Nice shot", "Good job"],
];
const ANNOUNCER = {
  "Announcer.RoundEnds5minutes": "5 minutes remain", "Announcer.RoundEnds2minutes": "2 minutes remain",
  "Announcer.RoundEnds60seconds": "1 minute remains", "Announcer.RoundEnds30seconds": "30 seconds remain",
  "Announcer.RoundEnds20seconds": "20 seconds remain", "Announcer.RoundEnds10seconds": "10 seconds remain",
  "Announcer.RoundEnds5seconds": "5 seconds remain", "Announcer.RoundEnds4seconds": "4 seconds remain",
  "Announcer.RoundEnds3seconds": "3 seconds remain", "Announcer.RoundEnds2seconds": "2 seconds remain",
  "Announcer.RoundEnds1seconds": "1 second remains"
};
let voteOptions = [];
// What a call vote is about, from its localisation token and argument.
const VOTE_ISSUE = {
  "#TF_vote_nextlevel": d => `next map ${d}`, "#TF_vote_changelevel": d => `change the map to ${d}`,
  "#TF_vote_kick_player_other": d => `kick ${d}`, "#TF_vote_kick_player_idle": d => `kick ${d} (idle)`,
  "#TF_vote_kick_player_cheating": d => `kick ${d} (cheating)`, "#TF_vote_kick_player_scamming": d => `kick ${d} (scamming)`,
  "#TF_vote_restart_game": () => "restart the game", "#TF_vote_scramble_teams": () => "scramble the teams",
  "#TF_vote_extendlevel": () => "extend the map", "#TF_vote_td_start_round": () => "start the round",
};
const voteIssue = ev => (Object.hasOwn(VOTE_ISSUE, ev.issue) ? VOTE_ISSUE[ev.issue] : d => [String(ev.issue ?? "").replace(/^#/, ""), d].filter(Boolean).join(" "))(ev.details || "");

// Kills by one player within 10 s of each other count as a streak: 2K, 3K, …
const STREAK_MS = 10000;
const streaks = new Map();
function streak(ev) {
  const s = streaks.get(ev.attacker), gap = s ? ev.tick - s.tick : -1;
  const n = gap >= 0 && gap * msPerTick <= STREAK_MS ? s.n + 1 : 1;
  streaks.set(ev.attacker, { tick: ev.tick, n });
  return n;
}
// A kill's icon, streak and airshot, worked out once for the feed and the embedding page.
const killed = new WeakMap();
export function killDetails(ev, p) {
  if (!killed.has(ev)) {
    const by = ev.attacker != null && ev.attacker !== ev.victim && p(ev.attacker);
    killed.set(ev, { icon: killIcon(ev), streak: by && !ev.feign ? streak(ev) : 0, airshot: Boolean(ev.airborne && PROJECTILE_WEAPON.test(ev.weapon || "")) });
  }
  return killed.get(ev);
}

// One row per event, newest on top, stamped with the round clock.
export function feedEvent(ev, p) {
  // Players who have since left are no longer known by id.
  const name = id => (id == null ? "" : p(id) ? `<span class="who" data-id="${Number(id)}">${who(p(id))}</span>` : '<span class="who"><span class="w">?</span></span>');
  let cls = "", html = null;
  switch (ev.type) {
    case "kill": {
      const a = ev.attacker != null && ev.attacker !== ev.victim ? p(ev.attacker) : null;
      const k = killDetails(ev, p), n = k.streak;
      if (n >= 2) cls += ` streak s${Math.min(n, 4)}`;
      const tags = [n >= 2 && chip(`s${Math.min(n, 4)}`, `${n}K`), ev.dominated && chip("dom", "DOMINATING"), ev.revenge && chip("rev", "REVENGE"),
        k.airshot && chip("air", "AIRSHOT"), ev.feign && chip("", "FEIGN")].filter(Boolean).join("");
      cls += ev.feign ? " feign" : "";
      html = `${a ? name(ev.attacker) : ""}${ev.assister != null ? "+" + name(ev.assister) : ""}${weaponIcon(ev, k.icon)}${name(ev.victim)}${tags}`;
      break;
    }
    case "uber": {
      const m = p(ev.medic);
      cls = `hot ${m?.team || ""}`;
      html = `${name(ev.medic)}${chip("", "ÜBER")}<span class="w">${esc(MEDIGUN_NAME[ev.medigun] || "")}</span>${ev.target != null ? " → " + name(ev.target) : ""}`;
      break;
    }
    case "uber_ready": cls = "minor"; html = `${name(ev.medic)} <span>${esc(MEDIGUN_NAME[ev.medigun] || "Medi Gun")} ÜberCharge ready</span>`; break;
    case "medic_death":
      if (!ev.drop) return;
      cls = `hot ${p(ev.attacker)?.team || ""}`;
      html = `${name(ev.attacker)}${chip("drop", "DROP")}${name(ev.medic)}<span class="w">died with full über</span>`;
      break;
    case "capture_blocked": html = `${name(ev.blocker)}<span class="w">defended ${esc(pointName(ev))}</span>`; break;
    case "building_destroyed":
      html = `${ev.attacker != null ? name(ev.attacker) : ""}<span class="w">destroyed</span>${name(ev.owner)}<span class="w">'s ${BUILDING[ev.object] || esc(ev.object)}</span>`;
      break;
    case "banner": cls = "minor"; html = `${name(ev.player)} <span>deployed the ${BANNER[ev.kind] || "banner"}</span>`; break;
    case "point_captured": cls = `big ${ev.team}`; html = `${teamName(ev.team)} captured ${esc(pointName(ev))}`; break;
    case "flag": {
      const what = esc(flagName(ev.team));
      if (ev.event === "captured") { cls = `big ${p(ev.player)?.team || "none"}`; html = `${name(ev.player)} <span>captured ${what}</span>`; }
      else if (ev.event === "defended") html = `${name(ev.player)}<span class="w">stopped</span>${name(ev.carrier)}<span class="w">with ${what}</span>`;
      // Player destruction: players pick up the dropped pickups all the time.
      else if (ev.event === "pickup" && state.map?.startsWith("pd_")) { if (!layerOn("flow")) return; cls = "minor"; html = `${name(ev.player)} <span>collected ${what}</span>`; }
      else if (ev.event === "pickup") { cls = "minor"; html = `${name(ev.player)} <span>picked up ${what}</span>`; }
      else if (ev.event === "dropped") { cls = "minor"; html = `${name(ev.player)} <span>dropped ${what}</span>`; }
      else if (ev.event === "returned") { cls = "minor"; html = `<span>${what[0].toUpperCase() + what.slice(1)} went home</span>`; }
      break;
    }
    case "vote_options":
      if (!ev.options?.length) return;
      voteOptions = ev.options;
      if (!layerOn("flow")) return;
      cls = "minor"; html = `<span>Vote: ${ev.options.map(esc).join(" / ")}</span>`;
      break;
    case "vote_cast": {
      if (!layerOn("flow")) return;
      const o = voteOptions[ev.option] ?? ["Yes", "No"][ev.option];
      if (o == null) return;
      cls = "minor"; html = `${name(ev.player)} <span>voted ${esc(o)}</span>`;
      break;
    }
    case "killed_capper":
      if (!layerOn("flow")) return;
      html = `${name(ev.killer)}${ev.assister != null ? "+" + name(ev.assister) : ""}<span class="w">killed capper</span>${name(ev.victim)}`;
      break;
    case "medic_defended": if (!layerOn("flow")) return; html = `${name(ev.player)}<span class="w">defended medic</span>${name(ev.medic)}`; break;
    case "tournament": if (!layerOn("flow") || ev.ready == null) return; cls = "minor"; html = `${name(ev.player)} <span>is ${ev.ready ? "ready" : "not ready"}</span>`; break;
    case "announcer":
      if (!layerOn("flow") || !ANNOUNCER[ev.sound]) return;
      cls = `minor ${ev.team === "red" || ev.team === "blue" ? ev.team : ""}`; html = `<span>${ANNOUNCER[ev.sound]}</span>`;
      break;
    case "time_remaining": if (!layerOn("flow") || !(ev.seconds > 0)) return; cls = "minor"; html = `<span>${mmss(ev.seconds)} remaining</span>`; break;
    case "pass_score": {
      cls = `big ${p(ev.scorer)?.team || "none"}`;
      html = `${name(ev.scorer)}${ev.assister != null ? "+" + name(ev.assister) : ""} <span>scored${ev.points > 1 ? ` ${ev.points} points` : ""}</span>`;
      break;
    }
    case "pass_stolen": html = `${name(ev.attacker)}<span class="w">stole the ball from</span>${name(ev.victim)}`; break;
    case "pass_blocked": cls = "minor"; html = `${name(ev.blocker)}<span class="w">blocked a pass from</span>${name(ev.owner)}`; break;
    case "ball_carrier_killed":
      if (!layerOn("flow")) return;
      html = `${name(ev.attacker)}${ev.assister != null ? "+" + name(ev.assister) : ""}<span class="w">killed the ball carrier</span>${name(ev.victim)}`;
      break;
    case "robot_killed": cls = "minor"; html = ev.attacker != null ? `${name(ev.attacker)} <span>destroyed a robot</span>` : "<span>A robot was destroyed</span>"; break;
    case "special_score": cls = p(ev.player)?.team || ""; html = `${name(ev.player)} <span>scored</span>`; break;
    case "team_leader_killed": html = `${name(ev.killer)}<span class="w">killed team leader</span>${name(ev.victim)}`; break;
    case "rocket_open": cls = `big ${ev.team}`; html = `${teamName(ev.team)} opened the rocket`; break;
    case "point_locked": if (!layerOn("flow")) return; cls = "minor"; html = `<span>${esc(pointName(ev))} ${ev.locked ? "locked" : "unlocked"}</span>`; break;
    case "sapped": cls = "minor"; html = `${name(ev.player)}<span class="w">sapped</span>${name(ev.owner)}<span class="w">'s ${BUILDING[ev.object] || esc(ev.object)}</span>`; break;
    case "vote_started":
      if (!layerOn("flow")) return;
      cls = "minor"; html = `${ev.caller != null ? name(ev.caller) + " " : ""}<span>called a vote: ${esc(voteIssue(ev))}</span>`;
      break;
    case "vote_passed": if (!layerOn("flow")) return; cls = "minor"; html = "<span>Vote passed</span>"; break;
    case "vote_failed": if (!layerOn("flow")) return; cls = "minor"; html = "<span>Vote failed</span>"; break;
    case "round_start": cls = "big none"; html = "Round start"; break;
    case "round_win": cls = `big ${ev.team}`; html = `${teamName(ev.team)} wins the round${state.lastScores ? ` · BLU ${state.lastScores.blue} – ${state.lastScores.red} RED` : ""}`; break;
    case "scores": state.lastScores = ev; if (ev.game_over) { cls = "big none"; html = `Match over · BLU ${ev.blue} – ${ev.red} RED`; } break;
    case "stalemate": cls = "big none"; html = "Stalemate"; break;
    case "overtime": cls = "big none"; html = "Overtime"; break;
    case "game_over": cls = "big none"; html = "Game over"; break;
    case "map": cls = "big none"; html = `Map change → ${esc(ev.map)}`; break;
    case "player_leave": cls = "minor"; html = `<span class="who">${esc(ev.name || "")}</span> <span>left</span>`; break;
    case "team_change":
      if (ev.disconnect || (ev.team !== "red" && ev.team !== "blue") || ev.old_team === "red" || ev.old_team === "blue") return;
      cls = "minor"; html = `${name(ev.player)} <span>joined ${teamName(ev.team)}</span>`;
      break;
  }
  if (html == null) return;
  const d = document.createElement("div");
  d.className = `ev ${cls}`;
  const stamp = ev.at != null ? mmss(ev.at) : state.clock;
  d.innerHTML = `${stamp ? `<time>${stamp}</time>` : ""}${html}`;
  prepend($("feed"), d);
}
export function chat(ev, p) {
  const d = document.createElement("div");
  const pl = ev.player != null ? p(ev.player) : null;
  const stamp = ev.at != null ? mmss(ev.at) : state.clock;
  const time = stamp ? `<time>${stamp}</time>` : "";
  if (ev.type === "voice_command") {
    const text = VOICE[ev.menu]?.[ev.item];
    if (!layerOn("flow") || !pl || !text) return;
    d.className = "voice";
    d.innerHTML = `${time}${who(pl)} <span class="w">🔊 ${esc(text)}</span>`;
  } else if (ev.type === "server_text") {
    if (!layerOn("flow") || !ev.text || ev.text.startsWith("#")) return;
    d.className = "srvtext";
    d.innerHTML = time + esc(ev.text);
  } else if (pl || ev.name) {
    d.innerHTML = `${time}${pl ? who(pl) : `<span class="w">${esc(ev.name)}</span>`}: ${coloured(ev, pl)}`;
  } else {
    // Server messages (plugins, rcon say) as notices; `say` from the console adds its own prefix.
    d.className = "notice";
    d.innerHTML = time + coloured(ev, pl, /^Console: /);
  }
  prepend($("chat"), d);
}
// A chat line in the colours its sender gave it (TF2's colour codes, as plugins use them):
// `team` is the sender's team colour, null the line's own colour.
function coloured(ev, pl, drop) {
  if (!ev.parts?.length) return esc(String(ev.text ?? "").replace(drop ?? /$^/, ""));
  return ev.parts.map(({ c, t }, i) => {
    const text = esc(i === 0 && drop ? t.replace(drop, "") : t);
    if (c === "team") return pl ? `<span class="t-${esc(pl.team)}">${text}</span>` : text;
    return /^[0-9A-F]{6}$/.test(c ?? "") ? `<span style="color:#${c}">${text}</span>` : text;
  }).join("");
}
// Rows kept in the feed and the chat, and how long a banner shows.
const ROWS_KEPT = 60, BANNER_MS = 3500;
function prepend(box, el) { box.prepend(el); while (box.children.length > ROWS_KEPT) box.lastChild.remove(); }
let bannerTimer;
export function banner(text, team) {
  const b = $("banner"); b.textContent = text; b.style.color = TEAM[team] || "#fff"; b.style.opacity = 1;
  clearTimeout(bannerTimer); bannerTimer = setTimeout(() => b.style.opacity = 0, BANNER_MS);
}
