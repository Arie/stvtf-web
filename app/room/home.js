// What the room's parts share: where its routes are, who this page is in it, and posting to it.
import { PAGE } from "../core/core.js";
import { env } from "../core/env.js";

export const IN_ROOM = /\/r\/[0-9a-f]{32}\/$/.test(PAGE.replace(/overlay\/?$/, ""));
// The room's or match's own address, which its routes are under.
export const HOME = new URL(PAGE.replace(/(overlay\/?|index\.html)?$/, ""), env.base);
// Who this page is in the room: its number, and whether it is an admin.
export const me = { id: 0, admin: false };
// Everyone's names, by number.
/** @type {Map<number, string>} */
export const names = new Map();
/** @param {string} path @param {object} body */
export const post = (path, body, base = HOME) =>
  fetch(new URL(path, base), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
