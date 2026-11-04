import assert from "node:assert/strict";
import { test } from "node:test";

// The address is read when the modules load.
globalThis.location = {
  pathname: "/w/abc/overlay/4.5",
  search: "?names=0&score=1&health=2&zoom=map&bg=flat&delay=x&dot=14",
};
const { DELAY, DELAY_MS, OVERLAY, PAGE, PLAIN_BG, optOn, overlayUnit, state } = await import("../../app/core.js");
const { layerOn } = await import("../../app/layers.js");

test("a delay after the last / holds the match back, and is not part of the page", () => {
  assert.equal(DELAY, "4.5");
  assert.equal(DELAY_MS, 4500);
  assert.equal(PAGE, "/w/abc/overlay/");
  assert.equal(OVERLAY, true);
});

test("toggles in the address: 1 on, 0 off, anything else the default", () => {
  assert.equal(optOn("names"), false);
  assert.equal(optOn("score", false), true);
  assert.equal(optOn("health"), true);
  assert.equal(optOn("health", false), false);
});

test("the overlay asks for what it says: the whole map, plain, with fixed markers", () => {
  assert.equal(state.autoZoom, false);
  assert.equal(PLAIN_BG, "flat");
  assert.equal(overlayUnit(), 14);
});

test("the overlay shows only capture areas unless told otherwise", () => {
  assert.equal(layerOn("caps"), true);
  assert.equal(layerOn("flow"), false);
});
