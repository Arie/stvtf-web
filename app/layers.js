import { $, OVERLAY } from "./core.js";
import { openStream } from "./stream.js";

// Overkill layers; `detail` ones need the hub's high-volume lines (?detail=1). All on by default; the overlay shows only caps.
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
const layerState = {};
for (const [name, l] of Object.entries(LAYERS)) {
  layerState[name] = OVERLAY ? name === "caps" : l.def;
  try { const v = !OVERLAY && localStorage.getItem("layer." + name); if (v === "1" || v === "0") layerState[name] = v === "1"; } catch {}
}
export const layerOn = name => layerState[name];
export const detailNeeded = () => Object.entries(LAYERS).some(([n, l]) => l.detail && layerState[n]);
export function setLayer(name, on) {
  const had = detailNeeded();
  layerState[name] = on;
  try { localStorage.setItem("layer." + name, on ? "1" : "0"); } catch {}
  renderSettings();
  if (had !== detailNeeded()) openStream();
}
export function setOverkill(on) {
  const had = detailNeeded();
  for (const name of Object.keys(LAYERS)) {
    layerState[name] = on;
    try { localStorage.setItem("layer." + name, on ? "1" : "0"); } catch {}
  }
  renderSettings();
  if (had !== detailNeeded()) openStream();
}
export const overkillOn = () => Object.keys(LAYERS).every(layerOn);
export function renderSettings() {
  const box = $("settings");
  if (!box) return;
  const row = (id, label, key, on, cls) => `<label class="${cls}"><input type="checkbox" data-layer="${id}"${on ? " checked" : ""}>${label}${key ? `<kbd>${key}</kbd>` : ""}</label>`;
  box.innerHTML = row("*", "Overkill (all)", "x", overkillOn(), "master") +
    Object.entries(LAYERS).map(([n, l]) => row(n, l.label, l.key, layerState[n], "")).join("");
}
