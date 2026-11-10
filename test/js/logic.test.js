import assert from "node:assert/strict";
import { test } from "node:test";
import {
  byClass,
  canvas,
  msPerTick,
  observeTick,
  optOn,
  parseMessage,
  renderTick,
  setClockRate,
  setTickInterval,
  state,
  statusText,
} from "../../app/core/core.js";
import { command, steamid64 } from "../../app/embed/embed.js";
import { layerOn } from "../../app/core/layers.js";
import { labelCorner } from "../../app/map/spawns.js";
import {
  drawOwn,
  inkByKey,
  markerFull,
  onClear,
  onDraw,
  onInk,
  onMarker,
  pointsOf,
  trimOwn,
} from "../../app/room/telestrator.js";
import { arcCount, arcShade, onArc, pendingAt } from "../../app/room/arcs.js";
import { arcTool } from "../../app/room/tools.js";
import { circleCount, onCircle } from "../../app/room/circles.js";
import { noteCount, onNote } from "../../app/room/notes.js";
import { env } from "../../app/core/env.js";
import { demostfAlias, sameLayout, screenOffset, teamsTurn, toScreen } from "../../app/map/view.js";

test("players sort by class, then name whatever its case, then id", () => {
  const rows = [
    { id: 1, class: "medic", name: "a" },
    { id: 2, class: "scout", name: "b" },
    { id: 3, class: "scout", name: "A" },
    { id: 5, class: "soldier", name: "x" },
    { id: 4, class: "soldier", name: "x" },
    { id: 6, class: "unknown", name: "a" },
    { id: 7, class: "made_up", name: "a" },
  ];
  assert.deepEqual(
    rows.sort(byClass).map(r => r.id),
    [3, 2, 4, 5, 1, 6, 7],
  );
});

test("the map's overview may be of another version of it", () => {
  const known = { cp_process_f12: {}, koth_product: {}, cp_gullywash_final1: {} };
  for (const [map, want] of [
    ["cp_process_f12", "cp_process_f12"],
    ["cp_process", "cp_process_f12"],
    ["cp_process_f13", "cp_process_f12"],
    ["koth_product_rcx", "koth_product"],
    ["cp_gullywash_final1", "cp_gullywash_final1"],
    ["cp_gullywash_f9", "cp_gullywash_f6"],
    ["cp_nope", null],
  ])
    assert.equal(demostfAlias(map, known), want, map);
});

test("an overview is used only if it covers the map's layout", () => {
  const overview = { boundary_min: { x: 0, y: 0 }, boundary_max: { x: 100, y: 100 } };
  const covering = (x1, y1, x0 = 0, y0 = 0) =>
    sameLayout(overview, {
      bounds: [
        [x0, y0],
        [x1, y1],
      ],
    });
  assert.equal(covering(100, 100), true);
  assert.equal(covering(100, 50), true, "exactly half");
  assert.equal(covering(100, 49), false);
  assert.equal(covering(300, 300, 200, 200), false, "nowhere near");
  assert.equal(sameLayout(overview, null), true, "no geometry yet");
});

test("the map turns so blue's spawns are on the left and red's on the right", () => {
  const spawns = (blue, red) => [
    { pos: blue, team: "blue" },
    { pos: [blue[0] + 100, blue[1]], team: "blue" },
    { pos: red, team: "red" },
  ];
  const blueLeft = (pts, rot) => {
    Object.assign(state.view, { cx: 0, cy: 0, scale: 1, rot });
    const [bx] = toScreen(...pts[0].pos),
      [rx] = toScreen(...pts[2].pos);
    return bx < rx;
  };
  // Gullywash: blue in the east, red in the west. Granary: blue north, red south.
  for (const [name, pts] of [
    ["gullywash", spawns([3385, -1270], [-2170, 652])],
    ["granary", spawns([-887, 3024], [-2186, -2968])],
    ["already left to right", spawns([-3000, 0], [3000, 200])],
  ]) {
    const rot = teamsTurn(pts);
    assert.equal(Math.round(rot / (Math.PI / 2)) * (Math.PI / 2), rot, `${name}: a quarter turn`);
    assert.ok(blueLeft(pts, rot), `${name}: blue on the left at ${rot}`);
  }
  assert.equal(teamsTurn(spawns([-3000, 0], [3000, 0])), 0);
  assert.equal(teamsTurn([{ pos: [0, 0], team: "red" }]), null, "one team only");
});

test("a spawn room's label keeps its corner while the view pans", () => {
  const r = { min: [-500, -300], max: [700, 400] };
  for (const rot of [0, Math.PI / 2, Math.PI, -Math.PI / 2]) {
    const picked = new Set();
    for (let cx = -3000; cx < 3000; cx += 0.731) {
      Object.assign(state.view, { cx, cy: 123.4, scale: 0.3, rot });
      const corners = [
        [r.min[0], r.min[1]],
        [r.max[0], r.min[1]],
        [r.max[0], r.max[1]],
        [r.min[0], r.max[1]],
      ].map(([x, y]) => toScreen(x, y));
      picked.add(corners.indexOf(labelCorner(corners)));
    }
    assert.equal(picked.size, 1, `one corner at ${rot}`);
  }
});

test("a screen offset is the same map offset whichever way the view is turned", () => {
  const was = { ...state.view };
  for (const rot of [0, Math.PI / 2, Math.PI, -Math.PI / 2, 0.3]) {
    Object.assign(state.view, { cx: 100, cy: 50, scale: 0.5, rot });
    const [centreX, centreY] = toScreen(100, 50);
    const [dx, dy] = screenOffset(30, -20, 0.5);
    const [x, y] = toScreen(100 + dx, 50 + dy);
    assert.ok(
      Math.abs(x - centreX - 30) < 1e-9 && Math.abs(y - centreY + 20) < 1e-9,
      `rot ${rot}: ${x - centreX}, ${y - centreY}`,
    );
  }
  Object.assign(state.view, was);
});

test("the tick clock jumps on a stall and otherwise eases", () => {
  // Behind the newest snapshot by the spacing (4 ticks) and the jitter margin (105 ms, 7 ticks).
  observeTick(1000);
  assert.ok(Math.abs(renderTick() - 989) < 1, `${renderTick()}`);
  observeTick(5000);
  assert.ok(Math.abs(renderTick() - 4989) < 1, `${renderTick()}`);
  observeTick(5010);
  const eased = renderTick();
  assert.ok(eased > 4988 && eased < 4990, `${eased} moves a little, not ten ticks`);
});

/** @param {number} ms */
const spin = ms => {
  const until = performance.now() + ms;
  while (performance.now() < until);
};

test("the tick clock runs at a room's playback rate, without a jump", () => {
  observeTick(1000);
  const before = renderTick();
  setClockRate(2);
  assert.ok(Math.abs(renderTick() - before) < 0.5, "no jump");
  spin(60);
  // 60 ms at twice 15 ms a tick.
  assert.ok(Math.abs(renderTick() - before - 8) < 1, `${renderTick() - before}`);
  setClockRate(1);
  assert.equal(state.paused, false);
});

test("a paused clock stands still and shows a sought snapshot as it is", () => {
  observeTick(2000);
  setClockRate(0);
  assert.equal(state.paused, true);
  const still = renderTick();
  spin(20);
  assert.equal(renderTick(), still);
  observeTick(9000);
  assert.ok(Math.abs(renderTick() - 9000) < 1e-9, `${renderTick()}`);
  setClockRate(1);
});

test("a tick interval comes only from a server that gives one", () => {
  setTickInterval(0);
  assert.equal(msPerTick, 15);
  setTickInterval(1 / 128);
  assert.equal(msPerTick, 7.8125);
  setTickInterval(0.015);
});

test("a toggle without an option in the address is its default", () => {
  assert.equal(optOn("names"), true);
  assert.equal(optOn("names", false), false);
});

test("the Steam ids an embedding page gets are real ones", () => {
  assert.equal(steamid64("[U:1:231502]"), "76561197960497230");
  assert.equal(steamid64("[U:1:1]"), "76561197960265729");
  for (const id of ["BOT", "", null, "[U:1:x]", "[A:1:1]"]) assert.equal(steamid64(id), null, id);
});

test("an embedding page can follow a player by any id, and stop", () => {
  state.lastPlayers = [
    { id: 3, steamid: "[U:1:231502]" },
    { id: 4, steamid: "[U:1:1]" },
  ];
  command({ type: "follow", steamid: "[U:1:231502]" });
  assert.equal(state.follow, 3);
  command({ type: "follow", steamid64: "76561197960265729" });
  assert.equal(state.follow, 4);
  command({ type: "follow", id: 3 });
  assert.equal(state.follow, 3);
  command({ type: "follow", id: 99 });
  assert.equal(state.follow, null);
});

test("an embedding page can switch known layers only", () => {
  command({ type: "layer", name: "caps", on: false });
  assert.equal(layerOn("caps"), false);
  command({ type: "layer", name: "caps", on: true });
  assert.equal(layerOn("caps"), true);
  command({ type: "layer", name: "made_up", on: true });
  assert.equal(layerOn("made_up"), undefined);
});

test("the sidebar keeps one word while SourceTV signs on, however often it is dropped", () => {
  const run = (shown, ...lines) =>
    lines.map(m => (shown = statusText(shown, typeof m === "string" ? { state: m } : m)[0]));
  const drop = { state: "disconnected", reason: "timed out" };
  // A server starting up: in and dropped during signon, then failing to connect, then live.
  assert.deepEqual(
    run(
      "waiting for data",
      "connecting",
      "new",
      "prespawn",
      "spawn",
      drop,
      "connecting",
      { state: "connect_failed", reason: "no answer" },
      "connecting",
      "new",
      "spawn",
      "full",
    ),
    [
      "connecting",
      "connecting",
      "connecting",
      "connecting",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "waiting for SourceTV",
      "live",
    ],
  );
  // A map change, signing on again, dropped once on the way.
  assert.deepEqual(run("live", "changelevel", "map_change", "new", "prespawn", drop, "connecting", "spawn", "full"), [
    "map changing",
    "map changing",
    "map changing",
    "map changing",
    "map changing",
    "map changing",
    "map changing",
    "live",
  ]);
  // Dropped while live.
  assert.deepEqual(run("no data", drop, "connecting", "new", "full"), [
    "reconnecting",
    "reconnecting",
    "reconnecting",
    "live",
  ]);
  assert.deepEqual(
    statusText("connecting", drop),
    ["waiting for SourceTV", "timed out"],
    "the reason goes in the tooltip",
  );
  assert.deepEqual(statusText("connecting", { state: "gave_up", reason: "wrong password" }), [
    "gave up: wrong password",
    "wrong password",
  ]);
});

test("stream messages lose invisible characters but keep emoji joiners", () => {
  const raw = JSON.stringify({
    type: "chat",
    name: "a\u202eb\u200bc",
    text: "\u{1F469}\u200d\u{1F4BB}",
    players: [{ name: "\u2066x\u2069" }],
  });
  assert.deepEqual(parseMessage(raw), {
    type: "chat",
    name: "abc",
    text: "\u{1F469}\u200d\u{1F4BB}",
    players: [{ name: "x" }],
  });
  assert.deepEqual(parseMessage(raw), parseMessage(raw));
  assert.equal(parseMessage('{"name":"a\\u202Eb"}').name, "ab");
  assert.equal(parseMessage('{"name":"plain"}').name, "plain");
});

test("a stroke drawn here shows at once and its echo from the room is not drawn twice", () => {
  drawOwn({ stroke: 1, points: [[0, 0, 0]] });
  drawOwn({ stroke: 1, points: [[5, 5, 16]] });
  onDraw({
    type: "draw",
    stroke: 1,
    points: [
      [0, 0, 0],
      [5, 5, 16],
    ],
  });
  assert.equal(pointsOf(1), 2);
  onDraw({ type: "draw", stroke: 2, points: [[1, 1, 0]] });
  assert.equal(pointsOf(2), 1, "someone else's stroke");
});

test("a page that comes back has the marker as the room has it", () => {
  onClear();
  /** @type {[number, number, number][]} */
  const pts = [
    [0, 0, 0],
    [1, 1, 8],
  ];
  onDraw({ type: "draw", stroke: 10, by: 2, points: [pts[0]] });
  onDraw({ type: "draw", stroke: 11, by: 2, points: pts });
  onMarker({ type: "marker", strokes: [{ stroke: 10, by: 2, points: pts }] });
  assert.equal(pointsOf(10), 2, "the whole stroke");
  assert.equal(pointsOf(11), 0, "cleared meanwhile");
  onClear();
});

test("a marker stroke the full map refused is trimmed to what the room took", () => {
  onClear();
  onMarker({ type: "marker", strokes: [{ stroke: 1, by: 2, points: [[0, 0, 0]] }], max: 3 });
  assert.ok(!markerFull());
  drawOwn({
    stroke: 2,
    by: 1,
    points: [
      [0, 0, 0],
      [1, 1, 8],
    ],
  });
  assert.ok(markerFull());
  trimOwn(2, 1);
  assert.equal(pointsOf(2), 1);
  trimOwn(2, 0);
  assert.equal(pointsOf(2), 0);
  onClear();
});

test("a jump arc is shaded like the jump trails, dark at the launch to bright at the peak", () => {
  assert.equal(arcShade("#ff0000", 0), "hsl(0 100% 24%)");
  assert.equal(arcShade("#ff0000", 1), "hsl(0 100% 74%)");
});

test("jump arcs come with the marker and an arc line, are no strokes, and go with a clear", () => {
  onClear();
  /** @type {[number, number, number][]} */
  const points = [
    [0, 0, 0],
    [0, 0, 100],
    [200, 0, 228],
    [300, 0, 200],
  ];
  onMarker({ type: "marker", strokes: [{ stroke: 2 ** 31, by: 1, points: [[0, 0, 0]], ink: "#ff0000", arc: points }] });
  assert.equal(arcCount(), 1);
  assert.equal(pointsOf(2 ** 31), 0, "not drawn as a stroke");
  onArc({ type: "arc", by: 1, stroke: 2 ** 31 + 1, ink: "#00ff00", points });
  assert.equal(arcCount(), 2);
  onClear();
  assert.equal(arcCount(), 0);
});

test("circles come with the marker and a circle line, are no strokes, and go with a clear", () => {
  onClear();
  onMarker({
    type: "marker",
    strokes: [{ stroke: 2 ** 30, by: 1, points: [[5, 5, 0]], ink: "#ff0000", circle: [5, 5, 64] }],
  });
  assert.equal(circleCount(), 1);
  assert.equal(pointsOf(2 ** 30), 0, "not drawn as a stroke");
  onCircle({ type: "circle", by: 1, stroke: 2 ** 30 + 1, ink: "#00ff00", at: [9, 9, 0] });
  assert.equal(circleCount(), 2);
  onClear();
  assert.equal(circleCount(), 0);
});

test("notes come with the marker and a note line, are no strokes, and go with a clear", () => {
  onClear();
  onMarker({
    type: "marker",
    strokes: [{ stroke: 2 ** 29, by: 1, points: [[5, 5, 0]], ink: "#ff0000", note: { text: "lobby" } }],
  });
  assert.equal(noteCount(), 1);
  assert.equal(pointsOf(2 ** 29), 0, "not drawn as a stroke");
  onNote({ type: "note", by: 1, stroke: 2 ** 29 + 1, ink: "#00ff00", text: "watch", at: null, player: 3 });
  assert.equal(noteCount(), 2);
  onClear();
  assert.equal(noteCount(), 0);
});

test("picking the marker with the arc tool armed leaves the map in drawing mode", () => {
  // A room bar button: its own onclick, then whatever else listens, as a browser runs them.
  const button = () => {
    /** @type {(() => void)[]} */
    const heard = [];
    const classes = new Set();
    const b = {
      classList: {
        contains: (/** @type {string} */ c) => classes.has(c),
        toggle: (/** @type {string} */ c, on = !classes.has(c)) => (on ? classes.add(c) : classes.delete(c), on),
      },
      /** @type {null | (() => void)} */
      onclick: null,
      addEventListener: (/** @type {string} */ _type, /** @type {() => void} */ f) => heard.push(f),
      click: () => [b.onclick, ...heard].forEach(f => f?.()),
    };
    return b;
  };
  const arc = button(),
    marker = button();
  marker.onclick = () => env.el.classList.toggle("drawing", marker.classList.toggle("on"));
  arcTool(/** @type {any} */ (arc), [/** @type {any} */ (marker)]);
  arc.click();
  assert.ok(env.el.classList.contains("drawing"), "armed");
  marker.click();
  assert.ok(marker.classList.contains("on") && !arc.classList.contains("on"));
  assert.ok(env.el.classList.contains("drawing"), "still drawing, with the marker");
});

test("the arc tool stays armed after an arc, for the next one", () => {
  const classes = new Set();
  const arc = {
    classList: {
      contains: (/** @type {string} */ c) => classes.has(c),
      toggle: (/** @type {string} */ c, on = !classes.has(c)) => (on ? classes.add(c) : classes.delete(c), on),
    },
    /** @type {null | (() => void)} */
    onclick: null,
  };
  /** @type {(e: any) => void} */
  let down = () => {};
  const listen = canvas.addEventListener;
  canvas.addEventListener = (/** @type {string} */ _type, /** @type {any} */ f) => (down = f);
  arcTool(/** @type {any} */ (arc), []);
  canvas.addEventListener = listen;
  arc.onclick?.();
  const click = () => down({ button: 0, clientX: 0, clientY: 0, preventDefault() {}, stopImmediatePropagation() {} });
  click();
  assert.ok(pendingAt(), "the launch waits for its landing");
  click();
  assert.equal(pendingAt(), null);
  assert.ok(arc.classList.contains("on") && env.el.classList.contains("drawing"), "still armed");
  arc.onclick?.();
  assert.ok(!arc.classList.contains("on") && !env.el.classList.contains("drawing"), "let go by its button");
});

test("r and b take a team's ink, y one's own back", () => {
  const red = "#d0453f",
    blu = "#4f8fc0";
  assert.deepEqual(inkByKey("r", "#ffd23f", null), { ink: red, own: "#ffd23f" });
  assert.deepEqual(inkByKey("b", red, "#ffd23f"), { ink: blu, own: "#ffd23f" }, "from one team's to the other's");
  assert.deepEqual(inkByKey("y", blu, "#ffd23f"), { ink: "#ffd23f", own: "#ffd23f" });
  assert.deepEqual(inkByKey("r", red, "#ffd23f"), { ink: null, own: "#ffd23f" }, "red already");
  assert.deepEqual(
    inkByKey("y", "#00ff00", "#ffd23f"),
    { ink: null, own: "#00ff00" },
    "picked since: that is one's own",
  );
  assert.deepEqual(inkByKey("y", red, null), { ink: null, own: null }, "none to go back to");
});

test("a tool that is on reads on its ink", () => {
  assert.equal(onInk("#ffd23f"), "#0b0c0e");
  assert.equal(onInk("#4f8fc0"), "#0b0c0e");
  assert.equal(onInk("#d0453f"), "#ffffff");
  assert.equal(onInk("#000000"), "#ffffff");
});
