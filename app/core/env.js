// Where the viewer runs: its own page (these defaults), or a <stv-map> element on another site,
// which configures this before the rest of the viewer loads.
/**
 * @type {{
 *   root: Document | ShadowRoot; el: HTMLElement; keys: GlobalEventHandlers; base: string; path: string;
 *   search: string; storage: boolean; emit: ((m: { type: string }) => void) | null; component: boolean;
 * }}
 */
export const env = {
  // Where $() finds the viewer's elements.
  root: document,
  // The viewer's outermost element: its flags (overlay, noside, …) and CSS variables go on it.
  el: /** @type {HTMLElement} */ (document.getElementById("app")),
  // Where keyboard shortcuts listen.
  keys: window,
  // The stv.tf server its files and data come from.
  base: document.baseURI,
  // The match's path and options, which a page has in its address.
  path: location.pathname,
  search: location.search,
  // Settings are kept per browser on stv.tf's own page only.
  storage: true,
  // Tells the embedding page what happens (see embed.js). Null on stv.tf's own page, where embed.js
  // posts to an iframe's parent instead.
  emit: null,
  component: false,
};

/** @param {Partial<typeof env>} o */
export function configure(o) {
  Object.assign(env, o);
}
// On another site every file is read cross-origin. A copy cached before the server sent CORS headers
// (by Cloudflare or the browser) would fail, so the component asks for addresses of its own (?cors=1).
/** @param {string | URL} path */
export const asset = path => {
  const url = new URL(path, env.base);
  if (env.component) url.searchParams.set("cors", "1");
  return url.href;
};
