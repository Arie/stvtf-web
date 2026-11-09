// Who is in a room: the list, the viewer's own name, admins' pen colours, and signing in.
import { $ } from "../core/core.js";
import { env } from "../core/env.js";
import { peopleListeners, streamParams } from "../core/stream.js";
import { inkOf, setInks } from "./telestrator.js";
import { me, names, post } from "./home.js";

// Where a name is kept between visits.
const NAME_KEY = "name";

// Who is in the room, with the viewer's own name to set; admins make others admin or not.
/**
 * @param {(admin: boolean) => void} allow @param {() => void} changed names changed
 * @returns {() => void} shows the list again
 */
export function people(allow, changed) {
  const list = $("peoplelist"),
    name = ownName();
  $("people").hidden = false;
  /** @type {import("../../types/stream").PeopleLine | null} */
  let latest = null;
  peopleListeners.add(m => {
    latest = m;
    show(m);
  });
  return () => latest && show(latest);
  /** @param {import("../../types/stream").PeopleLine} m */
  function show(m) {
    const self = m.people.find(p => p.id === me.id);
    if (self && self.admin !== me.admin) allow(self.admin);
    setInks(m.people);
    names.clear();
    for (const p of m.people) names.set(p.id, p.name);
    changed();
    // One's own name is edited in place, unless signed in; a list redrawn while typing keeps it.
    const typing = document.activeElement === name ? [name.selectionStart, name.selectionEnd] : null;
    list.replaceChildren(...m.people.map(p => personRow(p, name)));
    if (typing) {
      name.focus();
      name.setSelectionRange(typing[0], typing[1]);
    }
  }
}

// The viewer's own name: kept in the browser, told to the room as it changes and as the stream
// opens (so the room knows it from the start).
function ownName() {
  const name = /** @type {HTMLInputElement} */ ($("myname"));
  try {
    name.value = (env.storage && localStorage.getItem(NAME_KEY)) || "";
  } catch {}
  if (name.value) streamParams.set("name", name.value);
  name.onchange = () => {
    post("name", { name: name.value });
    streamParams.set("name", name.value);
    try {
      if (env.storage) localStorage.setItem(NAME_KEY, name.value);
    } catch {}
  };
  name.onkeydown = e => {
    if (e.key === "Enter") name.blur();
  };
  return name;
}

/** @typedef {import("../../types/stream").PeopleLine["people"][number]} Person */

// One person's row: their name (one's own to edit), "(you)" and their role, an admin's button to
// make them admin or not, and an admin's pen colour.
/** @param {Person} p @param {HTMLInputElement} name the viewer's own name */
function personRow(p, name) {
  const li = document.createElement("li");
  li.append(...personName(p, name));
  // "Arie (you) OWNER": spaces between, so it copies as it reads.
  const you = Object.assign(document.createElement("span"), { className: "you", textContent: "(you)" }),
    role = (p.owner && "owner") || (p.admin && "admin") || "",
    tag = Object.assign(document.createElement("span"), { className: "tag", textContent: role });
  li.append(...(p.id === me.id ? [" ", you] : []), " ", tag);
  if (me.admin && !p.owner && p.id !== me.id) {
    const b = document.createElement("button");
    b.textContent = p.admin ? "Remove admin" : "Make admin";
    b.onclick = () => post("admin", { person: p.id, admin: !p.admin });
    li.append(b);
  }
  // Last, so names line up: an admin's pen colour, which they pick for themselves.
  if (p.admin) li.append(penColour(p));
  return li;
}

// A person's name: one's own in place to edit (with a pen), unless signed in; a signed-in name
// leads to their Steam profile, with a check.
/** @param {Person} p @param {HTMLInputElement} name @returns {Node[]} */
function personName(p, name) {
  if (p.id === me.id && !p.signed) {
    name.placeholder = p.name;
    // A pen says it can be changed.
    const pen = Object.assign(document.createElement("span"), { className: "rename", textContent: "✎" });
    pen.title = "Change your name";
    pen.onclick = () => name.focus();
    return [name, pen];
  }
  const nm = /^\d+$/.test(p.steamid ?? "")
    ? steamLink(/** @type {string} */ (p.steamid))
    : document.createElement("span");
  nm.className = "nm";
  nm.textContent = p.name;
  if (p.signed) {
    const mark = Object.assign(document.createElement("span"), { className: "signed", textContent: "✓" });
    mark.title = "Signed in with serveme";
    nm.append(" ", mark);
  }
  return [nm];
}

// An admin's pen colour: one's own to pick, another's to see.
/** @param {Person} p */
function penColour(p) {
  if (p.id === me.id) {
    const pick = Object.assign(document.createElement("input"), { type: "color", className: "ink" });
    pick.title = "Your pen colour";
    pick.value = inkOf(p.id);
    pick.onchange = () => post("ink", { ink: pick.value });
    return pick;
  }
  const ink = Object.assign(document.createElement("i"), { className: "ink", title: "Their pen colour" });
  ink.style.background = inkOf(p.id);
  return ink;
}

/** @param {string} steamid */
function steamLink(steamid) {
  const a = document.createElement("a");
  a.href = `https://steamcommunity.com/profiles/${steamid}/`;
  a.target = "_blank";
  a.rel = "noopener noreferrer";
  a.title = "Steam profile";
  return a;
}

// Signing in with serveme: the name is theirs there, and a streamer may make themselves admin.
/** @param {{ admin: boolean; signed: boolean; streamer: boolean; signin: boolean }} m */
export function signedIn(m) {
  const back = encodeURIComponent(location.pathname);
  const signin = /** @type {HTMLAnchorElement} */ ($("signin")),
    signout = /** @type {HTMLAnchorElement} */ ($("signout")),
    claim = /** @type {HTMLButtonElement} */ ($("claim"));
  signin.href = new URL(`signin?return=${back}`, env.base).href;
  signout.href = new URL(`signout?return=${back}`, env.base).href;
  signin.hidden = !m.signin || m.signed;
  signout.hidden = !m.signed;
  $("myname").hidden = m.signed;
  claim.hidden = !m.streamer || m.admin;
  claim.onclick = () => post("claim", {}).then(() => (claim.hidden = true));
}
