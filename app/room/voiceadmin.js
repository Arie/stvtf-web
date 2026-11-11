// For a room's admins, at the end of the speakers list (panel/voicelist.js), once voice from
// outside the match comes (a team's Discord, through the voice bot): each
// voice that is no player's (no Steam link, or linked on another account) put on one.
import { $, state } from "../core/core.js";
import { HOME, me, post } from "./home.js";
import { addListExtra, renderList } from "../panel/voicelist.js";

// How often the voices are asked for while the list is open, in milliseconds.
const ASK_MS = 5000;

/** @typedef {{ discord: string; name: string; steamid: string | null; linked: string | null }} Heard */
/** @type {Heard[]} */
let voices = [];
// Whether this page may: an admin of the room, and where people sign in, a streamer.
let allowed = false;

/**
 * @template {keyof HTMLElementTagNameMap} K
 * @param {K} tag @param {Partial<HTMLElementTagNameMap[K]>} [props] @param {(Node | string)[]} children
 * @returns {HTMLElementTagNameMap[K]}
 */
function el(tag, props = {}, ...children) {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

// Whether `a` and `b` look like one name: the same letters and digits, or one within the other.
/** @param {string} a @param {string} b */
export function alike(a, b) {
  const [x, y] = [a, b].map(n => n.toLowerCase().replace(/[^\p{L}\p{N}]/gu, ""));
  return x.length > 2 && y.length > 2 && (x.includes(y) || y.includes(x));
}

// The players a voice may be: those whose name looks like its own first (a suggestion, never
// chosen for the admin).
/** @param {Heard} v */
function choice(v) {
  const select = el("select", { title: `Who ${v.name} is in the match` });
  select.dataset.match = v.discord;
  select.append(el("option", { value: "" }, "Not a player"));
  const players = [...state.players.values()].filter(p => p.team === "red" || p.team === "blue");
  players.sort((a, b) => Number(alike(b.name, v.name)) - Number(alike(a.name, v.name)));
  for (const p of players) {
    const o = el("option", { value: String(p.id) }, alike(p.name, v.name) ? `${p.name} (same name?)` : p.name);
    o.selected = v.steamid != null && p.steamid === v.steamid;
    select.append(o);
  }
  return select;
}

// The voices to put on a player: all but those whose own link is a player in the match (unless
// put on another by hand).
function toPlace() {
  const playing = new Set([...state.players.values()].map(p => p.steamid));
  return voices.filter(v => v.steamid != null || v.linked == null || !playing.has(v.linked));
}

function rows() {
  const place = toPlace();
  if (!allowed || !place.length) return [];
  const head = el("div", { className: "vteam t-none" }, el("span", { className: "vtname" }, "From Discord"));
  return [
    head,
    ...place.map(v => el("div", { className: "vrow vmatch" }, el("span", { className: "vname" }, v.name), choice(v))),
  ];
}

export function voiceAdmin() {
  const list = $("voicelist");
  addListExtra(rows);
  const show = () => list.hidden || renderList(list);
  const ask = () => {
    if (list.hidden || !me.admin) return;
    fetch(new URL("voices", HOME))
      .then(r => (r.ok ? r.json() : Promise.resolve([])).then(heard => ({ ok: r.ok, heard })))
      .then(
        (/** @type {{ ok: boolean; heard: Heard[] }} */ { ok, heard }) => {
          if (ok === allowed && JSON.stringify(heard) === JSON.stringify(voices)) return;
          [allowed, voices] = [ok, heard];
          show();
        },
        () => {},
      );
  };
  setInterval(ask, ASK_MS);
  $("voicetoggle").addEventListener("click", () => setTimeout(ask));
  list.addEventListener("change", e => {
    const select = /** @type {HTMLSelectElement} */ (e.target);
    const discord = select.dataset.match;
    if (!discord) return;
    select.blur();
    post("voice-match", { discord, player: select.value ? Number(select.value) : null }).then(ask, ask);
  });
}
