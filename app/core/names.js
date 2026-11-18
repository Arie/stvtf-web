// The names a room's admins gave the match's players and teams, in place of the game's.
import { $, state } from "./core.js";
/** @typedef {import("../../types/stream").NamesLine} NamesLine */

/** @type {{ players: Record<string, string>; teams: Record<string, string> }} */
export const given = { players: {}, teams: {} };

// `p` as the room names them; the game's own name is kept as `own`.
/** @template {{ name: string; own?: string; steamid?: string | null }} P @param {P} p @returns {P} */
export function named(p) {
  p.own ??= p.name;
  p.name = (p.steamid && given.players[p.steamid]) || p.own;
  return p;
}

// A team as it is called: the name the room gave it, else `label` (BLU, RED).
/** @param {string} team @param {string} label */
export const teamLabel = (team, label) => given.teams[team] || label;

/** @param {NamesLine} m */
export function onNames(m) {
  given.players = m.players ?? {};
  given.teams = m.teams ?? {};
  for (const p of [...state.players.values(), ...state.snaps.flatMap(s => s.players)]) named(p);
  for (const team of ["blue", "red"])
    for (const el of $("scoreboard").querySelectorAll(`.sb-team.${team} .sb-name`))
      el.textContent = teamLabel(team, team === "blue" ? "BLU" : "RED");
  state.lastPanel = 0;
}
