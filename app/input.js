import { $, OVERLAY, canvas, uiScale, applyScale, state } from "./core.js";
import { layerOn, setLayer, setOverkill, overkillOn } from "./layers.js";
import { viewCy, frameMap, toScreen } from "./view.js";

// A resize (or hiding the events) only refits the whole-map framing; auto zoom glides to it.
addEventListener("resize", () => { applyScale(); if (state.framed) frameMap(true); });
let drag = null;
canvas.addEventListener("mousedown", e => drag = { x: e.clientX, y: e.clientY, cx: state.view.cx, cy: state.view.cy, moved: false });
window.addEventListener("mouseup", e => {
  if (drag && !drag.moved) followAt(e.clientX, e.clientY);
  drag = null;
});
window.addEventListener("mousemove", e => {
  if (!drag) return;
  if (!drag.moved && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 4) return;
  if (!drag.moved) { drag.moved = true; setFollow(null); setAutoZoom(false); }
  state.view.cx = drag.cx - (e.clientX - drag.x) / state.view.scale;
  state.view.cy = drag.cy + (e.clientY - drag.y) / state.view.scale;
});
// h: player HUD and scores, s: just the scores, e: events and chat, g: overview or geometry,
// o: the overlay or back.
function toggle(cls) {
  const on = document.documentElement.classList.toggle(cls);
  try { localStorage.setItem(cls, on ? "1" : "0"); } catch {}
  dispatchEvent(new Event("resize"));
}
function toggleExclusive(id) {
  const other = $(id === "help" ? "settings" : "help");
  other.hidden = true;
  $(id).hidden = !$(id).hidden;
}
// Hold Tab for the scoreboard, as in TF2.
function showBoard(on) {
  if (OVERLAY || $("board").hidden === !on) return;
  $("board").hidden = !on;
  state.lastPanel = 0;
}
window.addEventListener("keyup", e => { if (e.key === "Tab") showBoard(false); });
window.addEventListener("blur", () => showBoard(false));
window.addEventListener("keydown", e => {
  if (e.ctrlKey || e.metaKey || e.altKey || e.target.closest?.("input:not([type=checkbox]), textarea")) return;
  if (e.key === "Tab") { e.preventDefault(); showBoard(true); return; }
  switch (e.key) {
    case "Escape": setFollow(null); $("help").hidden = true; $("settings").hidden = true; break;
    case "?": toggleExclusive("help"); break;
    case "k": if (!OVERLAY) toggleExclusive("settings"); break;
    case "x": if (!OVERLAY) setOverkill(!overkillOn()); break;
    case "j": if (!OVERLAY) setLayer("jumps", !layerOn("jumps")); break;
    case "l": if (!OVERLAY) setLayer("loadout", !layerOn("loadout")); break;
    case "h": toggle("nohud"); break;
    case "s": toggle("noscore"); break;
    case "e": setSidebar(document.documentElement.classList.contains("noside")); break;
    case "g": if (!$("bgtoggle").hidden) document.querySelector("#bgtoggle button:not(.on)").click(); break;
    case "o": location.replace(OVERLAY ? location.pathname.replace(/overlay\/?$/, "") : location.pathname.replace(/(index\.html)?$/, "overlay")); break;
  }
});
for (const cls of ["nohud", "noscore"]) {
  try { if (!OVERLAY && localStorage.getItem(cls) === "1") document.documentElement.classList.add(cls); } catch {}
}

function setAutoZoom(on) {
  if (OVERLAY || state.autoZoom === on) return;
  state.autoZoom = on;
  state.camTarget = null; state.cam = null;
  $("autozoom").hidden = on;
}

// Click a dot on the map or a card in the HUD to keep that player centred.
export function setFollow(id) {
  state.follow = id != null && id !== state.follow ? id : null;
  const p = state.lastPlayers.find(x => x.id === state.follow);
  $("follow").hidden = !p;
  if (p) $("follow-name").textContent = p.name;
  state.lastPanel = 0;
}
function followAt(cx, cy) {
  const r = canvas.getBoundingClientRect();
  let best = null, bestD = 18 * uiScale();
  for (const p of state.lastPlayers) {
    if (!p.alive) continue;
    const [x, y] = toScreen(p.pos[0], p.pos[1]);
    const d = Math.hypot(x - (cx - r.left), y - (cy - r.top));
    if (d < bestD) { best = p; bestD = d; }
  }
  if (best) setFollow(best.id);
}
for (const id of ["hud-blue", "hud-red"]) {
  $(id).addEventListener("click", e => {
    const c = e.target.closest(".card");
    if (c) setFollow(Number(c.dataset.id));
  });
}
$("follow-stop").addEventListener("click", () => setFollow(null));
$("settingstoggle").addEventListener("click", () => toggleExclusive("settings"));
$("settings").addEventListener("change", e => {
  const id = e.target.dataset.layer;
  if (id === "*") setOverkill(e.target.checked); else if (id) setLayer(id, e.target.checked);
});

$("helptoggle").addEventListener("click", () => toggleExclusive("help"));
$("autozoom").addEventListener("click", () => setAutoZoom(true));
$("feed").addEventListener("click", e => {
  const id = Number(e.target.closest(".who[data-id]")?.dataset.id);
  if (!OVERLAY && Number.isFinite(id) && id !== state.follow) setFollow(id);
});

function setSidebar(show) {
  document.documentElement.classList.toggle("noside", !show);
  $("sidetoggle").textContent = show ? "›" : "‹";
  $("sidetoggle").title = show ? "Hide events and chat (e)" : "Show events and chat (e)";
  try { localStorage.setItem("sidebar", show ? "1" : "0"); } catch {}
  dispatchEvent(new Event("resize"));
}
$("sidetoggle").addEventListener("click", () => setSidebar(document.documentElement.classList.contains("noside")));
try { if (!OVERLAY && localStorage.getItem("sidebar") === "0") setSidebar(false); } catch {}
for (const b of document.querySelectorAll("#bgtoggle button")) {
  b.addEventListener("click", () => {
    state.bg = b.dataset.bg;
    for (const o of document.querySelectorAll("#bgtoggle button")) o.classList.toggle("on", o === b);
  });
}
canvas.addEventListener("wheel", e => {
  e.preventDefault();
  setAutoZoom(false);
  const r = canvas.getBoundingClientRect(), v = state.view;
  const mx = e.clientX - r.left - r.width / 2, my = e.clientY - r.top - r.height / 2;
  const wx = v.cx + mx / v.scale, wy = v.cy - my / v.scale;
  v.scale *= Math.exp(-e.deltaY * 0.0015);
  v.cx = wx - mx / v.scale; v.cy = wy + my / v.scale;
  // No further out than the whole map, framed as at the start.
  const full = state.full;
  if (full && v.scale <= full.scale) Object.assign(v, { scale: full.scale, cx: full.cx, cy: viewCy(full.cy, full.scale) });
}, { passive: false });
