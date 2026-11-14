// Comms, for whoever started the match and for casters: at the end of the speakers list
// (panel/voicelist.js) a button swaps the list for this view, which is wider. With nothing
// connected: Discord or Mumble. Discord: a button that copies the command that brings the voice
// bot into a voice channel, to paste there or pass on to a team. Mumble: the server, its password
// and the channel, and the bot joins at once. Once it is in a channel, those it is in show: how
// each goes, a way to add another (the other team's), and who is who (voicewho.js). See the
// viewer's Voice.
import { $ } from "../core/core.js";
import { HOME } from "../room/home.js";
import { copyText } from "../room/copy.js";
import { addListExtra, renderList, setListView } from "./voicelist.js";
import { act, el, labelOf } from "./voicebits.js";
import {
  fillAfter,
  fillFor,
  keepTyped,
  mumbleButtons,
  mumbleFields,
  mumbleRequest,
  serverOf,
  typed,
} from "./mumbleform.js";
import { pollWho, whoEvents, whoRows } from "./voicewho.js";

// What most of competitive TF2 sets `tv_delay` to, in seconds: the bot's own when none is said.
const USUAL_DELAY = 90,
  MAX_DELAY = 300;
// How long the button says it copied, in milliseconds.
const COPIED_MS = 4000;
// How soon the bot is asked about again while it is on its way, in milliseconds.
const SOON_MS = 200;

// The command for Discord: `page` (the match's, or a room's on it) with the invite, and the delay
// only when it is not the usual.
/** @param {URL} page @param {string} invite @param {number} delay */
export function joinCommand(page, invite, delay) {
  const link = new URL(page);
  link.search = "";
  link.hash = "";
  link.searchParams.set("voice", invite);
  return `/stv join link:${link.href}${delay === USUAL_DELAY ? "" : ` delay:${delay}`}`;
}

/** @typedef {import("./mumbleform.js").InMumble} InMumble */
/** @type {{ invite: string; bot: string | null } | null} */
let botInvite = null;
// Where the bot is, as stv.tf last said.
/** @type {{ discord: { status: string; state?: string } | null; mumble: InMumble[] }} */
let comms = { discord: null, mumble: [] };
// What the view shows: none (the speakers list), the choice or what is connected (home), one of
// the two set-ups, or the bot being moved; the Mumble channel being moved or taken out (none
// while one is added); the delay asked for; whether the command was just copied; and why what was
// asked would not do.
/** @type {{ screen: "" | "home" | "discord" | "mumble" | "reconnect"; editing: number | null; delay: number; copied: boolean; refused: string }} */
const view = { screen: "", editing: null, delay: USUAL_DELAY, copied: false, refused: "" };

/** @type {HTMLElement} */
let list;
const redraw = () => list.hidden || renderList(list);

/** @param {string} status */
const failed = status => /^(Could not join|Left)/.test(status);
// Where the bot is in a channel is its `state` (in, connecting, failed, left); a site that says
// none is gone by its status text.
/** @param {{ status: string; state?: string }} c It would not join, or left. */
const out = c => (c.state ? c.state === "failed" || c.state === "left" : failed(c.status));
/** @param {{ status: string; state?: string }} c On its way in: also when the bot went silent. */
const onItsWay = c => (c.state ? c.state === "connecting" : c.status === "connecting");
/** @param {{ status: string; state?: string }} c What its line says. */
const said = c => (onItsWay(c) ? "connecting" : c.status);

// How it goes: green in a channel, yellow on the way, red when it would not do.
/**
 * @param {string} status @param {string} [more] @param {string} [where] Discord or Mumble, said before it.
 * @param {boolean} [bad] It would not do, whatever the words.
 */
function statusLine(status, more = "", where = "", bad = failed(status)) {
  let dot = "good";
  if (bad) dot = "bad";
  else if (status === "connecting" || status.startsWith("Waiting")) dot = "wait";
  const text = el("span", { className: "vstatustext" }, where ? `${where}: ${status}` : status);
  if (more) text.append(el("span", { className: "vhint" }, more));
  const line = el("div", { className: "vstatus" }, el("span", { className: `vdot ${dot}` }), text);
  return line;
}

function delayRow() {
  const secs = el("input", { type: "number", min: "0", max: String(MAX_DELAY), value: String(view.delay) });
  secs.dataset.action = "delay";
  const row = el(
    "label",
    { className: "vrow vinvite vfield" },
    labelOf("tv_delay"),
    secs,
    el("span", { className: "vhint" }, "s"),
  );
  row.title = "0 for casters";
  return row;
}

/** @param {string} back Where the back button goes. @param {string} title */
const top = (back, title) =>
  el(
    "div",
    { className: "vrow vtop" },
    act(back === "" ? "‹ Speakers" : "‹ Back", `go:${back}`),
    el("span", { className: "vtitle" }, title),
  );

// Nothing connected: where does the team talk?
function choice() {
  const discord = act("Discord", "go:discord");
  const mumble = act("Mumble", "go:mumble");
  discord.className = "vcopy";
  mumble.className = "vcopy vmumble";
  return [top("", "Comms"), el("div", { className: "vrow vinvite" }, discord, mumble)];
}

function discordSetup() {
  if (!botInvite) return [];
  const copy = act(view.copied ? "✓ Copied" : "Copy command", "copy");
  copy.className = "vcopy";
  // The word Discord leads to where the bot is added to a server.
  const discord = botInvite.bot
    ? el("a", { href: botInvite.bot, target: "_blank", rel: "noopener", title: "Invite bot to server" }, "Discord")
    : "Discord";
  const parts = [
    top("home", "Discord"),
    el("div", { className: "vrow vinvite" }, copy),
    el("div", { className: "vhint" }, "Paste in ", discord, ", while in voice"),
    delayRow(),
  ];
  parts.push(statusLine("Waiting for bot…"));
  return parts;
}

/** @param {InMumble | null} was The channel the bot is in already, to move it; none to add one. */
function mumbleSetup(was) {
  /** @type {HTMLElement[]} */
  const parts = [top("home", "Mumble")];
  if (view.refused) parts.push(statusLine(view.refused, "", "", true));
  else if (was && failed(was.status)) parts.push(statusLine(was.status));
  // A server the bot is on already: its password is known.
  const known = Boolean(was) || comms.mumble.some(m => serverOf(typed.server)?.host === m.host);
  return [...parts, ...mumbleFields(known), delayRow(), mumbleButtons(Boolean(was))];
}

// The bot is in a channel, or a few: how each goes, a way to add another, and who is who.
function connected() {
  const parts = [top("", "Comms")];
  if (comms.discord) {
    parts.push(
      statusLine(said(comms.discord), "", "Discord", out(comms.discord)),
      el("div", { className: "vhint" }, "/stv delay · /stv leave"),
    );
  }
  for (const m of comms.mumble) {
    const line = statusLine(said(m), `tv_delay ${m.delay} s`, "Mumble", out(m));
    line.append(act("Reconnect", `go:reconnect:${m.id}`));
    parts.push(line);
  }
  const more = el(
    "div",
    { className: "vrow vinvite vmore" },
    act("+ Discord", "go:discord"),
    act("+ Mumble", "go:mumble"),
  );
  return [...parts, more, ...whoRows()];
}

// The view that takes the list's place.
function commsView() {
  if (!botInvite) return [];
  const moved = view.screen === "reconnect" && comms.mumble.find(m => m.id === view.editing);
  if (moved) return mumbleSetup(moved);
  if (view.screen === "discord") return discordSetup();
  if (view.screen === "mumble") return mumbleSetup(null);
  // The only one, and it would not do: its form again, with why.
  const [only] = comms.mumble;
  if (!comms.discord && comms.mumble.length === 1 && out(only)) return mumbleSetup(only);
  return comms.discord || comms.mumble.length ? connected() : choice();
}

// In the speakers list: the way in.
function commsButton() {
  if (!botInvite || view.screen) return [];
  const b = act("🎧 Comms", "go:home", "Discord or Mumble");
  b.className = "vcopy";
  return [el("div", { className: "vrow vinvite" }, b)];
}

const postMumble = (/** @type {object} */ body) =>
  fetch(new URL("voice-mumble", HOME), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });

// Where the bot is, asked while the view is open: the view anew when that changed.
let soon = 0;
function pollComms() {
  if (!botInvite || !view.screen || list.hidden) return;
  fetch(new URL("voice-comms", HOME))
    .then(r => (r.status === 200 ? r.json() : null))
    .then(
      now => {
        if (!now) return;
        const shape = (/** @type {typeof comms} */ c) =>
          `${c.discord?.status ?? ""}/${c.discord?.state ?? ""}/${c.mumble.map(m => `${m.id}/${m.host}/${m.channel}/${m.delay}/${m.status}/${m.state ?? ""}`).join("/")}`;
        const changed = shape(now) !== shape(comms);
        // The bot came into Discord while its set-up showed: on to where it is.
        if (view.screen === "discord" && now.discord && !comms.discord) view.screen = "home";
        comms = now;
        // On its way: asked again soon, so the dot turns as the bot gets there.
        clearTimeout(soon);
        if (now.mumble.some((/** @type {InMumble} */ m) => onItsWay(m))) soon = window.setTimeout(pollComms, SOON_MS);
        if (changed) redraw();
      },
      () => {},
    );
  pollWho(redraw);
}

let copiedTimer = 0;
function copyCommand() {
  if (!botInvite) return;
  copyText(joinCommand(HOME, botInvite.invite, view.delay)).then(ok => {
    if (!ok) return;
    view.copied = true;
    redraw();
    clearTimeout(copiedTimer);
    copiedTimer = window.setTimeout(() => {
      view.copied = false;
      redraw();
    }, COPIED_MS);
  });
}

// The bot asked into the Mumble channel typed in (or moved there); why not, when it will not do.
function joinMumble() {
  const refuse = (/** @type {string} */ why) => {
    view.refused = why;
    redraw();
  };
  view.refused = "";
  const asked = mumbleRequest(view.editing, view.delay);
  if ("why" in asked) return refuse(asked.why);
  postMumble(asked.body).then(
    r => {
      if (!r.ok) return void r.text().then(why => refuse(why || "Refused"));
      view.screen = "home";
      view.editing = null;
      pollComms();
    },
    () => refuse("No answer"),
  );
}

/** @param {"" | "home" | "discord" | "mumble" | "reconnect"} to @param {number | null} [id] The Mumble channel to move. */
function showScreen(to, id = null) {
  view.screen = to;
  view.refused = "";
  view.editing = null;
  const [only] = comms.mumble;
  // One to move: its own, or the only one when it would not do (its form shows by itself).
  const m =
    to === "reconnect" ? comms.mumble.find(j => j.id === id) : to === "home" && comms.mumble.length === 1 && only;
  if (m && (to === "reconnect" || out(m))) {
    view.editing = m.id;
    fillFor(m);
    view.delay = m.delay;
  } else if (to === "mumble") {
    fillAfter(comms.mumble.at(-1));
  }
  setListView(to ? commsView : null);
  list.classList.toggle("wide", Boolean(to));
  redraw();
  pollComms();
}

/** @param {string} does What the button clicked does. */
function onAction(does) {
  if (does.startsWith("go:")) {
    const [to, id] = does.slice(3).split(":");
    return showScreen(/** @type {any} */ (to), id ? Number(id) : null);
  }
  if (does === "copy") return copyCommand();
  if (does === "join") return joinMumble();
  if (does === "leave") {
    const id = view.editing;
    comms = { ...comms, mumble: comms.mumble.filter(m => m.id !== id) };
    postMumble(id == null ? { leave: true } : { leave: true, id }).then(pollComms, () => {});
    showScreen("home");
  }
}

export function voiceInvite() {
  list = $("voicelist");
  addListExtra(commsButton);
  fetch(new URL("voice-invite", HOME))
    .then(r => (r.status === 200 ? r.json() : null))
    .then(
      g => {
        botInvite = g?.invite ? { invite: String(g.invite), bot: typeof g.bot === "string" ? g.bot : null } : null;
        redraw();
      },
      () => {},
    );
  setInterval(pollComms, 1000);
  whoEvents(list, redraw);
  list.addEventListener("click", e => {
    const b = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[data-action]"));
    if (b?.dataset.action && botInvite) onAction(b.dataset.action);
  });
  list.addEventListener("input", e => keepTyped(/** @type {HTMLInputElement} */ (e.target)));
  list.addEventListener("change", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    if (input.dataset.action !== "delay") return;
    const n = Math.round(Number(input.value));
    view.delay = Number.isFinite(n) ? Math.max(0, Math.min(MAX_DELAY, n)) : USUAL_DELAY;
  });
}
