// At the end of the speakers list (panel/voicelist.js): the match's demos to download with the
// comms in them, as the players' voice, once anything was said from outside it (a team's
// Discord). One a map, or more when the connection to its server dropped. See the viewer's Voice.
import { $ } from "../core/core.js";
import { HOME } from "../room/home.js";
import { addListExtra, renderList } from "./voicelist.js";

// How often the list is asked for while the speakers list is open, in milliseconds.
const ASK_MS = 30_000;

/** @typedef {{ map: string; mb: number; href: string }} Demo */
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
      a.textContent = `${d.map} (${d.mb} MB) ↓`;
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
  setInterval(ask, ASK_MS);
  $("voicetoggle").addEventListener("click", () => setTimeout(ask));
}
