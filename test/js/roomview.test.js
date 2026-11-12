import assert from "node:assert/strict";
import { test } from "node:test";
import { HAND_MS, RoomView, atTick, latestView, morePoints, sender } from "../../app/room/roomview.js";

/** @typedef {import("../../app/room/roomview.js").Seen} Seen */

const camera = () => ({ mode: "admin", cx: 1, cy: 2, w: 300, h: 200, rot: 0 });
/** A page's camera: following nobody, not moved by hand, at `now`. @returns {Seen} */
const still = (now = 10_000) => ({ follow: null, handMoved: 0, now });
/** @param {string} mode @param {object} [more] */
const line = (mode, more = {}) =>
  /** @type {import("../../types/stream").ViewLine} */ ({ type: "view", mode, by: 1, page: "a", ...more });

/** An admin's page named `page`. */
function admin(page = "a") {
  const v = new RoomView(page);
  v.setAdmin(true, still());
  return v;
}

test("viewers are locked to any view but a free one, admins never", () => {
  const viewer = new RoomView("v");
  assert.equal(viewer.hear(line("free"), still()).locked, false);
  for (const mode of ["auto", "follow", "admin"]) assert.equal(viewer.hear(line(mode), still()).locked, true, mode);
  for (const mode of ["free", "auto", "follow", "admin"])
    assert.equal(admin("b").hear(line(mode), still()).locked, false, mode);
});

test("auto zoom is everyone's, the admin who chose it too", () => {
  const leader = admin("a");
  const chosen = leader.choose("auto", camera);
  assert.deepEqual(chosen?.send, { view: { mode: "auto" }, take: true });
  const fx = leader.hear(line("auto"), { ...still(), follow: 3 });
  assert.equal(fx.auto, true);
  assert.equal(fx.follow, null, "and stops following");
  assert.equal(new RoomView("v").hear(line("auto"), still()).auto, true);
});

test("an admin's view is by hand, for the admin who chose it too", () => {
  const leader = admin("a");
  const chosen = leader.choose("admin", camera);
  assert.deepEqual(chosen?.send, { view: camera(), take: true });
  assert.deepEqual(chosen?.fx, { auto: false, follow: null });
  const fx = leader.hear(line("admin", camera()), still());
  assert.equal(fx.auto, false);
  assert.equal(fx.mirror, false, "the leader does not replay their own camera");
  const viewer = new RoomView("v").hear(line("admin", camera()), { ...still(), follow: 3 });
  assert.deepEqual(viewer, { locked: true, mirror: true, zoom: null, auto: false, follow: null });
});

test("a view is applied once, not on every camera move that comes in", () => {
  const viewer = new RoomView("v");
  assert.equal(viewer.hear(line("admin", camera()), still()).auto, false);
  const again = viewer.hear(line("admin", { ...camera(), cx: 50 }), still());
  assert.deepEqual(again, { locked: true, mirror: true, zoom: null }, "only replays the camera");
});

test("an admin clicking a player has everyone follow them, and stopping goes back", () => {
  const a = admin("a");
  a.hear(line("admin", camera()), still());
  assert.deepEqual(a.tick({ ...still(), follow: 7 }, camera), {
    view: { mode: "follow", player: 7, w: 300, h: 200 },
    take: true,
  });
  assert.equal(a.tick({ ...still(), follow: 7 }, camera), null, "once");
  // A camera move from before the follow came in does not cancel it.
  a.hear(line("admin", { ...camera(), cx: 9 }), { ...still(), follow: 7 });
  assert.equal(a.tick({ ...still(), follow: 7 }, camera), null);
  a.hear(line("follow", { player: 7 }), { ...still(), follow: 7 });
  assert.deepEqual(a.tick(still(), camera), { view: camera(), take: true }, "back to the admin's view");
  const viewer = new RoomView("v");
  assert.equal(viewer.hear(line("follow", { player: 7, page: "a" }), still()).follow, 7);
  assert.equal(viewer.tick({ ...still(), follow: null }, camera), null, "viewers choose nothing");
});

test("following stopped goes back to auto zoom or free as it was", () => {
  for (const mode of ["auto", "free"]) {
    const a = admin("a");
    a.hear(line(mode), still());
    a.tick({ ...still(), follow: 4 }, camera);
    a.hear(line("follow", { player: 4 }), { ...still(), follow: 4 });
    assert.deepEqual(a.tick(still(), camera), { view: { mode }, take: true }, mode);
  }
});

test("a follow the page drops by itself, at a map change, is not sent", () => {
  for (const page of ["a", "b"]) {
    const v = admin(page);
    v.hear(line("admin", camera()), still());
    if (page === "a") v.tick({ ...still(), follow: 7 }, camera);
    v.hear(line("follow", { player: 7 }), { ...still(), follow: 7 });
    v.dropped();
    assert.equal(v.tick(still(), camera), null, page === "a" ? "the leader" : "a page that mirrors it");
  }
});

test("any admin moving the camera by hand moves the admin's view, as a move", () => {
  const b = admin("b");
  b.hear(line("admin", camera()), still());
  assert.equal(b.tick(still(), camera), null, "not moved");
  assert.deepEqual(b.tick({ ...still(), handMoved: 20_000, now: 20_000 }, camera), { view: camera(), take: false });
  assert.equal(b.tick({ ...still(), handMoved: 20_000, now: 20_050 }, camera), null, "once per move");
  // While moving it, the admin does not replay another's camera.
  assert.equal(
    b.hear(line("admin", camera()), { ...still(), handMoved: 20_000, now: 20_000 + HAND_MS / 2 }).mirror,
    false,
  );
  assert.equal(
    b.hear(line("admin", camera()), { ...still(), handMoved: 20_000, now: 20_000 + HAND_MS * 2 }).mirror,
    true,
  );
});

test("a camera moved before the view was chosen does not take it", () => {
  const b = admin("b");
  // Moved in a free view: nothing to send.
  b.hear(line("free"), still());
  assert.equal(b.tick({ ...still(), handMoved: 15_000, now: 15_000 }, camera), null);
  // Another admin chooses their view: that old move does not take it over.
  b.hear(line("admin", camera()), { ...still(), handMoved: 15_000, now: 16_000 });
  assert.equal(b.tick({ ...still(), handMoved: 15_000, now: 16_050 }, camera), null);
  // A move auto zoom: moves are not sent.
  b.hear(line("auto"), still());
  assert.equal(b.tick({ ...still(), handMoved: 17_000, now: 17_000 }, camera), null);
});

test("a page leads, not a person: an admin's other tab replays the camera", () => {
  const tab1 = admin("a"),
    tab2 = admin("b");
  const view = line("admin", { ...camera(), page: "a" });
  assert.equal(tab1.hear(view, still()).mirror, false);
  assert.equal(tab2.hear(view, still()).mirror, true);
  assert.equal(tab1.leading(), true);
  assert.equal(tab2.leading(), false);
});

test("a viewer made admin is unlocked, and locked again when no longer", () => {
  const v = new RoomView("v");
  v.hear(line("auto"), still());
  assert.equal(v.setAdmin(true, still()).locked, false);
  assert.equal(v.setAdmin(false, still()).locked, true);
  assert.equal(v.choose("auto", camera), null, "only admins choose");
});

test("views waiting to be sent: the latest choice, and after it only the latest move", () => {
  const move = (/** @type {number} */ cx) => ({ view: { ...camera(), cx }, take: false });
  const choice = (/** @type {string} */ mode) => ({ view: { mode }, take: true });
  /** @type {import("../../app/room/roomview.js").Send[]} */
  let q = [];
  for (const s of [move(1), move(2), choice("auto"), move(3), move(4)]) q = latestView(q, s);
  assert.deepEqual(q, [choice("auto"), move(4)]);
  assert.deepEqual(latestView(q, choice("free")), [choice("free")]);
});

test("a stroke's end joins its points waiting", () => {
  /** @type {import("../../app/room/roomview.js").Strokes[]} */
  let q = [];
  q = morePoints(q, { stroke: 1, points: [[0, 0, 0]] });
  q = morePoints(q, { stroke: 1, points: [], end: true });
  assert.deepEqual(q, [{ stroke: 1, points: [[0, 0, 0]], end: true }]);
});

test("points waiting join their stroke, up to a request's worth", () => {
  /** @type {import("../../app/room/roomview.js").Strokes[]} */
  let q = [];
  q = morePoints(q, { stroke: 1, points: [[0, 0, 0]] });
  q = morePoints(q, { stroke: 1, points: [[1, 1, 16]] });
  q = morePoints(q, { stroke: 2, points: [[5, 5, 0]] });
  assert.deepEqual(q, [
    {
      stroke: 1,
      points: [
        [0, 0, 0],
        [1, 1, 16],
      ],
    },
    { stroke: 2, points: [[5, 5, 0]] },
  ]);
  for (let i = 0; i < 300; i++) q = morePoints(q, { stroke: 2, points: [[i, i, i]] });
  assert.deepEqual(
    q.map(s => s.points.length),
    [2, 256, 45],
  );
});

test("one request at a time: what comes meanwhile waits, folded, and goes next", async () => {
  /** @type {number[][]} */
  const sent = [];
  /** @type {(() => void)[]} */
  const done = [];
  const send = sender(
    (/** @type {number[]} */ item) => {
      sent.push(item);
      return new Promise(r => done.push(() => r(null)));
    },
    (waiting, item) => (waiting.length ? [[...waiting[0], ...item]] : [item]),
  );
  send([1]);
  send([2]);
  send([3]);
  assert.deepEqual(sent, [[1]], "the rest waits");
  done.shift()?.();
  await new Promise(r => setTimeout(r, 0));
  assert.deepEqual(sent, [[1], [2, 3]]);
  done.shift()?.();
  await new Promise(r => setTimeout(r, 0));
  send([4]);
  assert.deepEqual(sent, [[1], [2, 3], [4]], "straight away when nothing is on its way");
});

test("a followed player is followed at the zoom of the admin, who still scrolls it", () => {
  const a = admin("a");
  a.hear(line("free"), still());
  a.tick({ ...still(), follow: 7 }, camera);
  a.hear(line("follow", { player: 7, w: 300, h: 200 }), { ...still(), follow: 7 });
  const zoomed = () => ({ ...camera(), w: 150, h: 100 });
  assert.deepEqual(a.tick({ ...still(), follow: 7, handMoved: 20_000, now: 20_000 }, zoomed), {
    view: { mode: "follow", player: 7, w: 150, h: 100 },
    take: false,
  });
  const viewer = new RoomView("v");
  assert.deepEqual(viewer.hear(line("follow", { player: 7, w: 300, h: 200 }), still()).zoom, { w: 300, h: 200 });
  assert.deepEqual(
    viewer.hear(line("follow", { player: 7, w: 150, h: 100 }), { ...still(), follow: 7 }).zoom,
    { w: 150, h: 100 },
    "every time",
  );
  assert.equal(viewer.hear(line("auto"), still()).zoom, null);
  assert.equal(
    a.hear(line("follow", { player: 7, w: 150, h: 100 }), { ...still(), follow: 7 }).zoom,
    null,
    "the leader zooms their own",
  );
});

test("an event's tick is a moment in the recording, counted back from where playback is", () => {
  // At 60 s of play, tick 10000; 15 ms a tick.
  assert.equal(atTick(9000, { at: 60_000, now: 10_000 }, 15), 45_000);
  assert.equal(atTick(10_000, { at: 60_000, now: 10_000 }, 15), 60_000);
  assert.equal(atTick(1, { at: 1_000, now: 10_000 }, 15), 0, "not before the start");
});
