// At the end of the speakers list (panel/voicelist.js): the match's demos to download with the
// comms in them, as the players' voice, once anything was said from outside it (a team's
// Discord). One a map, or more when the connection to its server dropped. See the viewer's Voice.
import { $ } from "../core/core.js";
import { HOME } from "../room/home.js";
import { addListExtra, renderList } from "./voicelist.js";

// How often the list is asked for while the speakers list is open, in milliseconds.
const ASK_MS = 30_000;

// How often a demo asked for is looked for, and how long before giving up on it, in milliseconds.
const BUILT_MS = 200,
  GIVE_UP_MS = 60_000;

/** @typedef {{ map: string; mb: number; href: string }} Demo */
// Demos being built, by link: stv.tf sets a cookie of the mark each was asked with once it is.
/** @type {Set<string>} */
const building = new Set();
/** @type {Demo[]} */
let demos = [];

function rows() {
  if (!demos.length) return [];
  const head = document.createElement("div");
  head.className = "vteam t-none";
  head.title = "For playdemo";
  head.append(Object.assign(document.createElement("span"), { className: "vtname", textContent: "Demos with comms" }));
  return [
    head,
    ...demos.map(d => {
      const a = Object.assign(document.createElement("a"), { href: d.href, className: "vrow vdemo", download: "" });
      a.dataset.demo = d.href;
      a.textContent = building.has(d.href) ? `${d.map} · building…` : `${d.map} (${d.mb} MB) ↓`;
      return a;
    }),
  ];
}

export function voiceDemos() {
  const list = $("voicelist");
  addListExtra(rows);
  const ask = () => {
    if (list.hidden) return;
    fetch(new URL("demos", HOME))
      .then(r => (r.ok ? r.json() : []))
      .then(
        (/** @type {Demo[]} */ found) => {
          if (JSON.stringify(found) === JSON.stringify(demos)) return;
          demos = found;
          if (!list.hidden) renderList(list);
        },
        () => {},
      );
  };
  const show = () => list.hidden || renderList(list);
  list.addEventListener("click", e => {
    const a = /** @type {HTMLAnchorElement | null} */ (/** @type {Element} */ (e.target).closest("a.vdemo"));
    const href = a?.dataset.demo;
    if (!a || !href) return;
    if (building.has(href)) return void e.preventDefault();
    const mark = Math.random().toString(36).slice(2, 12);
    a.href = `${href}?built=${mark}`;
    building.add(href);
    // After the click went its way: the row is drawn anew.
    setTimeout(show);
    const since = Date.now();
    const timer = setInterval(() => {
      if (!document.cookie.includes(`built=${mark}`) && Date.now() - since < GIVE_UP_MS) return;
      clearInterval(timer);
      building.delete(href);
      document.cookie = "built=; Path=/; Max-Age=0";
      show();
    }, BUILT_MS);
  });
  setInterval(ask, ASK_MS);
  $("voicetoggle").addEventListener("click", () => setTimeout(ask));
}
