// The page's own data: the map geometry the server builds (src/bsp.rs) and the shared `state`
// in app/core.js. Only `tsc` reads this file.
import type { Player, Snapshot, GameEvent, Team, Vec3 } from "./stream";

export type SpawnRoom = { name: string; team: Team; min: Vec3; max: Vec3; disabled: boolean };
export type SpawnPoint = { pos: Vec3; team: Team; point?: number | null; disabled: boolean };
export type Spawns = { rooms: SpawnRoom[]; points: SpawnPoint[] };

/** The server's map geometry (`/map/<name>`). */
export type Geometry = {
  bounds: [Vec3, Vec3];
  spawns?: Spawns;
  packs?: { kind: "health" | "ammo"; size: string; pos: Vec3 }[];
  cap_areas?: { point: number; min: Vec3; max: Vec3 }[];
  floors?: { z: number; p: number[]; k?: string; d?: boolean }[];
  walls?: { z: number; p: number[] }[];
  track?: Vec3[];
  playable?: { origin: [number, number]; cell: number; w: number; h: number; bits: string };
};

/** demos.tf's overview image and the map box it covers. */
export type Overview = {
  /** The image, or a canvas made from it. */
  img: HTMLImageElement | HTMLCanvasElement;
  b: { boundary_min: { x: number; y: number }; boundary_max: { x: number; y: number } };
};

/** The view: the world point at the canvas centre, pixels per unit and the rotation (radians). */
export type View = { cx: number; cy: number; scale: number; rot: number };
/** A framing the camera heads for; `slow` brings a straggler in gently. */
export type Framing = { cx: number; cy: number; scale: number; slow?: boolean };

/** A short-lived drawing (a shot, a boom, a kill …) with when it started and how long it lasts, in ms. */
export type Effect = { kind: string; t0: number; life: number; [field: string]: any };
export type Mark = { pos: Vec3; t0: number };
export type Scorch = { pos: Vec3; t0: number; v: number };
export type Trail = { id: number; team?: Team; t0: number; end: number | null; z0: number; pts: number[][] };

export type State = {
  map: string | null;
  snaps: Snapshot[];
  /** Events not yet due (their tick is ahead of playback). */
  events: GameEvent[];
  effects: Effect[];
  /** Everyone seen this map, by id, as last seen. */
  players: Map<number, Player>;
  tickOffset: number | null;
  lastTick: number;
  /** When data last came in (performance.now()). */
  lastData?: number;
  geometry: Geometry | null;
  geoCanvas: { canvas: HTMLCanvasElement; lo: number[]; hi: number[] } | null;
  overview: Overview | null;
  spawns?: Spawns | null;
  /** Whether the map's data has come in (or is known to be missing). */
  mapData?: boolean;
  marks: Mark[];
  scorches: Scorch[];
  trails: Trail[];
  /** Health and ammo packs taken, by index into the geometry's packs: when they come back. */
  packsTaken: Map<number, number>;
  view: View;
  /** The framing of the whole map, and whether the view has been fitted and framed. */
  full?: Framing;
  fitted: boolean;
  framed?: boolean;
  autoZoom: boolean;
  camTarget?: Framing | null;
  /** The camera's spring velocities, and the aim it glides toward (which glides toward the framing). */
  cam?: {
    vx: number;
    vy: number;
    vz: number;
    aim?: { cx: number; cy: number; z: number; vx: number; vy: number; vz: number };
  } | null;
  bg: string;
  /** The followed player's id. */
  follow: number | null;
  lastPlayers: Player[];
  lastScores: GameEvent | null;
  /** The spawn room each team was last seen spawning in, and when. */
  spawnSeen: { [team: string]: { room: number; at: number } };
  lastPanel: number;
  /** The match clock as the feed shows it. */
  clock?: string;
};
