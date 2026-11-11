// The voice button (by Overview/Geometry) and its speakers list (opening upward, see
// panel/voicelist.js), and who talks on the HUD cards: for anyone heard a 🔊 (🔇 while muted),
// faint while silent and lit while they talk; a click on it mutes or unmutes them (elsewhere on
// the card, it still follows them). See core/voice.js.
import { $, canvas, state } from "../core/core.js";
import { voiceBadgeAt } from "../map/voicebadge.js";
import { env } from "../core/env.js";
import { openStream } from "../core/stream.js";
import * as voice from "../core/voice.js";
import { chooseIn, markTalking, renderList } from "./voicelist.js";
import { voiceInvite } from "./voiceinvite.js";

// How often the cards and the list show who talks, in milliseconds.
const REFRESH_MS = 150;

export function initVoice() {
  const button = /** @type {HTMLButtonElement} */ ($("voicetoggle")),
    list = $("voicelist");
  const why = voice.voiceUnsupported();
  if (why) {
    Object.assign(button, { disabled: true, title: why });
    return;
  }
  const show = () => {
    button.textContent = voice.voiceOn() ? "🔊" : "🔇";
    button.classList.toggle("on", voice.voiceOn());
    if (!list.hidden) renderList(list);
  };
  button.onclick = () => {
    if (!voice.voiceOn()) {
      voice.setVoice(true);
      openStream();
    }
    list.hidden = !list.hidden;
    show();
  };
  list.addEventListener("click", e => {
    if (!chooseIn(/** @type {Element} */ (e.target))) return;
    list.hidden = true;
    openStream();
  });
  list.addEventListener("input", e => {
    const range = /** @type {HTMLInputElement} */ (e.target);
    if (range.dataset.volume) voice.setVolume(range.dataset.volume, Number(range.value) / 100);
  });
  env.root.addEventListener("pointerdown", e => {
    const at = /** @type {Element} */ (e.composedPath()[0]);
    if (!list.hidden && !at.closest?.("#voicelist, #voicetoggle")) list.hidden = true;
  });
  voice.voiceListeners.add(show);
  voiceInvite();
  for (const id of ["hud-blue", "hud-red"]) cardClicks($(id));
  badgeClicks();
  setInterval(() => {
    cards();
    if (!list.hidden) markTalking(list);
  }, REFRESH_MS);
  show();
}

// Each HUD card's voice mark: for anyone heard so far a faint 🔊, full while they talk, and 🔇
// while muted; a click on it mutes or unmutes them, without waiting for them to talk.
function cards() {
  for (const card of /** @type {NodeListOf<HTMLElement>} */ (
    env.root.querySelectorAll("#hud-blue .card, #hud-red .card")
  ))
    cardMark(card);
}

// How a player's voice shows: muted or heard (and so there to mute), and whether they talk now;
// none for one never heard and not muted.
/** @param {number} id @returns {{ muted: boolean; talking: boolean } | null} */
function voiceState(id) {
  const steamid = state.players.get(id)?.steamid;
  if (!steamid || !voice.voiceOn()) return null;
  const muted = voice.isMuted(steamid);
  if (!muted && !voice.hasSpoken(steamid)) return null;
  return { muted, talking: voice.isTalking(id) };
}

/** @param {HTMLElement} card */
function cardMark(card) {
  const now = voiceState(Number(card.dataset.id));
  let mark = /** @type {HTMLElement | null} */ (card.querySelector(":scope > .vc"));
  if (!mark && !now) return;
  if (!mark) {
    mark = Object.assign(document.createElement("span"), { className: "vc" });
    card.append(mark);
  }
  let text = "";
  if (now) text = now.muted ? "🔇" : "🔊";
  if (mark.textContent !== text) mark.textContent = text;
  mark.classList.toggle("muted", Boolean(now?.muted));
  // Faint while silent, lit while talking, muted or not.
  mark.classList.toggle("quiet", !now?.talking);
  mark.title = now?.muted ? "Muted: click to unmute" : "Click to mute";
}

// A click on a dot's voice badge mutes or unmutes its player, and neither follows nor drags (but a
// drawing tool in hand draws).
function badgeClicks() {
  canvas.addEventListener(
    "mousedown",
    e => {
      if (env.el.classList.contains("drawing")) return;
      const r = canvas.getBoundingClientRect();
      const b = voiceBadgeAt(e.clientX - r.left, e.clientY - r.top);
      if (!b) return;
      e.stopImmediatePropagation();
      e.preventDefault();
      voice.toggleMuted(b.steamid);
    },
    { capture: true },
  );
}

// A click on a card's voice mark mutes or unmutes its player, and follows nobody.
/** @param {HTMLElement} hud */
function cardClicks(hud) {
  hud.addEventListener(
    "click",
    e => {
      const mark = /** @type {Element} */ (e.target).closest?.(".vc");
      if (!mark?.textContent) return;
      e.stopPropagation();
      const steamid = state.players.get(Number(/** @type {HTMLElement} */ (mark.closest(".card")).dataset.id))?.steamid;
      if (steamid) voice.toggleMuted(steamid);
    },
    { capture: true },
  );
}
