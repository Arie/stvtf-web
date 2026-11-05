import { msPerTick, state } from "./core.js";
import { embedEvent } from "./embed.js";
import { banner, teamName, pointName, flagName, chat, feedEvent } from "./feed.js";
import { showTimeAdded } from "./hud.js";
import { layerOn } from "./layers.js";
import { addScorch, nearFreshScorch } from "./overkill.js";
import { noteSpawn } from "./spawns.js";
/** @typedef {import("../types/stream").Player} Player */
/** @typedef {import("../types/stream").GameEvent} GameEvent */
/** @typedef {(id: number) => Player | undefined} Lookup A player by id. */

// ---------- events become effects when playback reaches their tick
// Playback can reach events late (a hidden tab holds the page back): effects are back-dated, so a late
// one shows only what is left of its life, and detail lines are dropped once they are too old.
export const MARK_LIFE_MS = 60000,
  MARK_CAP = 2000,
  SCORCH_CAP = 300,
  SCORCH_R = 65,
  PACK_REACH = 128,
  PACK_RESPAWN_MS = 10000;
// How long each effect shows, in ms.
const LIFE = {
  shot: 260,
  boom: 250,
  airblast: 350,
  sentryshot: 180,
  dmg: 900,
  kill: 3000,
  spawn: 700,
  uber: 1500,
  upgrade: 900,
  repair: 900,
  heal: 900,
  blood: 450,
  hit: 250,
  capbreak: 800,
};
// Detail reached later than this is dropped: impacts sooner, the time-added float later.
const LATE_MS = 1500,
  LATE_IMPACT_MS = 600,
  LATE_TIME_ADDED_MS = 3000;
// Jump trails kept at once.
const MAX_TRAILS = 80;
let eventTick = 0;
/** @param {GameEvent} ev */
const lagMs = ev => Math.max(0, (eventTick - ev.tick) * msPerTick);
/** @param {GameEvent} ev @param {number} ms */
const stale = (ev, ms) => lagMs(ev) > ms;
// An effect of `kind` that began when `ev` happened, for as long as LIFE gives it (or `life` ms).
/**
 * @param {keyof typeof LIFE} kind @param {GameEvent} ev @param {number} now @param {object} fields
 * @param {number} [life]
 */
function addEffect(kind, ev, now, fields, life = LIFE[kind]) {
  state.effects.push({ kind, ...fields, t0: now - lagMs(ev), life });
}
/** @param {GameEvent} ev @param {Lookup} p @param {number} now */
function buildingWork(ev, p, now) {
  if (stale(ev, LATE_MS) || (ev.type === "building_repair" && !layerOn("heals"))) return;
  const b = state.snaps.at(-1)?.buildings?.find(x => x.id === ev.id);
  if (b) addEffect(ev.type === "building_upgrade" ? "upgrade" : "repair", ev, now, { pos: b.pos, ev });
}
// A blast jump's start opens a trail (closing one still open); its end closes the open one.
/** @param {GameEvent} ev @param {Lookup} p @param {number} now */
function blastJump(ev, p, now) {
  if (!layerOn("jumps") || stale(ev, LATE_MS)) return;
  const open = state.trails.find(t => t.id === ev.player && !t.end);
  if (ev.phase === "start") {
    if (open) open.end = now;
    const pl = p(ev.player);
    state.trails.push({ id: ev.player, team: pl?.team, t0: now, end: null, z0: pl?.pos?.[2] ?? 0, pts: [] });
    if (state.trails.length > MAX_TRAILS) state.trails.shift();
  } else if (open) open.end = now;
}
// A pickup takes the nearest pack of its kind in reach, which shows as taken until it respawns.
/** @param {GameEvent} ev @param {Lookup} p @param {number} now */
function pickup(ev, p, now) {
  if (!layerOn("pickups") || stale(ev, LATE_MS)) return;
  const pos = p(ev.player)?.pos,
    packs = state.geometry?.packs;
  if (!pos || !packs) return;
  let best = -1,
    bd = PACK_REACH;
  packs.forEach((k, i) => {
    if (k.kind !== ev.item) return;
    const d = Math.hypot(k.pos[0] - pos[0], k.pos[1] - pos[1], (k.pos[2] - pos[2]) * 0.5);
    if (d < bd) {
      bd = d;
      best = i;
    }
  });
  if (best < 0) return;
  state.packsTaken.set(best, now + PACK_RESPAWN_MS);
}
/** @param {GameEvent} ev @param {Lookup} p */
const chatLine = (ev, p) => chat(ev, p);
/** @param {GameEvent} ev @param {Lookup} p */
const feedLine = (ev, p) => feedEvent(ev, p);
// What each event type does when playback reaches it, given the event, the player lookup and the time
// now. Any other type only goes to the feed.
/** @type {{ [type: string]: (ev: GameEvent, p: Lookup, now: number) => void }} */
const ON = {
  shot(ev, p, now) {
    addEffect("shot", ev, now, { ev, team: p(ev.player)?.team });
  },
  explosion(ev, p, now) {
    addEffect("boom", ev, now, { ev });
    if (layerOn("marks") && !stale(ev, MARK_LIFE_MS) && ev.origin) addScorch(ev.origin, now - lagMs(ev));
  },
  airblast(ev, p, now) {
    addEffect("airblast", ev, now, { id: ev.player });
  },
  sentry_shot(ev, p, now) {
    const snap = state.snaps.at(-1);
    const sentry = snap?.buildings.find(b => b.id === ev.sentry);
    if (sentry)
      addEffect("sentryshot", ev, now, { from: sentry.pos, aim: sentry.aim, team: sentry.team, target: ev.target });
  },
  damage(ev, p, now) {
    const v = p(ev.victim);
    if (v?.pos) addEffect("dmg", ev, now, { pos: v.pos, ev });
  },
  kill(ev, p, now) {
    if (!ev.feign) addEffect("kill", ev, now, { ev, team: p(ev.victim)?.team });
    feedEvent(ev, p);
  },
  spawn(ev, p, now) {
    addEffect("spawn", ev, now, { pos: ev.pos, team: ev.team });
    noteSpawn(ev);
  },
  uber(ev, p, now) {
    const m = p(ev.medic);
    if (m) addEffect("uber", ev, now, { id: ev.medic, team: m.team });
    feedEvent(ev, p);
  },
  time_added(ev) {
    if (!stale(ev, LATE_TIME_ADDED_MS)) showTimeAdded(ev);
  },
  building_upgrade: buildingWork,
  building_repair: buildingWork,
  heal(ev, p, now) {
    if (layerOn("heals") && !stale(ev, LATE_MS) && p(ev.patient)?.pos)
      addEffect("heal", ev, now, { id: ev.patient, ev });
  },
  impact(ev, p, now) {
    if (layerOn("hits") && !stale(ev, LATE_IMPACT_MS) && ev.pos)
      addEffect("hit", ev, now, { ev }, ev.kind === "blood" ? LIFE.blood : LIFE.hit);
  },
  mark(ev, p, now) {
    if (layerOn("marks") && !stale(ev, MARK_LIFE_MS) && ev.pos && !nearFreshScorch(ev.pos, now)) {
      state.marks.push({ pos: ev.pos, t0: now - lagMs(ev) });
      if (state.marks.length > MARK_CAP) state.marks.splice(0, state.marks.length - MARK_CAP);
    }
  },
  blast_jump: blastJump,
  pickup,
  capture_broken(ev, p, now) {
    feedEvent(ev, p);
    if (!layerOn("points") || stale(ev, LATE_MS)) return;
    const o = state.snaps.at(-1)?.objectives?.find(x => x.kind === "control_point" && x.index === ev.point);
    if (o) addEffect("capbreak", ev, now, { pos: o.pos });
  },
  point_captured(ev, p) {
    banner(`${teamName(ev.team)} captured ${pointName(ev)}`, ev.team);
    feedEvent(ev, p);
  },
  flag(ev, p) {
    const team = p(ev.player)?.team;
    if (ev.event === "captured") banner(`${teamName(team)} captured ${flagName(ev.team)}`, team || "none");
    feedEvent(ev, p);
  },
  round_win(ev, p) {
    banner(`${teamName(ev.team)} wins the round`, ev.team);
    feedEvent(ev, p);
  },
  round_start(ev, p) {
    if (!stale(ev, LATE_MS)) {
      state.scorches = [];
      state.marks = [];
      state.trails = [];
    }
    banner("Round start", "none");
    feedEvent(ev, p);
  },
  chat: chatLine,
  voice_command: chatLine,
  server_text: chatLine,
};
/** @param {number} tick @param {Map<number, Player>} byId */
export function fireEvents(tick, byId) {
  eventTick = tick;
  /** @type {GameEvent[]} */
  const due = [];
  state.events = state.events.filter(ev => (ev.tick <= tick ? (due.push(ev), false) : true));
  const now = performance.now();
  for (const ev of due) {
    const p = /** @param {number} id */ id => byId.get(id) || state.players.get(id);
    embedEvent(ev, byId);
    const on = Object.hasOwn(ON, ev.type) ? ON[ev.type] : feedLine;
    on(ev, p, now);
  }
}
