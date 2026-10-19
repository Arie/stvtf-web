import { MS_PER_TICK, state } from "./core.js";
import { banner, teamName, pointName, flagName, chat, feedEvent } from "./feed.js";
import { layerOn } from "./layers.js";
import { noteSpawn } from "./objectives.js";
import { addScorch, nearFreshScorch } from "./overkill.js";

// ---------- events become effects when playback reaches their tick
// Playback can reach events late (a hidden tab holds the page back): effects are back-dated, so a late
// one shows only what is left of its life, and detail lines are dropped once they are too old.
export const MARK_LIFE_MS = 60000, MARK_CAP = 2000, SCORCH_CAP = 300, SCORCH_R = 65, PACK_REACH = 128, PACK_RESPAWN_MS = 10000;
let eventTick = 0;
const lagMs = ev => Math.max(0, (eventTick - ev.tick) * MS_PER_TICK);
const stale = (ev, ms) => lagMs(ev) > ms;
export function fireEvents(tick, byId) {
  eventTick = tick;
  const due = [];
  state.events = state.events.filter(ev => (ev.tick <= tick ? (due.push(ev), false) : true));
  const now = performance.now();
  for (const ev of due) {
    const p = id => byId.get(id) || state.players.get(id);
    switch (ev.type) {
      case "shot": state.effects.push({ kind: "shot", ev, team: p(ev.player)?.team, t0: now - lagMs(ev), life: 260 }); break;
      case "explosion":
        state.effects.push({ kind: "boom", ev, t0: now - lagMs(ev), life: 450 });
        if (layerOn("marks") && !stale(ev, MARK_LIFE_MS) && ev.origin) addScorch(ev.origin, now - lagMs(ev));
        break;
      case "airblast": state.effects.push({ kind: "airblast", id: ev.player, t0: now - lagMs(ev), life: 350 }); break;
      case "sentry_shot": {
        const snap = state.snaps.at(-1);
        const sentry = snap?.buildings.find(b => b.id === ev.sentry);
        if (sentry) state.effects.push({ kind: "sentryshot", from: sentry.pos, aim: sentry.aim, team: sentry.team, target: ev.target, t0: now - lagMs(ev), life: 180 });
        break;
      }
      case "damage": {
        const v = p(ev.victim);
        if (v?.pos) state.effects.push({ kind: "dmg", pos: v.pos, ev, t0: now - lagMs(ev), life: 900 });
        break;
      }
      case "kill": {
        if (!ev.feign) state.effects.push({ kind: "kill", ev, team: p(ev.victim)?.team, t0: now - lagMs(ev), life: 3000 });
        feedEvent(ev, p);
        break;
      }
      case "spawn":
        state.effects.push({ kind: "spawn", pos: ev.pos, team: ev.team, t0: now - lagMs(ev), life: 700 });
        noteSpawn(ev);
        break;
      case "uber": {
        const m = p(ev.medic);
        if (m) state.effects.push({ kind: "uber", id: ev.medic, team: m.team, t0: now - lagMs(ev), life: 1500 });
        feedEvent(ev, p);
        break;
      }
      case "heal":
        if (layerOn("heals") && !stale(ev, 1500) && p(ev.patient)?.pos) state.effects.push({ kind: "heal", id: ev.patient, ev, t0: now - lagMs(ev), life: 900 });
        break;
      case "impact":
        if (layerOn("hits") && !stale(ev, 600) && ev.pos) state.effects.push({ kind: "hit", ev, t0: now - lagMs(ev), life: ev.kind === "blood" ? 450 : 250 });
        break;
      case "mark":
        if (layerOn("marks") && !stale(ev, MARK_LIFE_MS) && ev.pos && !nearFreshScorch(ev.pos, now)) {
          state.marks.push({ pos: ev.pos, t0: now - lagMs(ev) });
          if (state.marks.length > MARK_CAP) state.marks.splice(0, state.marks.length - MARK_CAP);
        }
        break;
      case "blast_jump": {
        if (!layerOn("jumps") || stale(ev, 1500)) break;
        const open = state.trails.find(t => t.id === ev.player && !t.end);
        if (ev.phase === "start") {
          if (open) open.end = now;
          const pl = p(ev.player);
          state.trails.push({ id: ev.player, team: pl?.team, t0: now, end: null, z0: pl?.pos?.[2] ?? 0, pts: [] });
          if (state.trails.length > 80) state.trails.shift();
        } else if (open) open.end = now;
        break;
      }
      case "pickup": {
        if (!layerOn("pickups") || stale(ev, 1500)) break;
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
        state.effects.push({ kind: "packflash", pos: packs[best].pos, t0: now - lagMs(ev), life: 600 });
        break;
      }
      case "capture_broken": {
        feedEvent(ev, p);
        if (!layerOn("points") || stale(ev, 1500)) break;
        const o = state.snaps.at(-1)?.objectives?.find(o => o.kind === "control_point" && o.index === ev.point);
        if (o) state.effects.push({ kind: "capbreak", pos: o.pos, t0: now - lagMs(ev), life: 800 });
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
        if (!stale(ev, 1500)) { state.scorches = []; state.marks = []; state.trails = []; }
        banner("Round start", "none"); feedEvent(ev, p); break;
      case "chat": case "voice_command": case "server_text": chat(ev, p); break;
      default: feedEvent(ev, p);
    }
  }
}
