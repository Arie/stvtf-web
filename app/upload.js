// The upload page (pages/upload.html): a demo sent in parts (a proxy in front takes only so much
// in one request), a part that failed sent again from where the server has it, and then on to
// where it plays. See the server's web/upload.rs.

// How often a part that would not go is tried again, and how long between tries, in milliseconds.
const TRIES = 5,
  AGAIN_MS = 1500;

// The parts of a file `size` long, each at most `part`: where each starts and ends.
/** @param {number} size @param {number} part @returns {[number, number][]} */
export function parts(size, part) {
  /** @type {[number, number][]} */
  const all = [];
  for (let at = 0; at < size; at += part) all.push([at, Math.min(size, at + part)]);
  return all;
}

/** @param {Response} r What a refusal says, for the page. */
const why = async r => (await r.text().catch(() => "")) || `Refused (${r.status})`;

// Sends `file` to `home` (the page's folder), saying how far it is; where it plays.
/** @param {File} file @param {URL} home @param {(done: number) => void} said @returns {Promise<string>} */
export async function upload(file, home, said) {
  const json = { "Content-Type": "application/json" };
  const begun = await fetch(new URL("upload", home), {
    method: "POST",
    headers: json,
    body: JSON.stringify({ size: file.size }),
  });
  if (!begun.ok) throw new Error(await why(begun));
  const { id, part } = await begun.json();
  const to = new URL(`upload/${id}`, home);
  let at = 0;
  for (let tries = 0; at < file.size;) {
    const end = Math.min(file.size, at + part);
    const r = await fetch(`${to}?at=${at}`, { method: "PUT", body: file.slice(at, end) }).catch(() => null);
    if (r?.ok) [at, tries] = [end, 0];
    // It has more or less than was thought (an answer lost on the way): on from where it is.
    else if (r?.status === 409) at = Number(await r.text()) || 0;
    else if (r && r.status < 500) throw new Error(await why(r));
    else if (++tries >= TRIES) throw new Error("Could not reach the server");
    else await new Promise(done => setTimeout(done, AGAIN_MS));
    said(at / file.size);
  }
  const done = await fetch(to, { method: "POST", headers: json, body: JSON.stringify({ name: file.name }) });
  if (!done.ok) throw new Error(await why(done));
  return (await done.json()).page;
}

const main = globalThis.document?.getElementById("upload");
if (main) {
  const form = /** @type {HTMLFormElement} */ (main.querySelector("form")),
    file = /** @type {HTMLInputElement} */ (main.querySelector("input")),
    button = /** @type {HTMLButtonElement} */ (main.querySelector("button")),
    bar = /** @type {HTMLProgressElement} */ (main.querySelector("progress")),
    error = /** @type {HTMLElement} */ (main.querySelector(".error"));
  if (!main.dataset.signed) {
    form.hidden = true;
    /** @type {HTMLElement} */ (main.querySelector(".signin")).hidden = false;
  }
  form.onsubmit = async e => {
    e.preventDefault();
    const demo = file.files?.[0];
    if (!demo) return;
    error.textContent = "";
    if (demo.size > Number(main.dataset.max) * 2 ** 20) return void (error.textContent = `Over ${main.dataset.max} MB`);
    [button.disabled, bar.hidden, bar.value] = [true, false, 0];
    try {
      location.assign(await upload(demo, new URL(document.baseURI), done => (bar.value = done)));
    } catch (err) {
      error.textContent = err instanceof Error ? err.message : "Failed";
      [button.disabled, bar.hidden] = [false, true];
    }
  };
}
