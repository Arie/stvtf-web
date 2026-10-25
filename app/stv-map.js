// <stv-map src="…"></stv-map>: the live map inside another site's page. `src` is a start link
// from serveme.tf (…/start?t=…) or a match page (…/w/<id>/). Attributes: view="overlay",
// delay="<seconds>", side="0", and the overlay's options (names, score, panel, rotate, dot, ui,
// zoom). It fires the same messages as an iframe sends, as events named by their type, and
// has follow() and setLayer(). The viewer runs once per page, so a page has one map.
import { configure } from "./env.js";

const BASE = new URL("../../", import.meta.url);
// See asset() in env.js.
const cors = url => { url.searchParams.set("cors", "1"); return url; };
const OPTIONS = ["side", "names", "score", "panel", "rotate", "dot", "ui", "zoom"];
let mounted = null;

// Chrome ignores @font-face inside a shadow root, so the HUD's font goes on the page.
function addFonts() {
  if (document.getElementById("stvtf-fonts")) return;
  const style = document.createElement("style");
  style.id = "stvtf-fonts";
  style.textContent = [["Regular", 400], ["Bold", 700]].map(([w, n]) =>
    `@font-face { font-family: "HUD Condensed"; src: url("${cors(new URL(`fonts/RobotoCondensed-${w}.ttf`, BASE))}") format("truetype"); font-weight: ${n}; }`).join("\n");
  document.head.append(style);
}

// The match's page path: a start link is asked where it leads, a match page is used as it is.
async function matchPage(src) {
  const url = new URL(src, BASE);
  if (url.pathname.endsWith("/start")) {
    url.searchParams.set("format", "json");
    const r = await fetch(url);
    if (!r.ok) throw new Error((await r.text()) || `start link: HTTP ${r.status}`);
    return (await r.json()).page;
  }
  const page = url.pathname.match(/^.*?\/w\/[^/]+\//)?.[0];
  if (!page) throw new Error(`${src} is not a start link or a match page`);
  return page;
}

const loaded = link => new Promise(resolve => { link.onload = link.onerror = resolve; });

class StvMap extends HTMLElement {
  #command = null;

  async connectedCallback() {
    if (this.shadowRoot) return;
    const shadow = this.attachShadow({ mode: "open" });
    shadow.innerHTML = `<style>
      :host { all: initial; display: block; position: relative; aspect-ratio: 16 / 9; contain: content; }
      .stvtf { position: absolute; inset: 0; height: auto; }
      .failed { display: grid; place-items: center; height: 100%; padding: 16px; background: #15171a; color: #d8dadd; font: 14px system-ui, sans-serif; }
    </style>`;
    try {
      if (mounted) throw new Error("This page already has an stv.tf map");
      mounted = this;
      addFonts();
      const page = await matchPage(this.getAttribute("src") || "");
      const html = await (await fetch(cors(new URL(page, BASE)))).text();
      const app = document.importNode(new DOMParser().parseFromString(html, "text/html").getElementById("app"), true);
      const links = [new URL("style.css", import.meta.url), cors(new URL("killicons.css", BASE))].map(href => {
        const link = document.createElement("link");
        link.rel = "stylesheet";
        link.href = href;
        return link;
      });
      shadow.append(...links, app);
      await Promise.all(links.map(loaded));

      const overlay = this.getAttribute("view") === "overlay", delay = this.getAttribute("delay") || "";
      const search = new URLSearchParams(OPTIONS.filter(o => this.hasAttribute(o)).map(o => [o, this.getAttribute(o)]));
      if (!this.hasAttribute("tabindex")) this.tabIndex = 0;
      configure({
        root: shadow, el: app, keys: this, base: BASE.href, storage: false, component: true,
        path: page + (overlay ? "overlay" : "") + (delay && (overlay ? "/" : "") + delay), search: `?${search}`,
        emit: m => this.dispatchEvent(new CustomEvent(m.type, { detail: m })),
      });
      this.#command = (await import("./embed.js")).command;
      await import("./main.js");
    } catch (e) {
      if (mounted === this && !this.#command) mounted = null;
      const failed = document.createElement("div");
      failed.className = "failed";
      failed.textContent = `stv.tf: ${e.message}`;
      shadow.append(failed);
    }
  }

  // A player by `steamid64`, `steamid` or `id`; none stops following.
  follow(player = {}) { this.#command?.({ type: "follow", ...player }); }
  setLayer(name, on) { this.#command?.({ type: "layer", name, on }); }
}

if (!customElements.get("stv-map")) customElements.define("stv-map", StvMap);
