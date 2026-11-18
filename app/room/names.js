// A room's names, for its admins: each player's and team's, filled in with a league's where the
// leagues know them, to adjust and apply for everyone (see core/names.js).
import { $, byClass, state } from "../core/core.js";
import { env } from "../core/env.js";
import { given } from "../core/names.js";
import { el } from "../panel/voicebits.js";
import { post } from "./home.js";
import { note } from "./tools.js";

/** @typedef {{ league: string; players: Record<string, string>; teams: Record<string, string> }} League */
/** @typedef {import("../../types/stream").Player} Player */

const SIDES = /** @type {const} */ ([
  ["blue", "BLU"],
  ["red", "RED"],
]);

/** @type {League[]} */
let leagues = [];

// The match's players now, bots aside, as the HUD orders them.
const players = () =>
  [...state.players.values()]
    .filter(p => (p.team === "red" || p.team === "blue") && /^\[U:1:\d+\]$/.test(p.steamid))
    .sort(byClass);

/** @param {string} key @param {string} value @param {string} hint */
function field(key, value, hint) {
  const input = el("input", { value, placeholder: hint, maxLength: 32, spellcheck: false });
  input.dataset.name = key;
  return input;
}

// The panel: a column a side, its team's name over its players'.
/** @param {HTMLElement} panel */
function show(panel) {
  const all = players();
  const columns = SIDES.map(([side, label]) =>
    el(
      "div",
      { className: `nteam ${side}` },
      field(`team:${side}`, given.teams[side] ?? "", label),
      ...all
        .filter(p => p.team === side)
        .map(p =>
          el(
            "label",
            {},
            el("span", {}, p.own ?? p.name),
            field(p.steamid, given.players[p.steamid] ?? "", p.own ?? p.name),
          ),
        ),
    ),
  );
  const pick = el("select", { hidden: true, ariaLabel: "League" });
  pick.onchange = () => fill(panel, leagues[Number(pick.value)], true);
  const apply = el("button", { className: "napply" }, "Apply");
  apply.onclick = () => send(panel, read(panel));
  const reset = el("button", {}, "Game names");
  reset.onclick = () => send(panel, { players: {}, teams: {} });
  panel.replaceChildren(
    el("div", { className: "nhead" }, pick, apply, reset),
    el("div", { className: "ncols" }, ...columns),
  );
  ask(panel, pick, all);
}

// Asks how the leagues know the players, and fills in what is still empty with the one knowing most.
/** @param {HTMLElement} panel @param {HTMLSelectElement} pick @param {Player[]} all */
function ask(panel, pick, all) {
  post("leagues", { players: all.map(p => ({ steamid: p.steamid, team: p.team })) })
    .then(r => (r.ok ? r.json() : { leagues: [] }))
    .then(
      said => {
        leagues = said.leagues ?? [];
        if (!leagues.length || !pick.isConnected) return;
        pick.replaceChildren(...leagues.map((l, i) => el("option", { value: String(i) }, l.league)));
        pick.hidden = false;
        fill(panel, leagues[0], false);
      },
      () => {},
    );
}

// Fills the panel in with `league`'s names: only where nothing is written, or `over` what is.
/** @param {HTMLElement} panel @param {League} league @param {boolean} over */
function fill(panel, league, over) {
  for (const input of /** @type {NodeListOf<HTMLInputElement>} */ (panel.querySelectorAll("input[data-name]"))) {
    const key = input.dataset.name ?? "";
    const name = key.startsWith("team:") ? league.teams[key.slice(5)] : league.players[key];
    if (over ? name !== undefined || input.value : !input.value) input.value = name ?? (over ? "" : input.value);
  }
}

/** @param {HTMLElement} panel */
function read(panel) {
  /** @type {{ players: Record<string, string>; teams: Record<string, string> }} */
  const names = { players: {}, teams: {} };
  for (const input of /** @type {NodeListOf<HTMLInputElement>} */ (panel.querySelectorAll("input[data-name]"))) {
    const key = input.dataset.name ?? "",
      name = input.value.trim();
    if (!name || name === input.placeholder) continue;
    if (key.startsWith("team:")) names.teams[key.slice(5)] = name;
    else names.players[key] = name;
  }
  return names;
}

/** @param {HTMLElement} panel @param {object} names */
function send(panel, names) {
  post("names", names).then(async r => {
    if (r.ok) panel.hidden = true;
    else note(await r.text());
  });
}

// The names button of the room bar opens the panel, and closes it.
/** @param {HTMLButtonElement} button */
export function namesTool(button) {
  const panel = $("names");
  button.onclick = () => {
    panel.hidden = !panel.hidden;
    if (!panel.hidden) show(panel);
  };
  env.keys.addEventListener("keydown", e => {
    if (e.key === "Escape") panel.hidden = true;
  });
  // Typing in it is no shortcut of the page's.
  panel.addEventListener("keydown", e => e.key !== "Escape" && e.stopPropagation());
}
