// Who is who, in the comms view (voiceinvite.js): everyone in the channel the bot is in, heard
// yet or not, each with the player they are (put there by someone, known from before, or guessed
// from their name: a guess shows as one until someone confirms it), to change: another player,
// nobody, or a Steam ID as people write them.
import { state } from "../core/core.js";
import { HOME } from "../room/home.js";
import * as voice from "../core/voice.js";
import { act, el, heading } from "./voicebits.js";

/** @typedef {{ discord: string; name: string; steamid: string | null; linked: string | null; heard: boolean; confirmed: boolean }} Person */
/** @type {Person[]} */
let people = [];
// Whose Steam ID is being typed, what is typed, and why it would not do.
let typing = "",
  typedId = "",
  refused = "";

// Whether `a` and `b` look like one name: the same letters and digits, or one within the other.
/** @param {string} a @param {string} b */
export function alike(a, b) {
  const [x, y] = [a, b].map(n => n.toLowerCase().replace(/[^\p{L}\p{N}]/gu, ""));
  return x.length > 2 && y.length > 2 && (x.includes(y) || y.includes(x));
}

// The player someone is, if they are one of the match now: by whom they are put on, else by
// the account they linked.
/** @param {Person} v */
const playerOf = v =>
  [...state.players.values()].find(p => p.steamid === (v.steamid ?? (v.confirmed ? null : v.linked)));

// The players someone may be: those whose name looks like theirs first, then RED, then BLU;
// not those someone else in the channel is.
/** @param {Person} v */
function choices(v) {
  const taken = new Set(people.filter(o => o !== v).map(o => playerOf(o)?.id));
  const players = [...state.players.values()].filter(p => (p.team === "red" || p.team === "blue") && !taken.has(p.id));
  const rank = (/** @type {{ name: string; team: string }} */ p) => {
    if (alike(p.name, v.name)) return 0;
    return p.team === "red" ? 1 : 2;
  };
  return players.sort((a, b) => rank(a) - rank(b));
}

/** @param {Person} v */
function picker(v) {
  const select = el("select", { title: `Who ${v.name} is in the match` });
  select.dataset.who = v.discord;
  const now = playerOf(v);
  const nobody = el("option", { value: "none" }, "Not a player");
  nobody.selected = !now && !v.steamid;
  select.append(nobody);
  for (const p of choices(v)) {
    const o = el("option", { value: String(p.id) }, p.name);
    o.selected = p.id === now?.id;
    select.append(o);
  }
  // Put on someone who is not in the match (yet): by their Steam ID.
  if (v.steamid && !now) select.append(el("option", { value: "kept", selected: true }, v.steamid));
  select.append(el("option", { value: "steamid" }, "A Steam ID…"));
  return select;
}

/** @param {Person} v */
function row(v) {
  const talking = voice
    .speakersHeard()
    .some(
      s => s.talking && (s.steamid === `discord:${v.discord}` || s.steamid === v.steamid || s.steamid === v.linked),
    );
  const name = el("span", { className: `vname${talking ? " on" : ""}` }, v.name);
  if (typing === v.discord) {
    const input = el("input", {
      type: "text",
      value: typedId,
      placeholder: "STEAM_0:1:… or a profile link",
      spellcheck: false,
    });
    input.dataset.steamid = v.discord;
    return el("div", { className: "vrow vwho" }, name, input, act("✓", `who-id:${v.discord}`), act("✕", "who-cancel"));
  }
  const select = picker(v);
  const set = Boolean(playerOf(v)) || Boolean(v.steamid);
  const linked = !v.steamid && Boolean(playerOf(v));
  // Set by someone or by their own link; a guess from the name; or not set.
  if (set) select.classList.add(v.confirmed || linked ? "set" : "maybe");
  const r = el("div", { className: "vrow vwho" }, name, select);
  if (linked) r.title = "Linked their Steam account through Discord";
  if (set && !v.confirmed && !linked) r.append(act("✓", `who-ok:${v.discord}`, "Yes, that is them"));
  return r;
}

export function whoRows() {
  if (!people.length) return [el("div", { className: "vhint" }, "Nobody is in the channel yet.")];
  const set = people.filter(v => playerOf(v) || v.steamid || v.confirmed).length;
  const parts = [heading("Who is who", `${set} of ${people.length} set`), ...people.map(row)];
  if (refused) parts.push(el("div", { className: "vhint vbad" }, refused));
  parts.push(
    el(
      "div",
      { className: "vhint" },
      "Set anyone before they speak: a player of the match, or a Steam ID in any form.",
    ),
  );
  return parts;
}

/** @param {() => void} show Draws the view anew. */
export function askWho(show) {
  fetch(new URL("voices", HOME))
    .then(r => (r.status === 200 ? r.json() : []))
    .then(
      (/** @type {Person[]} */ now) => {
        if (JSON.stringify(now) === JSON.stringify(people)) return;
        people = now;
        show();
      },
      () => {},
    );
}

/** @param {string} discord @param {object} whom @param {() => void} show */
function put(discord, whom, show) {
  refused = "";
  fetch(new URL("voice-match", HOME), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ discord, ...whom }),
  })
    .then(r => {
      if (r.ok) typing = "";
      else return r.text().then(why => (refused = why));
    })
    .catch(() => (refused = "stv.tf did not answer."))
    .finally(() => askWho(show))
    .finally(show);
}

// What the comms view's list hears that is of who is who.
/** @param {HTMLElement} list @param {() => void} show */
export function whoEvents(list, show) {
  list.addEventListener("change", e => {
    const select = /** @type {HTMLSelectElement} */ (e.target);
    const discord = select.dataset.who;
    if (!discord) return;
    select.blur();
    if (select.value === "steamid") {
      [typing, typedId, refused] = [discord, "", ""];
      return show();
    }
    if (select.value !== "kept") put(discord, select.value === "none" ? {} : { player: Number(select.value) }, show);
  });
  list.addEventListener("input", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    if (input.dataset.steamid) typedId = input.value;
  });
  list.addEventListener("keydown", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    if (e.key === "Enter" && input.dataset?.steamid) put(input.dataset.steamid, { steamid: typedId }, show);
  });
  list.addEventListener("click", e => {
    const does =
      /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[data-invite]"))?.dataset.invite ??
      "";
    if (does === "who-cancel") {
      typing = "";
      show();
    } else if (does.startsWith("who-id:")) put(does.slice(7), { steamid: typedId }, show);
    else if (does.startsWith("who-ok:")) {
      const v = people.find(p => p.discord === does.slice(7));
      if (v?.steamid) put(v.discord, { steamid: v.steamid }, show);
    }
  });
}
