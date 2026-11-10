// The voice button (by Overview/Geometry), its speakers list (opening upward: voice on or off,
// mute all, and everyone heard, each with a mute and a volume), and who talks on the HUD cards: a
// 🔊 while they talk, a 🔇 while muted; a click on it mutes or unmutes them (elsewhere on the card,
// it still follows them). See core/voice.js.
import { $, state } from "../core/core.js";
import { env } from "../core/env.js";
import { openStream } from "../core/stream.js";
import * as voice from "../core/voice.js";

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
    if (!list.hidden) render(list);
  };
  button.onclick = () => {
    if (!voice.voiceOn()) {
      voice.setVoice(true);
      openStream();
    }
    list.hidden = !list.hidden;
    show();
  };
  list.addEventListener("click", e => choose(list, /** @type {Element} */ (e.target)));
  list.addEventListener("input", e => {
    const range = /** @type {HTMLInputElement} */ (e.target);
    if (range.dataset.volume) voice.setVolume(range.dataset.volume, Number(range.value) / 100);
  });
  env.root.addEventListener("pointerdown", e => {
    const at = /** @type {Element} */ (e.composedPath()[0]);
    if (!list.hidden && !at.closest?.("#voicelist, #voicetoggle")) list.hidden = true;
  });
  voice.voiceListeners.add(show);
  for (const id of ["hud-blue", "hud-red"]) cardClicks($(id));
  setInterval(() => {
    cards();
    if (!list.hidden) talkingMarks(list);
  }, REFRESH_MS);
  show();
}

// What a click in the list does.
/** @param {HTMLElement} list @param {Element} at */
function choose(list, at) {
  const b = /** @type {HTMLButtonElement | null} */ (at.closest("button"));
  if (!b) return;
  if (b.dataset.act === "off") {
    voice.setVoice(false);
    list.hidden = true;
    openStream();
  } else if (b.dataset.act === "all") {
    const heard = voice.speakersHeard();
    const mute = heard.some(s => !voice.isMuted(s.steamid));
    for (const s of heard) voice.setMuted(s.steamid, mute);
  } else if (b.dataset.mute) voice.setMuted(b.dataset.mute, !voice.isMuted(b.dataset.mute));
}

/** @param {number | null} player @param {string} steamid */
const nameOf = (player, steamid) => (player == null ? null : state.players.get(player)?.name) ?? steamid;

/** @param {HTMLElement} list */
function render(list) {
  const heard = voice.speakersHeard();
  const rows = heard.map(s => {
    const row = document.createElement("div");
    row.className = "vrow";
    row.dataset.steamid = s.steamid;
    const mute = Object.assign(document.createElement("button"), {
      textContent: voice.isMuted(s.steamid) ? "🔇" : "🔊",
    });
    mute.dataset.mute = s.steamid;
    mute.title = voice.isMuted(s.steamid) ? "Unmute" : "Mute";
    const name = Object.assign(document.createElement("span"), {
      className: "vname",
      textContent: nameOf(s.player, s.steamid),
    });
    const range = Object.assign(document.createElement("input"), { type: "range", min: "0", max: "100" });
    range.value = String(Math.round(voice.volumeOf(s.steamid) * 100));
    range.dataset.volume = s.steamid;
    range.setAttribute("aria-label", `Volume of ${name.textContent}`);
    row.append(mute, name, range);
    return row;
  });
  const head = document.createElement("div");
  head.className = "vhead";
  const off = Object.assign(document.createElement("button"), { textContent: "Voice off" });
  off.dataset.act = "off";
  head.append(off);
  if (heard.length) {
    const all = Object.assign(document.createElement("button"), {
      textContent: heard.some(s => !voice.isMuted(s.steamid)) ? "Mute all" : "Unmute all",
    });
    all.dataset.act = "all";
    head.append(all);
  }
  const empty = Object.assign(document.createElement("div"), {
    className: "vempty",
    textContent: "Nobody has talked yet",
  });
  list.replaceChildren(head, ...(rows.length ? rows : [empty]));
  talkingMarks(list);
}

/** @param {HTMLElement} list */
function talkingMarks(list) {
  const heard = voice.speakersHeard();
  if (list.querySelectorAll(".vrow").length !== heard.length) return render(list);
  for (const s of heard)
    list.querySelector(`.vrow[data-steamid="${CSS.escape(s.steamid)}"]`)?.classList.toggle("talking", s.talking);
}

// Each HUD card's voice mark, as its player talks or is muted.
function cards() {
  for (const card of /** @type {NodeListOf<HTMLElement>} */ (
    env.root.querySelectorAll("#hud-blue .card, #hud-red .card")
  )) {
    const id = Number(card.dataset.id);
    const steamid = state.players.get(id)?.steamid;
    let mark = /** @type {HTMLElement | null} */ (card.querySelector(":scope > .vc"));
    const muted = Boolean(steamid) && voice.isMuted(steamid ?? ""),
      talking = voice.voiceOn() && voice.isTalking(id);
    if (!mark && !muted && !talking) continue;
    if (!mark) {
      mark = Object.assign(document.createElement("span"), { className: "vc" });
      card.append(mark);
    }
    let text = "";
    if (muted) text = "🔇";
    else if (talking) text = "🔊";
    if (mark.textContent !== text) mark.textContent = text;
    mark.title = muted ? "Muted: click to unmute" : "Talking: click to mute";
  }
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
      if (steamid) voice.setMuted(steamid, !voice.isMuted(steamid));
    },
    { capture: true },
  );
}
