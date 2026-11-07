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
  rounds: number[];
};

/** A piece of a room's telestrator stroke, in map coordinates. */
export type DrawLine = { type: "draw"; stroke: number; points: [number, number][]; by?: number };

/** Who is in a room, in the order they came; `id` is their number in it. */
export type PeopleLine = {
  type: "people";
  people: { id: number; name: string; admin: boolean; owner: boolean }[];
};

/** How a room's admin `by` has everyone see the map. */
export type ViewLine = { type: "view"; by: number } & (
  | { mode: "free" | "auto" }
  | { mode: "follow"; player: number }
  | { mode: "admin"; cx: number; cy: number; w: number; h: number; rot: number }
);

export type StreamMessage =
  | Snapshot
  | ServerLine
  | StatusLine
  | PlaybackLine
  | DrawLine
  | PeopleLine
  | ViewLine
  | { type: "data_age"; ms: number }
  | { type: "seek" }
  | GameEvent;
