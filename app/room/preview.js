// What the camera being placed would show: floors and walls flat shaded, players as pills.
// Drawn with three.js (app/vendor), loaded when first needed.
import { $, state, TEAM } from "../core/core.js";
import { KIND_COLOURS } from "../map/geometry.js";
import { footOf } from "./sight.js";
/** @typedef {import("./sight.js").Spot} Spot */
/** @typedef {import("./sight.js").Grid} Grid */

// TF2's picture at 16:9, top to bottom in degrees, and how near and far it is drawn, in map units.
const FOV = 73.74,
  NEAR = 8,
  FAR = 40000;
const WALL = "#8d887e",
  SKY = "#1c2733";
// A player standing: how wide and tall.
const PILL = { r: 24, h: 82 };

// three.js: untyped here.
/** @type {any} */
let THREE = null;
let asked = false,
  failed = false;
/** @type {{ renderer: any; scene: any; camera: any; world: any; of: object | null; grid: Grid | null; pills: any[]; teams: Record<string, any> } | null} */
let stage = null;

// A floor's triangles: its outline fanned from its first corner, at its height and slope.
/** @param {{ z: number; p: number[]; k?: string; g?: [number, number]; c?: string }} f @param {number[]} at @param {number[]} tint */
function floorInto(f, at, tint) {
  const n = f.p.length / 2,
    c = new THREE.Color(f.c ?? KIND_COLOURS[/** @type {"other"} */ (f.k && f.k in KIND_COLOURS ? f.k : "other")]);
  let [mx, my] = [0, 0];
  for (let i = 0; i < n; i++) [mx, my] = [mx + f.p[2 * i] / n, my + f.p[2 * i + 1] / n];
  const z = (/** @type {number} */ x, /** @type {number} */ y) =>
    f.z + (f.g ? f.g[0] * (x - mx) + f.g[1] * (y - my) : 0);
  for (let i = 1; i < n - 1; i++)
    for (const v of [0, i, i + 1]) {
      const [x, y] = [f.p[2 * v], f.p[2 * v + 1]];
      at.push(x, y, z(x, y));
      tint.push(c.r, c.g, c.b);
    }
}
// A wall's: from its top down to where the solid under it ends (a voxel, where nothing is known).
/** @param {{ z: number; p: number[]; c?: string }} w @param {Grid | null} grid @param {number[]} at @param {number[]} tint */
function wallInto(w, grid, at, tint) {
  const [x1, y1, x2, y2] = w.p,
    c = new THREE.Color(w.c ?? WALL),
    foot = grid ? footOf(grid, w.p, w.z) : w.z - 64;
  for (const [x, y, z] of [
    [x1, y1, w.z],
    [x2, y2, w.z],
    [x2, y2, foot],
    [x1, y1, w.z],
    [x2, y2, foot],
    [x1, y1, foot],
  ]) {
    at.push(x, y, z);
    tint.push(c.r, c.g, c.b);
  }
}
/** @param {any} g @param {Grid | null} grid */
function worldOf(g, grid) {
  /** @type {number[]} */
  const at = [],
    /** @type {number[]} */
    tint = [];
  for (const f of [...g.floors, ...(g.props ?? [])]) floorInto(f, at, tint);
  for (const w of g.walls) wallInto(w, grid, at, tint);
  const shape = new THREE.BufferGeometry();
  shape.setAttribute("position", new THREE.Float32BufferAttribute(at, 3));
  shape.setAttribute("color", new THREE.Float32BufferAttribute(tint, 3));
  shape.computeVertexNormals();
  return new THREE.Mesh(shape, new THREE.MeshLambertMaterial({ vertexColors: true, side: THREE.DoubleSide }));
}

function start() {
  const renderer = new THREE.WebGLRenderer({ canvas: $("cam-view"), antialias: true, preserveDrawingBuffer: true }),
    scene = new THREE.Scene(),
    camera = new THREE.PerspectiveCamera(FOV, 16 / 9, NEAR, FAR),
    sun = new THREE.DirectionalLight("#ffffff", 1.6);
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
  camera.up.set(0, 0, 1);
  sun.position.set(0.5, 0.3, 1);
  scene.background = new THREE.Color(SKY);
  scene.add(new THREE.HemisphereLight("#ffffff", "#8a8f99", 1.6), new THREE.AmbientLight("#ffffff", 0.8), sun);
  const pill = new THREE.CapsuleGeometry(PILL.r, PILL.h - 2 * PILL.r, 2, 8).rotateX(Math.PI / 2),
    teams = Object.fromEntries(
      ["red", "blue"].map(t => [t, new THREE.MeshLambertMaterial({ color: TEAM[/** @type {"red"} */ (t)] })]),
    );
  stage = { renderer, scene, camera, world: null, of: null, grid: null, pills: [], teams };
  stage.pills = Array.from({ length: 24 }, () => new THREE.Mesh(pill, teams.red));
  scene.add(...stage.pills);
}
// The map's, made again when it or what is known of its walls changed.
/** @param {NonNullable<typeof stage>} s @param {Grid | null} grid */
function keepWorld(s, grid) {
  if (s.of === state.geometry && s.grid === grid) return;
  if (s.world) {
    s.scene.remove(s.world);
    s.world.geometry.dispose();
    s.world.material.dispose();
  }
  [s.of, s.grid, s.world] = [state.geometry, grid, worldOf(state.geometry, grid)];
  s.scene.add(s.world);
}
/** @param {NonNullable<typeof stage>} s */
function placePills(s) {
  const alive = state.lastPlayers.filter(p => p.alive && p.pos && p.team in s.teams);
  s.pills.forEach((m, i) => {
    const p = alive[i];
    m.visible = Boolean(p?.pos);
    if (!p?.pos) return;
    m.material = s.teams[p.team];
    m.position.set(p.pos[0], p.pos[1], p.pos[2] + PILL.h / 2);
  });
}
async function load() {
  asked = true;
  try {
    // By its address: the page's types do not reach into it.
    THREE = await import(new URL("../vendor/three.module.min.js", import.meta.url).href);
    start();
  } catch (e) {
    failed = true;
    console.warn("no camera preview", e);
  }
}

// Draws what `c` would show, or (null) puts the preview away.
/** @param {Spot | null} c @param {Grid | null} grid */
export function preview(c, grid) {
  const view = /** @type {HTMLCanvasElement} */ ($("cam-view")),
    on = c !== null && !failed && Boolean(state.geometry);
  if (view.hidden === on) view.hidden = !on;
  if (!on || !c) return;
  if (!asked) load();
  if (!stage) return;
  keepWorld(stage, grid);
  placePills(stage);
  if (view.width !== view.clientWidth * stage.renderer.getPixelRatio())
    stage.renderer.setSize(view.clientWidth, view.clientHeight, false);
  stage.camera.position.set(...c.pos);
  stage.camera.lookAt(...c.aim);
  stage.renderer.render(stage.scene, stage.camera);
}
