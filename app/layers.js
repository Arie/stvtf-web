import { $, OVERLAY, optOn } from "./core.js";
import { env } from "./env.js";
import { openStream } from "./stream.js";

// Overkill layers; `detail` ones need the hub's high-volume lines (?detail=1). All on by default; the overlay shows only caps. Each name is also a query option (1 or 0).
const LAYERS = {
  caps: { label: "Capture areas", key: "", def: true, detail: false },
  heals: { label: "Heal numbers", key: "", def: true, detail: true },
  hits: { label: "Hit markers", key: "", def: true, detail: true },
  marks: { label: "Bullet holes and scorch marks", key: "", def: true, detail: true },
  jumps: { label: "Jump trails", key: "j", def: true, detail: true },
  pickups: { label: "Pickups", key: "", def: true, detail: true },
  points: { label: "Players on points", key: "", def: true, detail: false },
  flow: { label: "Votes, announcer and voice commands", key: "", def: true, detail: true },
  loadout: { label: "Weapon in hand", key: "l", def: true, detail: false },
  effects: { label: "Status effects", key: "", def: true, detail: false },
};
/** @type {{ [name: string]: boolean }} */
const layerState = {};
for (const [name, l] of Object.entries(LAYERS)) {
  layerState[name] = OVERLAY ? name === "caps" : l.def;
  try {
    const v = !OVERLAY && env.storage && localStorage.getItem("layer." + name);
    if (v === "1" || v === "0") layerState[name] = v === "1";
  } catch {}
  // The address beats the saved choice for this visit, without changing it.
  layerState[name] = optOn(name, layerState[name]);
}
export const layerOn = /** @param {string} name */ name => layerState[name];
export const detailNeeded = () => Object.entries(LAYERS).some(([n, l]) => l.detail && layerState[n]);
/** @param {string} name @param {boolean} on */
export function setLayer(name, on) {
  const had = detailNeeded();
  layerState[name] = on;
  try {
    if (env.storage) localStorage.setItem("layer." + name, on ? "1" : "0");
  } catch {}
  renderSettings();
  if (had !== detailNeeded()) openStream();
}
/** @param {boolean} on */
export function setOverkill(on) {
  const had = detailNeeded();
  for (const name of Object.keys(LAYERS)) {
    layerState[name] = on;
    try {
      if (env.storage) localStorage.setItem("layer." + name, on ? "1" : "0");
    } catch {}
  }
  renderSettings();
  if (had !== detailNeeded()) openStream();
}
export const overkillOn = () => Object.keys(LAYERS).every(layerOn);
export function renderSettings() {
  const box = $("settings");
  if (!box) return;
  /** @param {string} id @param {string} label @param {string} key @param {boolean} on @param {string} cls */
  const row = (id, label, key, on, cls) =>
    `<label class="${cls}"><input type="checkbox" data-layer="${id}"${on ? " checked" : ""}>${label}${key ? `<kbd>${key}</kbd>` : ""}</label>`;
  box.innerHTML =
    row("*", "Overkill (all)", "x", overkillOn(), "master") +
    Object.entries(LAYERS)
      .map(([n, l]) => row(n, l.label, l.key, layerState[n], ""))
      .join("");
}
