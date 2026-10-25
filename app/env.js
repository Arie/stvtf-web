// Where the viewer runs: its own page (these defaults), or a <stv-map> element on another site,
// which configures this before the rest of the viewer loads.
export const env = {
  // Where $() finds the viewer's elements.
  root: document,
  // The viewer's outermost element: its flags (overlay, noside, …) and CSS variables go on it.
  el: document.getElementById("app"),
  // Where keyboard shortcuts listen.
  keys: window,
  // The stv.tf server its files and data come from.
  base: document.baseURI,
  // The match's path and options, which a page has in its address.
  path: location.pathname,
  search: location.search,
  // Settings are kept per browser on stv.tf's own page only.
  storage: true,
  // Hands the parent what happens (see embed.js); on the page, that is postMessage to an iframe's parent.
  emit: null,
  component: false,
};

export function configure(o) { Object.assign(env, o); }
// On another site every file is read cross-origin: a copy cached before the server sent CORS
// headers (Cloudflare's, the browser's) would fail it, so these addresses are its own.
export const asset = path => {
  const url = new URL(path, env.base);
  if (env.component) url.searchParams.set("cors", "1");
  return url.href;
};
