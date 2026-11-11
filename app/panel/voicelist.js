// The speakers list's content (see panel/speakers.js): voice off, mute all and swap the teams
// heard; then RED and BLU with everyone on them now, heard or not, and the rest heard; each team
// with its mute, each person with theirs and a volume. Who talks is lit, and so is their team's
// name.
import { ICONS, state } from "../core/core.js";
import { asset, env } from "../core/env.js";
import * as voice from "../core/voice.js";

// The teams in the list, in order, and how they are named.
const TEAMS = [
  { team: "red", name: "RED" },
  { team: "blue", name: "BLU" },
  { team: "", name: "Others" },
];

/** @param {string} text @param {Record<string, string>} data @param {string} [title] */
function button(text, data, title = "") {
  const b = Object.assign(document.createElement("button"), { textContent: text, title });
  Object.assign(b.dataset, data);
  return b;
}

// A person's mute: their class icon as on the HUD, on their team's colour (faded and struck out
// while muted); a 🔊 or 🔇 for one with no class.
/** @param {string} steamid @param {import("../../types/stream").Player | undefined} p @param {boolean} muted */
function muteButton(steamid, p, muted) {
  const b = button(muted ? "🔇" : "🔊", { mute: steamid }, muted ? "Unmute" : "Mute");
  if (p && ICONS[p.class]) {
    b.textContent = "";
    b.className = `vci t-${p.team}`;
    b.append(Object.assign(document.createElement("img"), { src: asset(`icons/${p.class}.svg`), alt: p.class }));
  }
  b.classList.toggle("muted", muted);
  return b;
}

/** @param {Person} s */
function row(s) {
  const muted = voice.isMuted(s.steamid);
  const p = s.player == null ? undefined : state.players.get(s.player);
  const name = p?.name ?? s.name ?? s.steamid;
  const el = document.createElement("div");
  el.className = "vrow";
  el.dataset.steamid = s.steamid;
  const label = Object.assign(document.createElement("span"), { className: "vname", textContent: name });
  const range = Object.assign(document.createElement("input"), { type: "range", min: "0", max: "100" });
  range.value = String(Math.round(voice.volumeOf(s.steamid) * 100));
  range.dataset.volume = s.steamid;
  range.setAttribute("aria-label", `Volume of ${name}`);
  el.append(muteButton(s.steamid, p, muted), label, range);
  return el;
}

/** @typedef {{ steamid: string; player: number | null; name?: string | null }} Person */

// Where each player's HUD card is, top to bottom: the list keeps the HUD's order.
function hudOrder() {
  /** @type {Map<number, number>} */
  const at = new Map();
  for (const c of /** @type {NodeListOf<HTMLElement>} */ (env.root.querySelectorAll("#hud-red .card, #hud-blue .card")))
    at.set(Number(c.dataset.id), at.size);
  return at;
}

// Who is listed, by team: RED's and BLU's players now (bots aside) as the HUD has them, then the
// rest heard: under a team those taken for it (a coach in its Discord), else under Others.
/** @returns {{ team: string; name: string; people: Person[] }[]} */
function groups() {
  /** @type {Map<string, Person[]>} */
  const by = new Map(TEAMS.map(t => [t.team, []]));
  const order = hudOrder();
  const players = [...state.players.values()].sort(
    (a, b) => (order.get(a.id) ?? Infinity) - (order.get(b.id) ?? Infinity),
  );
  for (const p of players)
    if ((p.team === "red" || p.team === "blue") && p.steamid && p.steamid !== "BOT")
      by.get(p.team)?.push({ steamid: p.steamid, player: p.id });
  const listed = new Set([...by.values()].flat().map(p => p.steamid));
  for (const s of voice.speakersHeard()) {
    const team = voice.teamOf(s.steamid);
    if (!listed.has(s.steamid)) by.get(team === "red" || team === "blue" ? team : "")?.push(s);
  }
  return TEAMS.map(t => ({ ...t, people: by.get(t.team) ?? [] }));
}

// What the list shows, to know when to build it anew.
/** @param {ReturnType<typeof groups>} gs */
const shape = gs =>
  gs
    .map(
      g =>
        `${g.team}:${g.people.map(p => `${p.steamid}/${p.player == null ? (p.name ?? "") : state.players.get(p.player)?.class}`).join(",")}`,
    )
    .join("|");

// More for the end of the list: the Discord invite (voiceinvite.js), and for a room's admins the
// voices from outside to match (room/voiceadmin.js).
/** @type {(() => HTMLElement[])[]} */
const extras = [];
/** @param {() => HTMLElement[]} more */
export const addListExtra = more => void extras.push(more);

/** @param {HTMLElement} list */
export function renderList(list) {
  // Not under a slider being dragged or a choice being made: it would come loose. Once let go,
  // the list catches up.
  if (list.querySelector("input[type=range]:active, select:focus, input[type=number]:focus"))
    return void (list.dataset.stale = "1");
  delete list.dataset.stale;
  const gs = groups();
  list.dataset.shape = shape(gs);
  const head = document.createElement("div");
  head.className = "vhead";
  head.append(button("Voice off", { act: "off" }));
  head.append(button(voice.allMuted() ? "Unmute all" : "Mute all", { act: "all" }));
  head.append(button("⇄ Swap", { act: "swap" }, "Hear the other team, mute this one"));
  /** @type {HTMLElement[]} */
  const parts = [head];
  for (const { team, name, people } of gs) {
    // The teams always, to mute one before anyone on it talks; the rest once heard.
    if (!people.length && !team) continue;
    const group = document.createElement("div");
    group.className = `vteam t-${team || "none"}`;
    group.dataset.team = team;
    group.append(Object.assign(document.createElement("span"), { className: "vtname", textContent: name }));
    if (team) {
      const muted = voice.isTeamMuted(team);
      group.append(button(muted ? "Unmute team" : "Mute team", { team }));
    }
    parts.push(group, ...people.map(row));
  }
  list.replaceChildren(...parts, ...extras.flatMap(more => more()));
  markTalking(list);
}

// Lights who talks, and their team's name; the list anew when who is in it changed.
/** @param {HTMLElement} list */
export function markTalking(list) {
  const heard = voice.speakersHeard();
  if (list.dataset.stale || list.dataset.shape !== shape(groups())) renderList(list);
  /** @type {Set<string>} */
  const teams = new Set();
  for (const s of heard) {
    list.querySelector(`.vrow[data-steamid="${CSS.escape(s.steamid)}"]`)?.classList.toggle("talking", s.talking);
    if (s.talking) teams.add(voice.teamOf(s.steamid) ?? "");
  }
  for (const g of /** @type {NodeListOf<HTMLElement>} */ (list.querySelectorAll(".vteam")))
    g.classList.toggle(
      "talking",
      teams.has(g.dataset.team ?? "") || (!g.dataset.team && [...teams].some(t => !["red", "blue"].includes(t))),
    );
}

// What a click in the list does; whether it turned voice off.
/** @param {Element} at */
export function chooseIn(at) {
  const b = /** @type {HTMLButtonElement | null} */ (at.closest("button"));
  if (!b) return false;
  const { act, team, mute } = b.dataset;
  if (act === "off") {
    voice.setVoice(false);
    return true;
  }
  if (act === "all") voice.muteEveryone(!voice.allMuted());
  else if (act === "swap") voice.swapTeams();
  else if (team) voice.setTeamMuted(team, !voice.isTeamMuted(team));
  else if (mute) voice.toggleMuted(mute);
  return false;
}
