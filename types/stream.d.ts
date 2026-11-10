// What the page receives on its event stream: stvtf's output (its README's "Output" section),
// plus the lines the server adds. Only `tsc` reads this file; the page is plain JavaScript.

export type Vec3 = [number, number, number];
export type Team = "red" | "blue" | "spectator" | "none";
export type TeamScore = { red: number; blue: number };

export type Medigun = {
  kind: "medigun" | "kritzkrieg" | "quickfix" | "vaccinator";
  releasing: boolean;
  resist: "bullet" | "blast" | "fire" | null;
  target: number | null;
};

export type PlayerScore = {
  points: number;
  kills: number;
  deaths: number;
  assists: number;
  damage: number;
  healing: number;
  dominations: number;
  streak: number;
};

/** A player: where they are while alive, or where they died (null if before the tool connected). */
export type Player = PlayerFields & ({ alive: true; pos: Vec3 } | { alive: false; pos: Vec3 | null });

export type LivePlayer = Player & { alive: true };

type PlayerFields = {
  id: number;
  ent: number;
  name: string;
  steamid: string;
  team: Team;
  /** Lower-case class name, or "unknown". */
  class: string;
  hp: number;
  max_hp: number;
  yaw: number | null;
  pitch: number | null;
  /** Charge percentage. */
  uber: number;
  ping: number;
  conds: string[];
  /** The medigun charge they are under. */
  ubered: string | null;
  medigun: Medigun | null;
  weapon: { item: number; clip: number | null } | null;
  flames: { crit: boolean; hit: boolean } | null;
  /** Seconds until a dead player respawns, once the server has set it. */
  respawn?: number;
  score?: PlayerScore;
  carrying?: "sentry" | "dispenser" | "teleporter";
};

export type Projectile = {
  id: number;
  kind: string;
  team: Team;
  pos: Vec3;
  yaw: number;
  owner: number;
  crit: boolean;
};

export type Building = {
  id: number;
  kind: "sentry" | "dispenser" | "teleporter";
  team: Team;
  level: number;
  hp: number;
  max_hp: number;
  pos: Vec3;
  yaw: number;
  builder: number;
  sapped: boolean;
  building: boolean;
  mini?: boolean;
  entrance?: boolean;
  aim?: number;
  target?: number | null;
  attacking?: boolean;
};

/**
 * Control points, carts, flags and the game modes' own objectives; `kind` tells them apart.
 * Only a control point can be without a position.
 */
export type Objective = ObjectiveFields &
  (
    | { kind: "control_point"; pos: Vec3 | null }
    | { kind: "cart" | "flag" | "ball" | "goal" | "robot" | "core" | "revive" | "money" | "tank"; pos: Vec3 }
  );
/** An objective that has a position. */
export type PlacedObjective = Objective & { pos: Vec3 };

type ObjectiveFields = {
  id?: number;
  index?: number;
  team?: Team;
  owner?: Team | number;
  [field: string]: any;
};

export type Timer = { team: Team; remaining: number; paused: boolean; setup: boolean };

// Stopwatch: `setting` in the first half (`time` as of the last capture), then counting down to beat it.
export type Stopwatch = {
  attackers: Team;
  setting: boolean;
  state: "unset" | "running" | "overtime";
  time: number;
  paused: boolean;
  points: number;
  /** Its timer is the round timer the HUD shows: the time is there already. */
  in_hud?: boolean;
};

export type Snapshot = {
  type: "snapshot";
  tick: number;
  players: Player[];
  projectiles: Projectile[];
  buildings: Building[];
  objectives: Objective[];
  round: string;
  round_time: number | null;
  match_time: number | null;
  time_left: number | null;
  win_limit: number;
  score: TeamScore;
  timers?: Timer[];
  stopwatch?: Stopwatch;
  captures?: TeamScore;
  points?: { red: number; blue: number; max: number | null; [field: string]: any };
  mvm?: { [field: string]: any };
};

export type ServerLine = {
  type: "server";
  tick: number;
  map: string;
  hostname: string;
  tick_interval: number;
  max_players: number;
  spawn_count: number;
};

export type StatusLine = { type: "status"; state: string; reason?: string; [field: string]: any };

/**
 * A game event (kill, damage, spawn, chat, …). Their fields differ by `type`; see stvtf's README.
 * `history` marks one from before the page opened, with `names` for players who have left.
 */
export type GameEvent = {
  type: string;
  tick: number;
  history?: boolean;
  names?: { [id: number]: { name: string; team: Team } };
  [field: string]: any;
};

/** A room's playback: times in milliseconds of play. */
export type PlaybackLine = {
  type: "playback";
  paused: boolean;
  speed: number;
  at: number;
  length: number;
  /** What is kept, from `start`, of the map that began at `origin`: a new map drops the one before. */
  start?: number;
  origin?: number;
  rounds: number[];
  /** A live match, still being recorded: its length grows. */
  live: boolean;
};

/** A piece of a room's telestrator stroke: map points with when they were drawn, in ms into the stroke. */
export type DrawLine = {
  type: "draw";
  stroke: number;
  points: [number, number, number][];
  by?: number;
};

/** The permanent marker on a room's map, as a page joins. */
export type MarkerLine = {
  type: "marker";
  strokes: {
    stroke: number;
    by: number;
    points: [number, number, number][];
    ink?: string;
    arc?: [number, number, number][];
    circle?: [number, number, number];
    light?: [number, number, number];
    note?: { text: string; player?: number };
  }[];
  /** Points the map holds, all strokes together. */
  max?: number;
};

/** A jump arc an admin drew: its points from launch to landing, in the drawer's ink. */
export type ArcLine = { type: "arc"; by: number; stroke: number; ink: string; points: [number, number, number][] };

/** A circle an admin put down: its centre on the floor, in the drawer's ink. */
export type CircleLine = { type: "circle"; by: number; stroke: number; ink: string; at: [number, number, number] };

/** The spotlight an admin put down, in place of where it was: its spot on the floor, in the drawer's ink. */
export type LightLine = { type: "light"; by: number; stroke: number; ink: string; at: [number, number, number] };

/** A note an admin put down: at a spot on the floor, or on a player (user id), in the drawer's ink. */
export type NoteLine = {
  type: "note";
  by: number;
  stroke: number;
  ink: string;
  text: string;
  at: [number, number, number] | null;
  player: number | null;
};

/** A player talking: their Opus frames (base64, 20 ms each), with `?voice=1`. */
export type VoiceLine = {
  type: "voice";
  tick: number;
  player: number | null;
  steamid: string;
  rate: number;
  frames: string[];
};

/** An admin wiped the room's map. */
export type ClearLine = { type: "clear"; by: number };

/** Who is in a room, in the order they came; `id` is their number in it. */
export type PeopleLine = {
  type: "people";
  people: {
    id: number;
    name: string;
    admin: boolean;
    owner: boolean;
    signed: boolean;
    steamid?: string;
    ink?: string;
  }[];
};

/** How a room's admin `by` has everyone see the map. */
export type ViewLine = { type: "view"; by: number; page: string } & (
  | { mode: "free" | "auto" }
  | { mode: "follow"; player: number; w?: number; h?: number }
  | { mode: "admin"; cx: number; cy: number; w: number; h: number; rot: number }
);

/** How to watch a room in TF2: its relay's host:port (`connect`, for the console) and IP:port (`address`, for a
 * steam:// link, which takes no names), its password, and why it cannot play, if it cannot. No `connect`: none. */
export type RelayLine = {
  type: "relay";
  connect?: string;
  address?: string | null;
  password?: string;
  problem?: string | null;
};

export type StreamMessage =
  | Snapshot
  | ServerLine
  | StatusLine
  | PlaybackLine
  | DrawLine
  | MarkerLine
  | ArcLine
  | CircleLine
  | LightLine
  | NoteLine
  | VoiceLine
  | ClearLine
  | PeopleLine
  | ViewLine
  | RelayLine
  | { type: "data_age"; ms: number }
  | { type: "seek" }
  | GameEvent;
