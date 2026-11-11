// At the end of the speakers list (panel/voicelist.js), for whoever started the match and for
// casters: a button that copies the command that brings the voice bot into a Discord voice
// channel for this match, to paste there or pass on to a team. It carries the match's invite, without which no voice
// comes in. See the viewer's Voice.
import { $ } from "../core/core.js";
import { HOME } from "../room/home.js";
import { copyText } from "../room/copy.js";
import { addListExtra, renderList } from "./voicelist.js";

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
// Whether the command was just copied, and whether the gear's settings show.
let copied = false,
  settings = false;

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

// Where the bot is added to a Discord server, said as `text`.
/** @param {string} bot @param {string} text */
const addBot = (bot, text) =>
  el("a", { href: bot, target: "_blank", rel: "noopener", className: "vrow vinvite" }, text);

// One button that copies the command (never shown: it is long), and a gear for what is seldom
// needed: the delay, and the way to the bot. Nothing comes and goes around the button: the list
// opens upward, so it would move under the pointer.
function rows() {
  if (!given) return [];
  const head = el("div", { className: "vteam t-none" }, el("span", { className: "vtname" }, "Discord voice"));
  const copy = el("button", { className: "vcopy" }, copied ? "✓ Now paste it in Discord" : "Copy Discord command");
  copy.dataset.invite = "copy";
  copy.title =
    "The /stv join command for this match: paste it in Discord, in the server of the voice channel you are in";
  const gear = el("button", { className: "vgear" }, "⚙");
  gear.dataset.invite = "settings";
  gear.title = "Voice delay, and adding the bot";
  gear.setAttribute("aria-expanded", String(settings));
  /** @type {HTMLElement[]} */
  const parts = [head, el("div", { className: "vrow vinvite" }, copy, gear)];
  if (settings) {
    const secs = el("input", { type: "number", min: "0", max: String(MAX_DELAY), value: String(delay) });
    secs.dataset.invite = "delay";
    parts.push(
      el("label", { className: "vrow vinvite" }, "Voice delay", secs, "s"),
      el("div", { className: "vhint" }, "The server's tv_delay. 0 for casters talking over the stream."),
    );
    if (given.bot) parts.push(addBot(given.bot, "Add the bot to a Discord server ↗"));
  }
  return parts;
}

export function voiceInvite() {
  const list = $("voicelist");
  addListExtra(rows);
  const show = () => list.hidden || renderList(list);
  fetch(new URL("voice-invite", HOME))
    .then(r => (r.status === 200 ? r.json() : null))
    .then(
      g => {
        given = g?.invite ? { invite: String(g.invite), bot: typeof g.bot === "string" ? g.bot : null } : null;
        show();
      },
      () => {},
    );
  let timer = 0;
  list.addEventListener("click", e => {
    const b = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[data-invite]"));
    if (!b || !given) return;
    if (b.dataset.invite === "settings") {
      settings = !settings;
      return void show();
    }
    if (b.dataset.invite !== "copy") return;
    copyText(joinCommand(HOME, given.invite, delay)).then(ok => {
      if (!ok) return;
      copied = true;
      show();
      clearTimeout(timer);
      timer = window.setTimeout(() => {
        copied = false;
        show();
      }, COPIED_MS);
    });
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
