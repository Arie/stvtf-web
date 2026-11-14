// The voice button (by Overview/Geometry) and its speakers list (opening upward, see
// panel/voicelist.js), and who talks on the HUD cards: for anyone heard a faint 🔊 (🔇 while
// muted), and while they talk the waveform of their dot's badge (map/voicebadge.js); a click on it
// mutes or unmutes them (elsewhere on the card, it still follows them). See core/voice.js.
import { $, state } from "../core/core.js";
import { env } from "../core/env.js";
import { openStream } from "../core/stream.js";
import * as voice from "../core/voice.js";
import { UNKNOWN, bars, ease, heat } from "../map/voicebadge.js";
import { chooseIn, markTalking, renderList } from "./voicelist.js";
import { voiceInvite } from "./voiceinvite.js";
import { voiceDemos } from "./voicedemos.js";

// How often the cards and the list show who talks, in milliseconds.
const REFRESH_MS = 150;
// How much bigger a card's mark is for someone at their loudest.
const GROW = 0.4;

// How loud each card's waveform shows its player, eased.
/** @type {Map<number, number>} */
const eased = new Map();
// Whether the waveforms are being drawn, frame by frame.
let waving = false;

export function initVoice() {
  const button = /** @type {HTMLButtonElement} */ ($("voicetoggle")),
    list = $("voicelist");
  const why = voice.voiceUnsupported();
  if (why) {
    Object.assign(button, { disabled: true, title: why });
    return;
  }
  const show = () => {
    // On from an earlier visit, a browser still plays nothing before a click: shown as off, and asking for one.
    const waiting = voice.voiceWaiting();
    button.textContent = voice.voiceOn() && !waiting ? "🔊" : "🔇";
    button.title = waiting ? "Click to hear" : "The players' voice";
    button.classList.toggle("on", voice.voiceOn() && !waiting);
    button.classList.toggle("wait", waiting);
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
  voiceDemos();
  for (const id of ["hud-blue", "hud-red"]) cardClicks($(id));
  setInterval(() => {
    cards();
    if (!list.hidden) markTalking(list);
  }, REFRESH_MS);
  show();
}

// Each HUD card's voice mark: for anyone heard so far a faint 🔊, a waveform while they talk, and
// 🔇 while muted; a click on it mutes or unmutes them, without waiting for them to talk.
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
  const wave = Boolean(now?.talking && !now.muted);
  if (wave !== mark.classList.contains("wave")) waveMark(mark, Number(card.dataset.id), wave);
  if (!wave) markText(mark, now);
  mark.classList.toggle("muted", Boolean(now?.muted));
  // Faint while silent, lit while talking, muted or not.
  mark.classList.toggle("quiet", !now?.talking);
  mark.title = now?.muted ? "Muted: click to unmute" : "Click to mute";
}

/** @param {HTMLElement} mark @param {{ muted: boolean } | null} now */
function markText(mark, now) {
  let text = "";
  if (now) text = now.muted ? "🔇" : "🔊";
  if (mark.textContent !== text) mark.textContent = text;
}

// Makes player `id`'s `mark` a waveform (its bars, drawn frame by frame from now), or what it was.
/** @param {HTMLElement} mark @param {number} id @param {boolean} wave */
function waveMark(mark, id, wave) {
  mark.classList.toggle("wave", wave);
  mark.replaceChildren(...(wave ? bars(0, 0).map(() => document.createElement("i")) : []));
  mark.removeAttribute("style");
  if (!wave) eased.delete(id);
  if (wave && !waving) {
    waving = true;
    requestAnimationFrame(waves);
  }
}

// The cards' waveforms, each frame while anyone's shows: taller, bigger and hotter the louder they
// are, as on their dot.
/** @param {number} t */
function waves(t) {
  const marks = /** @type {NodeListOf<HTMLElement>} */ (env.root.querySelectorAll(".card > .vc.wave"));
  waving = marks.length > 0;
  if (!waving) return;
  for (const mark of marks) {
    const id = Number(/** @type {HTMLElement} */ (mark.parentElement).dataset.id);
    const level = ease(eased, id, voice.loudnessOf(id) ?? UNKNOWN);
    const colour = heat(level);
    mark.style.color = colour;
    mark.style.borderColor = level > 0.55 ? colour : "";
    mark.style.transform = `scale(${(1 + GROW * level).toFixed(3)})`;
    bars(level, t).forEach((h, i) => {
      /** @type {HTMLElement} */ (mark.children[i]).style.height = `${Math.round(h * 75)}%`;
    });
  }
  requestAnimationFrame(waves);
}

// A click on a card's voice mark mutes or unmutes its player, and follows nobody.
/** @param {HTMLElement} hud */
function cardClicks(hud) {
  hud.addEventListener(
    "click",
    e => {
      const mark = /** @type {Element} */ (e.target).closest?.(".vc");
      if (!mark?.hasChildNodes()) return;
      e.stopPropagation();
      const steamid = state.players.get(Number(/** @type {HTMLElement} */ (mark.closest(".card")).dataset.id))?.steamid;
      if (steamid) voice.toggleMuted(steamid);
    },
    { capture: true },
  );
}
