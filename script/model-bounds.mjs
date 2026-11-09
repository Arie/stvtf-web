// The upward faces of every prop model's collision mesh in TF2's VPKs, for props.txt: the tops
// of static props (containers, crates) that strokes are drawn on in TF2. Run with the path of
// tf2_misc_dir.vpk:
//
//   node script/model-bounds.mjs .../tf/tf2_misc_dir.vpk | gzip -9 > props.txt.gz
//
// A line per model with a collision mesh: its name, then its triangles that face up (x y z three
// times each), in the model's own units.
import { openSync, readSync, readFileSync } from "node:fs";

const SIGNATURE = 0x55aa1234;
const HEADER_V2 = 28;
// A directory entry after its name: CRC, preload size, archive, offset, length, terminator.
const ENTRY = 18;
// In the directory file itself, after the tree.
const IN_DIR = 0x7fff;
// The physics engine works in metres, its y down and z forward.
const INCHES_PER_METRE = 39.3700787;
// A triangle faces up when its normal points up at least this much in the model: a prop tilted
// a little still has its tops, kept or not once the map places it.
const UP = 0.5;
const VPHY = 0x59485056; // "VPHY"
// Sizes of the structures read: phyheader_t, the VPHY header, IVP_Compact_Ledgetree_Node,
// IVP_Compact_Ledge, IVP_Compact_Triangle, IVP_Compact_Poly_Point.
const PHY_HEADER = 16;
const VPHY_HEADER = 28;
const NODE = 28;
const LEDGE = 16;
const TRIANGLE = 16;
const POINT = 16;

const dir = process.argv[2];
if (!dir?.endsWith("_dir.vpk")) {
  console.error("usage: node script/model-bounds.mjs .../tf2_misc_dir.vpk");
  process.exit(2);
}
const buf = readFileSync(dir);
if (buf.readUInt32LE(0) !== SIGNATURE || buf.readUInt32LE(4) !== 2) throw new Error("not a VPK 2 directory");
const treeSize = buf.readUInt32LE(8);
let at = HEADER_V2;
const cstring = () => {
  const end = buf.indexOf(0, at);
  const s = buf.toString("latin1", at, end);
  at = end + 1;
  return s;
};

/** @typedef {{ name: string; archive: number; offset: number; length: number; preload: Buffer }} Entry */
/** @type {Entry[]} */
const phys = [];
for (let ext = cstring(); ext; ext = cstring())
  for (let path = cstring(); path; path = cstring())
    for (let name = cstring(); name; name = cstring()) {
      const preloadBytes = buf.readUInt16LE(at + 4),
        archive = buf.readUInt16LE(at + 6),
        offset = buf.readUInt32LE(at + 8),
        length = buf.readUInt32LE(at + 12);
      const preload = buf.subarray(at + ENTRY, at + ENTRY + preloadBytes);
      at += ENTRY + preloadBytes;
      if (ext === "phy" && path.startsWith("models/props"))
        phys.push({ name: `${path}/${name}.mdl`, archive, offset, length, preload });
    }

/** @type {Map<number, number>} */
const archives = new Map();
const fileOf = (/** @type {number} */ i) => {
  let fd = archives.get(i);
  if (fd === undefined) {
    fd = openSync(i === IN_DIR ? dir : dir.replace("_dir.vpk", `_${String(i).padStart(3, "0")}.vpk`), "r");
    archives.set(i, fd);
  }
  return fd;
};

/** @param {Entry} e */
function contents(e) {
  const out = Buffer.alloc(e.preload.length + e.length);
  e.preload.copy(out);
  const base = e.archive === IN_DIR ? HEADER_V2 + treeSize : 0;
  readSync(fileOf(e.archive), out, e.preload.length, e.length, base + e.offset);
  return out;
}

/** A point of the physics engine's, in the model's units. @param {Buffer} b @param {number} o */
const point = (b, o) => [
  b.readFloatLE(o) * INCHES_PER_METRE,
  b.readFloatLE(o + 8) * INCHES_PER_METRE,
  -b.readFloatLE(o + 4) * INCHES_PER_METRE,
];

/** The convex pieces of the surface at `s`: the leaves of its ledge tree. @param {Buffer} b @param {number} s */
function ledges(b, s) {
  const out = [];
  const stack = [s + b.readInt32LE(s + 32)];
  while (stack.length) {
    const node = /** @type {number} */ (stack.pop());
    const right = b.readInt32LE(node);
    if (right === 0) out.push(node + b.readInt32LE(node + 4));
    else stack.push(node + NODE, node + right);
  }
  return out;
}

/** The upward triangles of the surface at `s`, as point triples. @param {Buffer} b @param {number} s */
function surfaceTriangles(b, s) {
  /** @type {number[][][]} */
  const out = [];
  for (const l of ledges(b, s)) {
    const points = b.readInt32LE(l),
      n = b.readInt16LE(l + 12);
    /** @type {number[][][]} */
    const piece = [];
    for (let t = 0; t < n; t++) {
      const tri = l + LEDGE + t * TRIANGLE;
      piece.push([0, 1, 2].map(k => point(b, l + points + (b.readUInt32LE(tri + 4 + k * 4) & 0xffff) * POINT)));
    }
    // Outward, away from the middle of the piece (it is convex).
    const all = piece.flat();
    const mid = [0, 1, 2].map(k => all.reduce((a, p) => a + p[k], 0) / all.length);
    for (const [a, p, q] of piece) {
      const u = [0, 1, 2].map(k => p[k] - a[k]),
        v = [0, 1, 2].map(k => q[k] - a[k]);
      const n3 = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
      const len = Math.hypot(...n3);
      if (len === 0) continue;
      const side = [0, 1, 2].reduce((d, k) => d + n3[k] * (a[k] - mid[k]), 0) < 0 ? -1 : 1;
      if ((side * n3[2]) / len >= UP) out.push(side > 0 ? [a, p, q] : [a, q, p]);
    }
  }
  return out;
}

/** The upward triangles of a .phy, or null when it is not one read here. @param {Buffer} b */
function upward(b) {
  const solids = b.readInt32LE(8);
  /** @type {number[][][]} */
  const out = [];
  let o = PHY_HEADER;
  for (let i = 0; i < solids && o + 4 <= b.length; i++) {
    const size = b.readInt32LE(o);
    if (b.readUInt32LE(o + 4) !== VPHY) return null;
    out.push(...surfaceTriangles(b, o + 4 + VPHY_HEADER));
    o += 4 + size;
  }
  return out;
}

const lines = [];
for (const e of phys) {
  let tris;
  try {
    tris = upward(contents(e));
  } catch {
    tris = null;
  }
  if (!tris?.length) continue;
  const nums = tris.flat(2).map(v => Math.round(v * 10) / 10);
  lines.push(`${e.name.toLowerCase()} ${nums.join(" ")}`);
}
lines.sort();
process.stdout.write(lines.join("\n") + "\n");
