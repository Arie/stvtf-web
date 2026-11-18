// The score, set right by a room's admins: a click on a team's score on the HUD, the number it
// should be, Enter. The room keeps the difference for that game (see core/names.js).
import { $ } from "../core/core.js";
import { me, post } from "./home.js";
import { note } from "./tools.js";

const shown = (/** @type {string} */ team) => Number($(`score-${team}`).textContent) || 0;

/** @param {"blue" | "red"} team */
function edit(team) {
  const score = $(`score-${team}`);
  if (!me.admin || score.classList.contains("editing")) return;
  const input = Object.assign(document.createElement("input"), {
    type: "number",
    min: "0",
    max: "99",
    className: "sb-edit",
  });
  input.value = String(shown(team));
  input.setAttribute("aria-label", `${team === "blue" ? "BLU" : "RED"} score`);
  // Once: taking the input away blurs it.
  const done = () => {
    if (!score.classList.contains("editing")) return;
    score.classList.remove("editing");
    input.remove();
  };
  input.onkeydown = e => {
    e.stopPropagation();
    if (e.key === "Escape") return done();
    if (e.key !== "Enter") return;
    const to = Math.round(Number(input.value));
    done();
    post("score", { team, score: to }).then(async r => r.ok || note(await r.text()));
  };
  input.onblur = done;
  score.classList.add("editing");
  score.after(input);
  input.select();
}

// An admin's click on a score edits it.
export function scoreTool() {
  for (const team of /** @type {const} */ (["blue", "red"]))
    $(`score-${team}`).addEventListener("click", () => edit(team));
}
