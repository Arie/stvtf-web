import { msPerTick, state } from "./core.js";
import { banner, teamName, pointName, flagName, chat, feedEvent } from "./feed.js";
import { showTimeAdded } from "./hud.js";
import { layerOn } from "./layers.js";
import { noteSpawn } from "./objectives.js";
import { addScorch, nearFreshScorch } from "./overkill.js";

// ---------- events become effects when playback reaches their tick
// Playback can reach events late (a hidden tab holds the page back): effects are back-dated, so a late
// one shows only what is left of its life, and detail lines are dropped once they are too old.
export const MARK_LIFE_MS = 60000, MARK_CAP = 2000, SCORCH_CAP = 300, SCORCH_R = 65, PACK_REACH = 128, PACK_RESPAWN_MS = 10000;
// How long each effect shows, in ms.
const LIFE = { shot: 260, boom: 250, airblast: 350, sentryshot: 180, dmg: 900, kill: 3000, spawn: 700, uber: 1500,
  upgrade: 900, repair: 900, heal: 900, blood: 450, hit: 250, capbreak: 800 };
// Detail reached later than this is dropped: impacts sooner, the time-added float later.
const LATE_MS = 1500, LATE_IMPACT_MS = 600, LATE_TIME_ADDED_MS = 3000;
// Jump trails kept at once.
const MAX_TRAILS = 80;
let eventTick = 0;
const lagMs = ev => Math.max(0, (eventTick - ev.tick) * msPerTick);
const stale = (ev, ms) => lagMs(ev) > ms;
export function fireEvents(tick, byId) {
  eventTick = tick;
  const due = [];
  state.events = state.events.filter(ev => (ev.tick <= tick ? (due.push(ev), false) : true));
  const now = performance.now();
  for (const ev of due) {
    const p = id => byId.get(id) || state.players.get(id);
    switch (ev.type) {
      case "shot": state.effects.push({ kind: "shot", ev, team: p(ev.player)?.team, t0: now - lagMs(ev), life: LIFE.shot }); break;
      case "explosion":
        state.effects.push({ kind: "boom", ev, t0: now - lagMs(ev), life: LIFE.boom });
        if (layerOn("marks") && !stale(ev, MARK_LIFE_MS) && ev.origin) addScorch(ev.origin, now - lagMs(ev));
        break;
      case "airblast": state.effects.push({ kind: "airblast", id: ev.player, t0: now - lagMs(ev), life: LIFE.airblast }); break;
      case "sentry_shot": {
        const snap = state.snaps.at(-1);
        const sentry = snap?.buildings.find(b => b.id === ev.sentry);
        if (sentry) state.effects.push({ kind: "sentryshot", from: sentry.pos, aim: sentry.aim, team: sentry.team, target: ev.target, t0: now - lagMs(ev), life: LIFE.sentryshot });
        break;
      }
      case "damage": {
        const v = p(ev.victim);
        if (v?.pos) state.effects.push({ kind: "dmg", pos: v.pos, ev, t0: now - lagMs(ev), life: LIFE.dmg });
        break;
      }
      case "kill": {
        if (!ev.feign) state.effects.push({ kind: "kill", ev, team: p(ev.victim)?.team, t0: now - lagMs(ev), life: LIFE.kill });
        feedEvent(ev, p);
        break;
      }
      case "spawn":
        state.effects.push({ kind: "spawn", pos: ev.pos, team: ev.team, t0: now - lagMs(ev), life: LIFE.spawn });
        noteSpawn(ev);
        break;
      case "uber": {
        const m = p(ev.medic);
        if (m) state.effects.push({ kind: "uber", id: ev.medic, team: m.team, t0: now - lagMs(ev), life: LIFE.uber });
        feedEvent(ev, p);
        break;
      }
      case "time_added": if (!stale(ev, LATE_TIME_ADDED_MS)) showTimeAdded(ev); break;
      case "building_upgrade": case "building_repair": {
        if (stale(ev, LATE_MS) || (ev.type === "building_repair" && !layerOn("heals"))) break;
        const b = state.snaps.at(-1)?.buildings?.find(x => x.id === ev.id);
        if (b) state.effects.push({ kind: ev.type === "building_upgrade" ? "upgrade" : "repair", pos: b.pos, ev, t0: now - lagMs(ev), life: LIFE[ev.type === "building_upgrade" ? "upgrade" : "repair"] });
        break;
      }
      case "heal":
        if (layerOn("heals") && !stale(ev, LATE_MS) && p(ev.patient)?.pos) state.effects.push({ kind: "heal", id: ev.patient, ev, t0: now - lagMs(ev), life: LIFE.heal });
        break;
      case "impact":
        if (layerOn("hits") && !stale(ev, LATE_IMPACT_MS) && ev.pos) state.effects.push({ kind: "hit", ev, t0: now - lagMs(ev), life: ev.kind === "blood" ? LIFE.blood : LIFE.hit });
        break;
      case "mark":
        if (layerOn("marks") && !stale(ev, MARK_LIFE_MS) && ev.pos && !nearFreshScorch(ev.pos, now)) {
          state.marks.push({ pos: ev.pos, t0: now - lagMs(ev) });
          if (state.marks.length > MARK_CAP) state.marks.splice(0, state.marks.length - MARK_CAP);
        }
        break;
      case "blast_jump": {
        if (!layerOn("jumps") || stale(ev, LATE_MS)) break;
        const open = state.trails.find(t => t.id === ev.player && !t.end);
        if (ev.phase === "start") {
          if (open) open.end = now;
          const pl = p(ev.player);
          state.trails.push({ id: ev.player, team: pl?.team, t0: now, end: null, z0: pl?.pos?.[2] ?? 0, pts: [] });
          if (state.trails.length > MAX_TRAILS) state.trails.shift();
        } else if (open) open.end = now;
        break;
      }
      case "pickup": {
        if (!layerOn("pickups") || stale(ev, LATE_MS)) break;
        const pl = p(ev.player), packs = state.geometry?.packs;
        if (!pl?.pos || !packs) break;
        let best = -1, bd = PACK_REACH;
        packs.forEach((k, i) => {
          if (k.kind !== ev.item) return;
          const d = Math.hypot(k.pos[0] - pl.pos[0], k.pos[1] - pl.pos[1], (k.pos[2] - pl.pos[2]) * 0.5);
          if (d < bd) { bd = d; best = i; }
        });
        if (best < 0) break;
        state.packsTaken.set(best, now + PACK_RESPAWN_MS);
        break;
      }
      case "capture_broken": {
        feedEvent(ev, p);
        if (!layerOn("points") || stale(ev, LATE_MS)) break;
        const o = state.snaps.at(-1)?.objectives?.find(x => x.kind === "control_point" && x.index === ev.point);
        if (o) state.effects.push({ kind: "capbreak", pos: o.pos, t0: now - lagMs(ev), life: LIFE.capbreak });
        break;
      }
      case "point_captured": banner(`${teamName(ev.team)} captured ${pointName(ev)}`, ev.team); feedEvent(ev, p); break;
      case "flag": {
        const team = p(ev.player)?.team;
        if (ev.event === "captured") banner(`${teamName(team)} captured ${flagName(ev.team)}`, team || "none");
        feedEvent(ev, p);
        break;
      }
      case "round_win": banner(`${teamName(ev.team)} wins the round`, ev.team); feedEvent(ev, p); break;
      case "round_start":
        if (!stale(ev, LATE_MS)) { state.scorches = []; state.marks = []; state.trails = []; }
        banner("Round start", "none"); feedEvent(ev, p); break;
      case "chat": case "voice_command": case "server_text": chat(ev, p); break;
      default: feedEvent(ev, p);
    }
  }
}
