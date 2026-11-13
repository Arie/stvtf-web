// How to watch a room in TF2, when it is relayed: the console line to paste, and a link that has
// Steam connect.
import { $ } from "../core/core.js";
import { relayListeners } from "../core/stream.js";
import { copied, copyText } from "./copy.js";

export function inTf2() {
  const cmd = $("tf2cmd"),
    copy = $("tf2copy"),
    why = $("tf2why");
  relayListeners.add(m => {
    $("intf2").hidden = !m.connect;
    if (!m.connect) return;
    cmd.textContent = `connect ${m.connect}; password "${m.password}"`;
    // Steam's link takes an IP address, not a name: none without one.
    const open = /** @type {HTMLAnchorElement} */ ($("tf2open"));
    open.hidden = !m.address;
    if (m.address) open.href = `steam://connect/${m.address}/${m.password}`;
    why.hidden = !m.problem;
    why.textContent = m.problem ? `Cannot be played in TF2: ${m.problem}` : "";
  });
  copy.onclick = () => copyText(cmd.textContent ?? "").then(ok => ok && copied(copy));
}
