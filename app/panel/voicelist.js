// The speakers list's content (see panel/speakers.js): voice off, mute all and swap the teams
// heard; then everyone heard, by team (RED, BLU, the rest), each team with its mute, each speaker
// with theirs and a volume. Who talks is lit, and so is their team's name.
import { state } from "../core/core.js";
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

/** @param {{ steamid: string; player: number | null }} s */
function row(s) {
  const muted = voice.isMuted(s.steamid);
  const name = (s.player == null ? null : state.players.get(s.player)?.name) ?? s.steamid;
  const el = document.createElement("div");
  el.className = "vrow";
  el.dataset.steamid = s.steamid;
  const label = Object.assign(document.createElement("span"), { className: "vname", textContent: name });
  const range = Object.assign(document.createElement("input"), { type: "range", min: "0", max: "100" });
  range.value = String(Math.round(voice.volumeOf(s.steamid) * 100));
  range.dataset.volume = s.steamid;
  range.setAttribute("aria-label", `Volume of ${name}`);
  el.append(button(muted ? "🔇" : "🔊", { mute: s.steamid }, muted ? "Unmute" : "Mute"), label, range);
  return el;
}

/** @param {HTMLElement} list */
export function renderList(list) {
  // Not under a slider being dragged: it would come loose. Once let go, the list catches up.
  if (list.querySelector("input[type=range]:active")) return void (list.dataset.stale = "1");
  delete list.dataset.stale;
  const heard = voice.speakersHeard();
  const head = document.createElement("div");
  head.className = "vhead";
  head.append(button("Voice off", { act: "off" }));
  const anyHeard =
    heard.some(s => !voice.isMuted(s.steamid)) || !voice.isTeamMuted("red") || !voice.isTeamMuted("blue");
  head.append(button(anyHeard ? "Mute all" : "Unmute all", { act: "all" }));
  head.append(button("⇄ Swap", { act: "swap" }, "Hear the other team, mute this one"));
  /** @type {HTMLElement[]} */
  const parts = [head];
  for (const { team, name } of TEAMS) {
    const of = heard.filter(
      s =>
        (voice.teamOf(s.steamid) ?? "") === team || (!team && !["red", "blue"].includes(voice.teamOf(s.steamid) ?? "")),
    );
    // The teams always, to mute one before anyone on it talks; the rest once heard.
    if (!of.length && !team) continue;
    const group = document.createElement("div");
    group.className = `vteam t-${team || "none"}`;
    group.dataset.team = team;
    group.append(Object.assign(document.createElement("span"), { className: "vtname", textContent: name }));
    if (team) {
      const muted = voice.isTeamMuted(team);
      group.append(button(muted ? "Unmute team" : "Mute team", { team }));
    }
    parts.push(group, ...of.map(row));
  }
  list.replaceChildren(...parts);
  markTalking(list);
}

// Lights who talks, and their team's name; anew when someone new was heard.
/** @param {HTMLElement} list */
export function markTalking(list) {
  const heard = voice.speakersHeard();
  if (list.dataset.stale || list.querySelectorAll(".vrow").length !== heard.length) renderList(list);
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
  if (act === "all") voice.muteEveryone(voice.speakersHeard().some(s => !voice.isMuted(s.steamid)));
  else if (act === "swap") voice.swapTeams();
  else if (team) voice.setTeamMuted(team, !voice.isTeamMuted(team));
  else if (mute) voice.toggleMuted(mute);
  return false;
}
