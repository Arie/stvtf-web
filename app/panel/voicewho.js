// Who is who, in the comms view (voiceinvite.js): everyone in the channel the bot is in, heard
// yet or not, each with the player they are (put there by someone, known from before, or guessed
// from their name: a guess shows as one until someone confirms it), to change: another player,
// nobody, or a Steam ID as people write them.
import { state } from "../core/core.js";
import { HOME } from "../room/home.js";
import * as voice from "../core/voice.js";
import { act, el, heading } from "./voicebits.js";

/** @typedef {{ id: string; name: string; steamid: string | null; linked: string | null; heard: boolean; confirmed: boolean }} Person */
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
  const select = el("select", {});
  select.dataset.who = v.id;
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
  select.append(el("option", { value: "steamid" }, "Steam ID…"));
  return select;
}

/** @param {Person} v */
function row(v) {
  const talking = voice
    .speakersHeard()
    .some(s => s.talking && (s.steamid === `discord:${v.id}` || s.steamid === v.steamid || s.steamid === v.linked));
  const name = el("span", { className: `vname${talking ? " on" : ""}` }, v.name);
  if (typing === v.id) {
    const input = el("input", {
      type: "text",
      value: typedId,
      placeholder: "Steam ID or profile link",
      spellcheck: false,
    });
    input.dataset.steamid = v.id;
    return el("div", { className: "vrow vwho" }, name, input, act("✓", `who-id:${v.id}`), act("✕", "who-cancel"));
  }
  const select = picker(v);
  const set = Boolean(playerOf(v)) || Boolean(v.steamid);
  const linked = !v.steamid && Boolean(playerOf(v));
  // Set by someone or by their own link; a guess from the name; or not set.
  if (set) select.classList.add(v.confirmed || linked ? "set" : "maybe");
  const r = el("div", { className: "vrow vwho" }, name, select);
  if (linked) r.title = "Linked via Discord";
  if (set && !v.confirmed && !linked) r.append(act("✓", `who-ok:${v.id}`, "Confirm"));
  return r;
}

export function whoRows() {
  if (!people.length) return [el("div", { className: "vhint" }, "Nobody there")];
  const set = people.filter(v => playerOf(v) || v.steamid || v.confirmed).length;
  const parts = [heading("Who is who", `${set}/${people.length}`), ...people.map(row)];
  if (refused) parts.push(el("div", { className: "vhint vbad" }, refused));
  return parts;
}

/** @param {() => void} redraw Draws the view anew. */
export function pollWho(redraw) {
  fetch(new URL("voices", HOME))
    .then(r => (r.status === 200 ? r.json() : []))
    .then(
      (/** @type {Person[]} */ now) => {
        if (JSON.stringify(now) === JSON.stringify(people)) return;
        people = now;
        redraw();
      },
      () => {},
    );
}

/** @param {string} id @param {object} whom @param {() => void} redraw */
function putOn(id, whom, redraw) {
  refused = "";
  fetch(new URL("voice-match", HOME), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ discord: id, ...whom }),
  })
    .then(r => {
      if (r.ok) typing = "";
      else return r.text().then(why => (refused = why));
    })
    .catch(() => (refused = "No answer"))
    .finally(() => pollWho(redraw))
    .finally(redraw);
}

// What the comms view's list hears that is of who is who.
/** @param {HTMLElement} list @param {() => void} redraw */
export function whoEvents(list, redraw) {
  list.addEventListener("change", e => {
    const select = /** @type {HTMLSelectElement} */ (e.target);
    const id = select.dataset.who;
    if (!id) return;
    select.blur();
    if (select.value === "steamid") {
      [typing, typedId, refused] = [id, "", ""];
      return redraw();
    }
    if (select.value !== "kept") putOn(id, select.value === "none" ? {} : { player: Number(select.value) }, redraw);
  });
  list.addEventListener("input", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    if (input.dataset.steamid) typedId = input.value;
  });
  list.addEventListener("keydown", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    if (e.key !== "Enter" || !input.dataset?.steamid) return;
    // Let go of, so the view is drawn anew (it is not, under someone typing).
    input.blur();
    putOn(input.dataset.steamid, { steamid: typedId }, redraw);
  });
  list.addEventListener("click", e => {
    const does =
      /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[data-action]"))?.dataset.action ??
      "";
    if (does === "who-cancel") {
      typing = "";
      redraw();
    } else if (does.startsWith("who-id:")) putOn(does.slice(7), { steamid: typedId }, redraw);
    else if (does.startsWith("who-ok:")) {
      const v = people.find(p => p.id === does.slice(7));
      if (v?.steamid) putOn(v.id, { steamid: v.steamid }, redraw);
    }
  });
}
