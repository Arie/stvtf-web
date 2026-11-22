// The Mumble set-up of the comms view (voiceinvite.js): the server, its password and the channel,
// kept as typed while the view is drawn anew.
import { act, el, labelOf } from "./voicebits.js";

// Mumble's own port.
export const MUMBLE_PORT = 64738;

/** @typedef {{ id: number; host: string; port: number; channel: string; delay: number; status: string; state?: string }} InMumble */

// A Mumble server as someone writes it: its name or address, with a port or without, or a
// mumble:// link. Null when the text is not a server.
/** @param {string} text @returns {{ host: string; port: number } | null} */
export function serverOf(text) {
  const m = /^(?:mumble:\/\/)?(?:[^@/\s]*@)?([A-Za-z0-9.-]+)(?::(\d{1,5}))?(?:\/\S*)?$/.exec(text.trim());
  if (!m) return null;
  const port = m[2] ? Number(m[2]) : MUMBLE_PORT;
  return port >= 1 && port <= 65535 ? { host: m[1], port } : null;
}

// What is typed into the form.
export const typed = { server: "", password: "", channel: "" };

// A server as it is typed: its port only when it is not Mumble's own.
const serverText = (/** @type {InMumble} */ m) => (m.port === MUMBLE_PORT ? m.host : `${m.host}:${m.port}`);

// Fills the form for moving `m` with where it is.
/** @param {InMumble} m */
export function fillFor(m) {
  Object.assign(typed, { server: serverText(m), password: "", channel: m.channel });
}

// Fills the form for another channel: most likely on the server of `last`, the bot's latest channel.
/** @param {InMumble | undefined} last */
export function fillAfter(last) {
  Object.assign(typed, { server: last ? serverText(last) : "", password: "", channel: "" });
}

/** @param {keyof typeof typed} name @param {string} label @param {string} hint @param {string} [type] */
function field(name, label, hint, type = "text") {
  const input = el("input", { type, value: typed[name], placeholder: hint, autocomplete: "off", spellcheck: false });
  input.dataset.mumble = name;
  return el("label", { className: "vrow vinvite vfield" }, labelOf(label), input);
}

// The form's fields. For a server the bot is on already (`known`), an empty password keeps the old one.
/** @param {boolean} known */
export function mumbleFields(known) {
  return [
    field("server", "Server", "host:port"),
    field("password", "Password", known ? "unchanged" : "optional", "password"),
    field("channel", "Channel", ""),
  ];
}

// Connect, or for a channel the bot is in (`moving`) Reconnect and Disconnect.
/** @param {boolean} moving */
export function mumbleButtons(moving) {
  const join = act(moving ? "Reconnect" : "Connect", "join");
  join.className = "vcopy vmumble";
  const buttons = el("div", { className: "vrow vinvite" }, join);
  if (moving) {
    const leave = act("Disconnect", "leave");
    leave.className = "vcopy vleave";
    buttons.append(leave);
  }
  return buttons;
}

// The request that asks the bot into the channel typed in, as channel `id` (null: one more), `delay`
// seconds behind; or why it cannot be made. With no password typed, the one given before for that
// server is used.
/** @param {number | null} id @param {number} delay @returns {{ body: object } | { why: string }} */
export function mumbleRequest(id, delay) {
  const server = serverOf(typed.server);
  if (!server) return { why: "Invalid server" };
  const channel = typed.channel.trim();
  if (!channel) return { why: "Channel name required" };
  return { body: { ...(id == null ? {} : { id }), ...server, password: typed.password || undefined, channel, delay } };
}

// Keeps what is typed into one of the form's fields.
/** @param {HTMLInputElement} input */
export function keepTyped(input) {
  const f = /** @type {keyof typeof typed | undefined} */ (input.dataset.mumble);
  if (f) typed[f] = input.value;
}
