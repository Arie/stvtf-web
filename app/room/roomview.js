// How everyone in a room sees the map, as its admins set it, without the page around it: what
// a page does when a view comes in, when its admin chooses one, and when they move the camera or
// click a player. room.js connects it to the page.
//
// The rules:
// - Choosing (the menu, or clicking a player) sets the kind of view: free (each their own
//   camera), auto zoom, following a player, or an admin's view. Moving the camera never does:
//   in an admin's view any admin moves it by hand, the latest to move it leading. The server
//   holds to the same, so a move that crosses a choice on the way cannot undo it.
// - A page leads, not a person: an admin's two tabs are two pages.
// - Auto zoom is everyone's, the admin who chose it too. An admin's view is moved by hand: no auto
//   zoom or following, for that admin either. A followed player is everyone's, at the zoom of the
//   admin who clicked them, which they still scroll; stopping goes back to the view before.
// - Viewers cannot move their camera unless the view is free; admins always can.
// - A view is applied once when it changes, not on every camera move that comes in (many a
//   second): the admin may click a player between two of them.
// - Only what an admin does after a view is applied counts, not a move from before it.
// - A follow the page drops by itself (the match reset under it) is not sent: nobody chose it.

// An admin moving the camera this recently is moving it now: their page does not replay another's.
export const HAND_MS = 400;

/** @typedef {import("../../types/stream").ViewLine} ViewLine */
/** @typedef {{ mode: string; [k: string]: unknown }} View */
/** A view to send: `take` when chosen, otherwise a camera move. @typedef {{ view: View; take: boolean }} Send */
/** What the page's camera is doing. @typedef {{ follow: number | null; handMoved: number; now: number }} Seen */
/**
 * What the page does: lock the viewer's camera or not, replay the admin's camera or not, how
 * much of the map to show round a followed player (null: as it is), and, when the view changed,
 * auto zoom on or off and whom to follow.
 * @typedef {{ locked: boolean; mirror: boolean; zoom: { w: number; h: number } | null; auto?: boolean; follow?: number | null }} Effects
 */

export class RoomView {
  /** @param {string} page this page's name among its admin's pages */
  constructor(page) {
    this.page = page;
    /** @type {ViewLine} */
    this.line = { type: "view", mode: "free", by: 0, page: "" };
    this.admin = false;
    // The view as last applied, so a camera move coming in does not apply it again.
    this.applied = "";
    // Whom the room last had this page follow, so the admin's own click shows as a change.
    /** @type {number | null} */
    this.followSeen = null;
    // The view before an admin started following someone, to go back to.
    this.beforeFollow = "free";
    // The latest hand move that counted.
    this.handSent = 0;
  }

  leading() {
    return this.admin && this.line.page === this.page;
  }

  /** @param {Seen} seen */
  moving(seen) {
    return this.admin && seen.now - seen.handMoved < HAND_MS;
  }

  /** A view came in. @param {ViewLine} line @param {Seen} seen @returns {Effects} */
  hear(line, seen) {
    this.line = line;
    return this.apply(seen);
  }

  /** The page's admin rights changed. @param {boolean} on @param {Seen} seen @returns {Effects} */
  setAdmin(on, seen) {
    this.admin = on;
    return this.apply(seen);
  }

  /** @param {Seen} seen @returns {Effects} */
  apply(seen) {
    const v = this.line,
      lead = this.leading();
    const zoom = v.mode === "follow" && !lead && v.w && v.h ? { w: v.w, h: v.h } : null;
    /** @type {Effects} */
    const fx = {
      locked: v.mode !== "free" && !this.admin,
      mirror: v.mode === "admin" && !lead && !this.moving(seen),
      zoom,
    };
    const key = [v.mode, v.page, lead, v.mode === "follow" ? v.player : ""].join();
    if (key === this.applied) return fx;
    this.applied = key;
    this.handSent = Math.max(this.handSent, seen.handMoved);
    if (v.mode === "auto") Object.assign(fx, { auto: true, follow: null });
    else if (v.mode === "admin") Object.assign(fx, { auto: false, follow: null });
    else if (v.mode === "follow" && !lead) fx.follow = v.player;
    this.followSeen = fx.follow !== undefined ? fx.follow : seen.follow;
    return fx;
  }

  /**
   * The admin chose `mode` in the menu; `camera` is their view as an admin's view.
   * @param {string} mode @param {() => View} camera @returns {{ send: Send; fx: Partial<Effects> } | null}
   */
  choose(mode, camera) {
    if (!this.admin) return null;
    if (mode !== "admin") return { send: { view: { mode }, take: true }, fx: {} };
    this.followSeen = null;
    return { send: { view: camera(), take: true }, fx: { auto: false, follow: null } };
  }

  /** The page stopped following by itself (the match reset): nothing to send. */
  dropped() {
    this.followSeen = null;
  }

  /**
   * What the admin did with the camera since last asked: clicked a player or stopped following,
   * or moved it by hand.
   * @param {Seen} seen @param {() => View} camera @returns {Send | null}
   */
  tick(seen, camera) {
    if (!this.admin) return null;
    if (seen.follow !== this.followSeen) {
      this.followSeen = seen.follow;
      if (seen.follow != null) {
        if (this.line.mode !== "follow") this.beforeFollow = this.line.mode;
        return { view: following(seen.follow, camera()), take: true };
      }
      if (this.line.mode !== "follow") return null;
      return { view: this.beforeFollow === "admin" ? camera() : { mode: this.beforeFollow }, take: true };
    }
    if (seen.handMoved <= this.handSent) return null;
    this.handSent = seen.handMoved;
    if (this.line.mode === "admin") return { view: camera(), take: false };
    // Scrolled while following: everyone's zoom.
    if (this.line.mode === "follow" && seen.follow != null)
      return { view: following(seen.follow, camera()), take: false };
    return null;
  }
}

/** Following `player`, as much of the map round them as `camera` shows. @param {number} player @param {View} camera */
const following = (player, camera) => ({ mode: "follow", player, w: camera.w, h: camera.h });

/**
 * Where in the recording an event of game tick `tick` is, from where playback is now (`at`,
 * milliseconds of play, at game tick `now`): play time goes with ticks while a map plays.
 * @param {number} tick @param {{ at: number; now: number }} playing @param {number} msPerTick
 */
export const atTick = (tick, playing, msPerTick) =>
  Math.max(0, Math.round(playing.at - (playing.now - tick) * msPerTick));

/**
 * Sends one request at a time, so they arrive in order and the delay stays one round trip
 * however slow they are; `add` folds what comes in meanwhile into what waits.
 * @template T @param {(item: T) => Promise<unknown>} post @param {(waiting: T[], item: T) => T[]} add
 * @returns {(item: T) => void}
 */
export function sender(post, add) {
  /** @type {T[]} */
  let waiting = [];
  let busy = false;
  const next = () => {
    if (busy || !waiting.length) return;
    const item = /** @type {T} */ (waiting.shift());
    busy = true;
    post(item)
      .catch(() => {})
      .finally(() => {
        busy = false;
        next();
      });
  };
  return item => {
    waiting = add(waiting, item);
    next();
  };
}

/** Views waiting: a choice always goes (the latest), and after it only the latest camera move. @param {Send[]} waiting @param {Send} item */
export const latestView = (waiting, item) => (item.take ? [item] : [...waiting.filter(w => w.take), item]);

// Points in one drawing request, at most.
const MAX_POINTS = 256;
/** @typedef {{ stroke: number; points: [number, number, number][]; end?: boolean }} Strokes */
/** Points waiting join their stroke's. @param {Strokes[]} waiting @param {Strokes} item */
export function morePoints(waiting, item) {
  const last = waiting[waiting.length - 1];
  if (last?.stroke === item.stroke && last.points.length + item.points.length <= MAX_POINTS) {
    last.points.push(...item.points);
    if (item.end) last.end = true;
    return waiting;
  }
  return [...waiting, { ...item, points: [...item.points] }];
}
