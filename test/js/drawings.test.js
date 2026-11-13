import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { drawOwn, onClear, onMarker, onPlaybackMap, pointsOf } from "../../app/room/telestrator.js";

const marker = strokes => ({ type: "marker", strokes });
const theirs = { stroke: 7, by: 2, points: [[1, 1, 0]] };
const draw = () => drawOwn({ stroke: 1, points: [[5, 5, 0]], by: 1 });

beforeEach(() => {
  onClear();
  onPlaybackMap({ load: 1 });
});

test("an own stroke is gone on another map, the marker line first or the playback line", () => {
  draw();
  onMarker(marker([]));
  assert.equal(pointsOf(1), 1, "a marker line on the same map keeps it");
  onPlaybackMap({ load: 1 });
  assert.equal(pointsOf(1), 1);
  onPlaybackMap({ load: 2 });
  assert.equal(pointsOf(1), 0, "playback first");
  onMarker(marker([]));
  draw();
  onMarker(marker([]));
  onPlaybackMap({ load: 3 });
  assert.equal(pointsOf(1), 0, "marker first");
});

test("the new map's marker survives, whichever line comes first", () => {
  onMarker(marker([theirs]));
  onPlaybackMap({ load: 2 });
  assert.equal(pointsOf(7), 1, "marker first");
  onPlaybackMap({ load: 3 });
  assert.equal(pointsOf(7), 0, "playback first: the old map's is dropped");
  onMarker(marker([{ ...theirs, stroke: 8 }]));
  assert.equal(pointsOf(8), 1);
});
