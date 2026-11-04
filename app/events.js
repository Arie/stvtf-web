import { noteCombat } from "./camera.js";
import { msPerTick, state } from "./core.js";
import { embedEvent } from "./embed.js";
import { banner, teamName, pointName, flagName, chat, feedEvent } from "./feed.js";
import { showTimeAdded } from "./hud.js";
import { layerOn } from "./layers.js";
import { addScorch, nearFreshScorch } from "./overkill.js";
import { noteSpawn } from "./spawns.js";
/** @typedef {import("../types/stream").Player} Player */

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
const lagMs = ev => Math.max(0, (eventTick - ev.tick) * msPerTick);
const stale = (ev, ms) => lagMs(ev) > ms;
function buildingWork(ev, p, now) {
  if (stale(ev, LATE_MS) || (ev.type === "building_repair" && !layerOn("heals"))) return;
  const b = state.snaps.at(-1)?.buildings?.find(x => x.id === ev.id);
  if (b)
    state.effects.push({
      kind: ev.type === "building_upgrade" ? "upgrade" : "repair",
      pos: b.pos,
      ev,
      t0: now - lagMs(ev),
      life: LIFE[ev.type === "building_upgrade" ? "upgrade" : "repair"],
    });
}
// A blast jump's start opens a trail (closing one still open); its end closes the open one.
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
function pickup(ev, p, now) {
  if (!layerOn("pickups") || stale(ev, LATE_MS)) return;
  const pl = p(ev.player),
    packs = state.geometry?.packs;
  if (!pl?.pos || !packs) return;
  let best = -1,
    bd = PACK_REACH;
  packs.forEach((k, i) => {
    if (k.kind !== ev.item) return;
    const d = Math.hypot(k.pos[0] - pl.pos[0], k.pos[1] - pl.pos[1], (k.pos[2] - pl.pos[2]) * 0.5);
    if (d < bd) {
      bd = d;
      best = i;
    }
  });
  if (best < 0) return;
  state.packsTaken.set(best, now + PACK_RESPAWN_MS);
}
const chatLine = (ev, p) => chat(ev, p);
const feedLine = (ev, p) => feedEvent(ev, p);
// What each event type does when playback reaches it, given the event, the player lookup and the time
// now. Any other type only goes to the feed.
const ON = {
  shot(ev, p, now) {
    state.effects.push({ kind: "shot", ev, team: p(ev.player)?.team, t0: now - lagMs(ev), life: LIFE.shot });
  },
  explosion(ev, p, now) {
    state.effects.push({ kind: "boom", ev, t0: now - lagMs(ev), life: LIFE.boom });
    if (layerOn("marks") && !stale(ev, MARK_LIFE_MS) && ev.origin) addScorch(ev.origin, now - lagMs(ev));
  },
  airblast(ev, p, now) {
    state.effects.push({ kind: "airblast", id: ev.player, t0: now - lagMs(ev), life: LIFE.airblast });
  },
  sentry_shot(ev, p, now) {
    const snap = state.snaps.at(-1);
    const sentry = snap?.buildings.find(b => b.id === ev.sentry);
    if (sentry)
      state.effects.push({
        kind: "sentryshot",
        from: sentry.pos,
        aim: sentry.aim,
        team: sentry.team,
        target: ev.target,
        t0: now - lagMs(ev),
        life: LIFE.sentryshot,
      });
  },
  damage(ev, p, now) {
    const v = p(ev.victim);
    if (v?.pos) state.effects.push({ kind: "dmg", pos: v.pos, ev, t0: now - lagMs(ev), life: LIFE.dmg });
  },
  kill(ev, p, now) {
    if (!ev.feign)
      state.effects.push({ kind: "kill", ev, team: p(ev.victim)?.team, t0: now - lagMs(ev), life: LIFE.kill });
    feedEvent(ev, p);
  },
  spawn(ev, p, now) {
    state.effects.push({ kind: "spawn", pos: ev.pos, team: ev.team, t0: now - lagMs(ev), life: LIFE.spawn });
    noteSpawn(ev);
  },
  uber(ev, p, now) {
    const m = p(ev.medic);
    if (m) state.effects.push({ kind: "uber", id: ev.medic, team: m.team, t0: now - lagMs(ev), life: LIFE.uber });
    feedEvent(ev, p);
  },
  time_added(ev) {
    if (!stale(ev, LATE_TIME_ADDED_MS)) showTimeAdded(ev);
  },
  building_upgrade: buildingWork,
  building_repair: buildingWork,
  heal(ev, p, now) {
    if (layerOn("heals") && !stale(ev, LATE_MS) && p(ev.patient)?.pos)
      state.effects.push({ kind: "heal", id: ev.patient, ev, t0: now - lagMs(ev), life: LIFE.heal });
  },
  impact(ev, p, now) {
    if (layerOn("hits") && !stale(ev, LATE_IMPACT_MS) && ev.pos)
      state.effects.push({
        kind: "hit",
        ev,
        t0: now - lagMs(ev),
        life: ev.kind === "blood" ? LIFE.blood : LIFE.hit,
      });
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
    if (o) state.effects.push({ kind: "capbreak", pos: o.pos, t0: now - lagMs(ev), life: LIFE.capbreak });
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
  const due = [];
  state.events = state.events.filter(ev => (ev.tick <= tick ? (due.push(ev), false) : true));
  const now = performance.now();
  for (const ev of due) {
    const p = id => byId.get(id) || state.players.get(id);
    embedEvent(ev, byId);
    noteCombat(ev, now);
    const on = Object.hasOwn(ON, ev.type) ? ON[ev.type] : feedLine;
    on(ev, p, now);
  }
}
