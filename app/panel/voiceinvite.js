// Comms, for whoever started the match and for casters: at the end of the speakers list
// (panel/voicelist.js) a button swaps the list for this view, which is wider. With nothing
// connected: Discord or Mumble. Discord: a button that copies the command that brings the voice
// bot into a voice channel, to paste there or pass on to a team. Mumble: the server, its password
// and the channel, and the bot joins at once. Once it is in a channel, only that one shows: how
// it goes, and who is who (voicewho.js). See the viewer's Voice.
import { $ } from "../core/core.js";
import { HOME } from "../room/home.js";
import { copyText } from "../room/copy.js";
import { addListExtra, renderList, setListView } from "./voicelist.js";
import { act, el } from "./voicebits.js";
import { askWho, whoEvents, whoRows } from "./voicewho.js";

// What most of competitive TF2 sets `tv_delay` to, in seconds: the bot's own when none is said.
const USUAL_DELAY = 90,
  MAX_DELAY = 300;
// How long the button says it copied, in milliseconds.
const COPIED_MS = 4000;
// How soon the bot is asked about again while it is on its way, in milliseconds.
const SOON_MS = 200;
// Mumble's own port.
const MUMBLE_PORT = 64738;

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

// A Mumble server as someone writes it: its name or address, with a port or without, or a
// mumble:// link. None for what is no server.
/** @param {string} text @returns {{ host: string; port: number } | null} */
export function serverOf(text) {
  const m = /^(?:mumble:\/\/)?(?:[^@/\s]*@)?([A-Za-z0-9.-]+)(?::(\d{1,5}))?(?:\/\S*)?$/.exec(text.trim());
  if (!m) return null;
  const port = m[2] ? Number(m[2]) : MUMBLE_PORT;
  return port >= 1 && port <= 65535 ? { host: m[1], port } : null;
}

/** @typedef {{ host: string; port: number; channel: string; delay: number; status: string }} InMumble */
/** @type {{ invite: string; bot: string | null } | null} */
let given = null;
// Where the bot is, as stv.tf last said.
/** @type {{ discord: { status: string } | null; mumble: InMumble | null }} */
let comms = { discord: null, mumble: null };
// Which part of the view shows: none (the speakers list), the choice or what is connected, one
// of the two set-ups, or the bot being moved.
/** @type {"" | "home" | "discord" | "mumble" | "reconnect"} */
let screen = "";
let delay = USUAL_DELAY;
let copied = false;
// What is typed for Mumble, kept while the view is drawn anew, and why it would not do.
const typed = { server: "", password: "", channel: "" };
let refused = "";

/** @type {HTMLElement} */
let list;
const show = () => list.hidden || renderList(list);

/** @param {string} status */
const failed = status => /^(Could not join|Left)/.test(status);

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
  line.id = "vcomms";
  return line;
}

// A form's labels: one width, so what is typed lines up.
const labelOf = (/** @type {string} */ text) => el("span", { className: "vlabel" }, text);

function delayRow() {
  const secs = el("input", { type: "number", min: "0", max: String(MAX_DELAY), value: String(delay) });
  secs.dataset.invite = "delay";
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

/** @param {keyof typeof typed} name @param {string} label @param {string} hint @param {string} [type] */
function field(name, label, hint, type = "text") {
  const input = el("input", { type, value: typed[name], placeholder: hint, autocomplete: "off", spellcheck: false });
  input.dataset.mumble = name;
  return el("label", { className: "vrow vinvite vfield" }, labelOf(label), input);
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
  if (!given) return [];
  const copy = act(copied ? "✓ Copied" : "Copy command", "copy");
  copy.className = "vcopy";
  // The word Discord leads to where the bot is added to a server.
  const discord = given.bot
    ? el("a", { href: given.bot, target: "_blank", rel: "noopener", title: "Invite bot to server" }, "Discord")
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

/** @param {boolean} moving The bot is somewhere already: this moves it. */
function mumbleSetup(moving) {
  /** @type {HTMLElement[]} */
  const parts = [top("home", "Mumble")];
  const was = comms.mumble;
  if (refused) parts.push(statusLine(refused, "", "", true));
  else if (!moving && was && failed(was.status)) parts.push(statusLine(was.status));
  const join = act(moving ? "Reconnect" : "Connect", "join");
  join.className = "vcopy vmumble";
  parts.push(
    field("server", "Server", "host:port"),
    field("password", "Password", moving ? "unchanged" : "optional", "password"),
    field("channel", "Channel", ""),
    delayRow(),
  );
  const buttons = el("div", { className: "vrow vinvite" }, join);
  if (moving) {
    const leave = act("Disconnect", "leave");
    leave.className = "vcopy vleave";
    buttons.append(leave);
  }
  parts.push(buttons);
  return parts;
}

// The bot is in a channel: how it goes there, and who is who.
function connected() {
  const parts = [top("", "Comms")];
  if (comms.mumble) {
    const m = comms.mumble;
    const line = statusLine(m.status, `tv_delay ${m.delay} s`, "Mumble");
    line.append(act("Reconnect", "go:reconnect"));
    parts.push(line);
  } else if (comms.discord) {
    parts.push(
      statusLine(comms.discord.status, "", "Discord"),
      el("div", { className: "vhint" }, "/stv delay · /stv leave"),
    );
  }
  return [...parts, ...whoRows()];
}

// The view that takes the list's place.
function view() {
  if (!given) return [];
  const up = comms.discord || (comms.mumble && !failed(comms.mumble.status));
  if (screen === "reconnect" && comms.mumble) return mumbleSetup(true);
  if (up) return connected();
  if (screen === "discord") return discordSetup();
  if (screen === "mumble" || comms.mumble) return mumbleSetup(false);
  return choice();
}

// In the speakers list: the way in.
function entry() {
  if (!given || screen) return [];
  const b = act("🎧 Comms", "go:home", "Discord or Mumble");
  b.className = "vcopy";
  return [el("div", { className: "vrow vinvite" }, b)];
}

const ask = (/** @type {object} */ body) =>
  fetch(new URL("voice-mumble", HOME), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });

// Where the bot is, asked while the view is open: the view anew when that changed, else only
// the line that says how it goes (so nothing is drawn anew under someone typing).
let soon = 0;
function look() {
  if (!given || !screen || list.hidden) return;
  fetch(new URL("voice-comms", HOME))
    .then(r => (r.status === 200 ? r.json() : null))
    .then(
      now => {
        if (!now) return;
        const shape = (/** @type {typeof comms} */ c) =>
          `${Boolean(c.discord)}/${c.mumble ? `${c.mumble.host}/${c.mumble.channel}/${failed(c.mumble.status)}/${c.mumble.status === "connecting"}` : ""}`;
        const changed = shape(now) !== shape(comms);
        comms = now;
        // On its way: asked again soon, so the dot turns as the bot gets there.
        clearTimeout(soon);
        if (now.mumble?.status === "connecting") soon = window.setTimeout(look, SOON_MS);
        if (changed) return void show();
        const line = list.querySelector("#vcomms .vstatustext");
        const said = now.mumble ? `Mumble: ${now.mumble.status}` : now.discord && `Discord: ${now.discord.status}`;
        if (line?.firstChild && said) line.firstChild.textContent = said;
      },
      () => {},
    );
  askWho(show);
}

let copiedTimer = 0;
function copyCommand() {
  if (!given) return;
  copyText(joinCommand(HOME, given.invite, delay)).then(ok => {
    if (!ok) return;
    copied = true;
    show();
    clearTimeout(copiedTimer);
    copiedTimer = window.setTimeout(() => {
      copied = false;
      show();
    }, COPIED_MS);
  });
}

// The bot asked into the Mumble channel typed in (or moved there); why not, when it will not do.
function joinMumble() {
  const server = serverOf(typed.server);
  refused = "";
  if (!server) refused = "Invalid server";
  else if (!typed.channel.trim()) refused = "Channel name required";
  if (!server || refused) return void show();
  const refuse = (/** @type {string} */ why) => {
    refused = why;
    show();
  };
  // No password typed while moving the bot: the one given before.
  const password = typed.password || (screen === "reconnect" ? undefined : "");
  ask({ ...server, password, channel: typed.channel.trim(), delay }).then(
    r => {
      if (!r.ok) return void r.text().then(why => refuse(why || "Refused"));
      comms = { discord: null, mumble: { ...server, channel: typed.channel.trim(), delay, status: "connecting" } };
      screen = "home";
      show();
      look();
    },
    () => refuse("No answer"),
  );
}

/** @param {"" | "home" | "discord" | "mumble" | "reconnect"} to */
function go(to) {
  screen = to;
  refused = "";
  if (to === "reconnect" && comms.mumble) {
    const m = comms.mumble;
    Object.assign(typed, {
      server: m.port === MUMBLE_PORT ? m.host : `${m.host}:${m.port}`,
      password: "",
      channel: m.channel,
    });
    delay = m.delay;
  }
  setListView(to ? view : null);
  list.classList.toggle("wide", Boolean(to));
  show();
  look();
}

/** @param {string} does What the button clicked does. */
function clicked(does) {
  if (does.startsWith("go:")) return go(/** @type {any} */ (does.slice(3)));
  if (does === "copy") return copyCommand();
  if (does === "join") return joinMumble();
  if (does === "leave") {
    comms = { ...comms, mumble: null };
    screen = "home";
    show();
    ask({ leave: true });
  }
}

export function voiceInvite() {
  list = $("voicelist");
  addListExtra(entry);
  fetch(new URL("voice-invite", HOME))
    .then(r => (r.status === 200 ? r.json() : null))
    .then(
      g => {
        given = g?.invite ? { invite: String(g.invite), bot: typeof g.bot === "string" ? g.bot : null } : null;
        show();
      },
      () => {},
    );
  setInterval(look, 1000);
  whoEvents(list, show);
  list.addEventListener("click", e => {
    const b = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[data-invite]"));
    if (b?.dataset.invite && given) clicked(b.dataset.invite);
  });
  list.addEventListener("input", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    const f = /** @type {keyof typeof typed | undefined} */ (input.dataset.mumble);
    if (f) typed[f] = input.value;
  });
  list.addEventListener("change", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    if (input.dataset.invite !== "delay") return;
    const n = Math.round(Number(input.value));
    delay = Number.isFinite(n) ? Math.max(0, Math.min(MAX_DELAY, n)) : USUAL_DELAY;
  });
}
