// What the comms view's parts are made of (voiceinvite.js, voicewho.js).

/**
 * @template {keyof HTMLElementTagNameMap} K
 * @param {K} tag @param {Partial<HTMLElementTagNameMap[K]>} props @param {(Node | string)[]} children
 * @returns {HTMLElementTagNameMap[K]}
 */
export function el(tag, props, ...children) {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

/** @param {string} text @param {string} [more] */
export function heading(text, more = "") {
  const h = el("div", { className: "vteam t-none" }, el("span", { className: "vtname" }, text));
  if (more) h.append(el("span", { className: "vhint" }, more));
  return h;
}

// A button that does `does` (the comms view hears clicks by it).
/** @param {string} what @param {string} does @param {string} [title] */
export function act(what, does, title = "") {
  const b = el("button", { title }, what);
  b.dataset.action = does;
  return b;
}

// A form's labels: one width, so what is typed lines up.
/** @param {string} text */
export const labelOf = text => el("span", { className: "vlabel" }, text);
