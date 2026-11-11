// At the end of the speakers list (panel/voicelist.js), for whoever started the match and for
// casters: the command that brings the voice bot into a Discord voice channel for this match,
// to paste there or pass on to a team. It carries the match's invite, without which no voice
// comes in. See the viewer's Voice.
import { $ } from "../core/core.js";
import { HOME } from "../room/home.js";
import { copyText } from "../room/copy.js";
import { addListExtra, renderList } from "./voicelist.js";

// What most of competitive TF2 sets `tv_delay` to, in seconds: the bot's own when none is said.
const USUAL_DELAY = 90,
  MAX_DELAY = 300;
// How long the button says it copied, in milliseconds.
const COPIED_MS = 1500;

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

function rows() {
  if (!given) return [];
  const head = document.createElement("div");
  head.className = "vteam t-none";
  head.append(Object.assign(document.createElement("span"), { className: "vtname", textContent: "Discord voice" }));
  const command = joinCommand(HOME, given.invite, delay);
  const line = document.createElement("div");
  line.className = "vrow vinvite";
  const text = Object.assign(document.createElement("input"), { type: "text", readOnly: true, value: command });
  text.title = "Paste this in Discord, in the server of the voice channel you are in";
  text.setAttribute("aria-label", "The command that brings the voice bot in");
  const copy = Object.assign(document.createElement("button"), { textContent: "Copy" });
  copy.dataset.invite = "copy";
  line.append(text, copy);
  const set = document.createElement("label");
  set.className = "vrow vinvite";
  const secs = Object.assign(document.createElement("input"), { type: "number", min: "0", max: String(MAX_DELAY) });
  secs.value = String(delay);
  secs.dataset.invite = "delay";
  set.title = "The server's tv_delay: how far its SourceTV is behind the game. 0 for casters talking over the stream";
  set.append("SourceTV delay", secs, "s");
  /** @type {HTMLElement[]} */
  const parts = [head, line, set];
  if (given.bot) {
    const add = Object.assign(document.createElement("a"), { href: given.bot, target: "_blank", rel: "noopener" });
    add.textContent = "Add the bot to a Discord server";
    add.className = "vrow vinvite";
    parts.push(add);
  }
  return parts;
}

export function voiceInvite() {
  const list = $("voicelist");
  addListExtra(rows);
  const show = () => list.hidden || renderList(list);
  fetch(new URL("voice-invite", HOME))
    .then(r => (r.ok ? r.json() : null))
    .then(
      g => {
        given = g?.invite ? { invite: String(g.invite), bot: typeof g.bot === "string" ? g.bot : null } : null;
        show();
      },
      () => {},
    );
  list.addEventListener("click", e => {
    const b = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[data-invite=copy]"));
    if (!b || !given) return;
    copyText(joinCommand(HOME, given.invite, delay)).then(ok => {
      if (!ok) return;
      b.textContent = "Copied";
      setTimeout(() => (b.textContent = "Copy"), COPIED_MS);
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
