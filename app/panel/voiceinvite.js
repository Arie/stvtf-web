// Setting up comms, for whoever started the match and for casters: at the end of the speakers
// list (panel/voicelist.js) a button that swaps the list for this view. In it, Discord: a button
// that copies the command that brings the voice bot into a voice channel, to paste there or pass
// on to a team. And Mumble: the server, its password and the channel, and the bot joins at once
// and says how it goes. It carries the match's invite, without which no voice
// comes in. See the viewer's Voice.
import { $ } from "../core/core.js";
import { HOME } from "../room/home.js";
import { copyText } from "../room/copy.js";
import { addListExtra, renderList, setListView } from "./voicelist.js";

// What most of competitive TF2 sets `tv_delay` to, in seconds: the bot's own when none is said.
const USUAL_DELAY = 90,
  MAX_DELAY = 300;
// How long the button says it copied, in milliseconds.
const COPIED_MS = 4000;

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

/** @type {{ invite: string; bot: string | null } | null} */
let given = null;
let delay = USUAL_DELAY;
// Whether the command was just copied, the gear's settings show, and the set-up view is open.
let copied = false,
  settings = false,
  open = false;
// What is typed for Mumble, kept while the view is drawn anew; and where the bot is, as stv.tf
// last said.
const mumble = { server: "", password: "", channel: "" };
/** @type {{ host: string; port: number; channel: string; status: string } | null} */
let asked = null;
let refused = "";

// Mumble's own port.
const MUMBLE_PORT = 64738;

// A Mumble server as someone writes it: its name or address, with a port or without, or a
// mumble:// link. None for what is no server.
/** @param {string} text @returns {{ host: string; port: number } | null} */
export function serverOf(text) {
  const m = /^(?:mumble:\/\/)?(?:[^@/\s]*@)?([A-Za-z0-9.-]+)(?::(\d{1,5}))?(?:\/\S*)?$/.exec(text.trim());
  if (!m) return null;
  const port = m[2] ? Number(m[2]) : MUMBLE_PORT;
  return port >= 1 && port <= 65535 ? { host: m[1], port } : null;
}

/**
 * @template {keyof HTMLElementTagNameMap} K
 * @param {K} tag @param {Partial<HTMLElementTagNameMap[K]>} props @param {(Node | string)[]} children
 * @returns {HTMLElementTagNameMap[K]}
 */
function el(tag, props, ...children) {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

/** @param {string} text */
const heading = text => el("div", { className: "vteam t-none" }, el("span", { className: "vtname" }, text));

// Where the bot is added to a Discord server, said as `text`.
/** @param {string} bot @param {string} text */
const addBot = (bot, text) =>
  el("a", { href: bot, target: "_blank", rel: "noopener", className: "vrow vinvite" }, text);

/** @param {string} what @param {string} does @param {string} [title] */
function act(what, does, title = "") {
  const b = el("button", { title }, what);
  b.dataset.invite = does;
  return b;
}

/** @param {keyof typeof mumble} name @param {string} label @param {string} hint @param {string} [type] */
function field(name, label, hint, type = "text") {
  const input = el("input", { type, value: mumble[name], placeholder: hint, autocomplete: "off", spellcheck: false });
  input.dataset.mumble = name;
  return el("label", { className: "vrow vinvite vfield" }, label, input);
}

// In the speakers list: the way in.
function entry() {
  if (!given || open) return [];
  const b = act("Set up comms", "open", "Bring a team's Discord or Mumble into this match");
  b.className = "vcopy";
  return [el("div", { className: "vrow vinvite" }, b)];
}

// The view that takes the list's place: Discord, Mumble, and the delay both go by.
function view() {
  if (!given) return [];
  const back = act("‹ Speakers", "close");
  const top = el("div", { className: "vhead" }, back);
  // Discord: one button that copies the command (never shown: it is long), and a gear.
  const copy = act(
    copied ? "✓ Now paste it in Discord" : "Copy Discord command",
    "copy",
    "The /stv join command for this match: paste it in Discord, in the server of the voice channel you are in",
  );
  copy.className = "vcopy";
  const gear = act("⚙", "settings", "Voice delay, and adding the bot");
  gear.className = "vgear";
  gear.setAttribute("aria-expanded", String(settings));
  /** @type {HTMLElement[]} */
  const parts = [top, heading("Discord"), el("div", { className: "vrow vinvite" }, copy, gear)];
  if (settings) {
    const secs = el("input", { type: "number", min: "0", max: String(MAX_DELAY), value: String(delay) });
    secs.dataset.invite = "delay";
    parts.push(
      el("label", { className: "vrow vinvite" }, "Voice delay", secs, "s"),
      el(
        "div",
        { className: "vhint" },
        "The server's tv_delay, for Discord and Mumble. 0 for casters talking over the stream.",
      ),
    );
    if (given.bot) parts.push(addBot(given.bot, "Add the bot to a Discord server ↗"));
  }
  parts.push(heading("Mumble"));
  if (asked) {
    const status = el("div", { className: "vhint vstatus" }, asked.status);
    status.id = "vmumble";
    parts.push(status, el("div", { className: "vrow vinvite" }, act("Send the bot away", "leave")));
  } else {
    const join = act("Bring the bot in", "join", "The bot joins that channel at once, marked as recording");
    join.className = "vcopy";
    parts.push(
      field("server", "Server", "mumble.example.com:64738"),
      field("password", "Password", "if it has one", "password"),
      field("channel", "Channel", "its name"),
      el("div", { className: "vrow vinvite" }, join),
    );
    if (refused) parts.push(el("div", { className: "vhint vstatus" }, refused));
  }
  return parts;
}

/** @type {HTMLElement} */
let list;
const show = () => list.hidden || renderList(list);
const ask = (/** @type {object} */ body) =>
  fetch(new URL("voice-mumble", HOME), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });

// Where the bot is in Mumble: asked while the view is open.
function look() {
  if (!given || !open || list.hidden) return;
  fetch(new URL("voice-mumble", HOME))
    .then(r => (r.status === 200 ? r.json() : null))
    .then(
      now => {
        const changed = Boolean(now) !== Boolean(asked);
        asked = now;
        const line = list.querySelector("#vmumble");
        // The line alone where that will do: the view is not drawn anew under someone typing.
        if (!changed && line && now) line.textContent = now.status;
        else if (changed) show();
      },
      () => {},
    );
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

// The bot asked into the Mumble channel typed in; why not, when stv.tf will not have it.
function joinMumble() {
  const server = serverOf(mumble.server);
  refused = "";
  if (!server) refused = "That is no server: its name, and its port if it is not Mumble's own.";
  else if (!mumble.channel.trim()) refused = "Which channel?";
  if (!server || refused) return void show();
  asked = { ...server, channel: mumble.channel.trim(), status: "on its way" };
  show();
  const failed = (/** @type {string} */ why) => {
    asked = null;
    refused = why;
    show();
  };
  ask({ ...server, password: mumble.password, channel: mumble.channel.trim(), delay }).then(
    r => (r.ok ? look() : r.text().then(why => failed(why || "stv.tf would not have it."))),
    () => failed("stv.tf did not answer."),
  );
}

/** @param {string | undefined} does What the button clicked does. */
function clicked(does) {
  switch (does) {
    case "open":
    case "close":
      open = does === "open";
      setListView(open ? view : null);
      show();
      return look();
    case "settings":
      settings = !settings;
      return void show();
    case "copy":
      return copyCommand();
    case "join":
      return joinMumble();
    case "leave":
      asked = null;
      show();
      return void ask({ leave: true });
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
  list.addEventListener("click", e => {
    const b = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[data-invite]"));
    if (b && given) clicked(b.dataset.invite);
  });
  list.addEventListener("input", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    const f = /** @type {keyof typeof mumble | undefined} */ (input.dataset.mumble);
    if (f) mumble[f] = input.value;
  });
  list.addEventListener("change", e => {
    const input = /** @type {HTMLInputElement} */ (e.target);
    if (input.dataset.invite !== "delay") return;
    const n = Math.round(Number(input.value));
    delay = Number.isFinite(n) ? Math.max(0, Math.min(MAX_DELAY, n)) : USUAL_DELAY;
    input.blur();
    show();
  });
}
