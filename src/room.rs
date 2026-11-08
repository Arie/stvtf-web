//! A room: a recording played into its own hub, paused, sped up and sought by its owner, with
//! everyone in it seeing the same.

use crate::hub::{SharedHub, announce, publish, restore, send, send_keeping};
use crate::lock;
use crate::people::People;
use crate::recording::Recording;
use crate::relays::{Offer, RoomRelay, Was};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use stvtf::Line;

pub(crate) const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
/// How often pages hear where playback is while it plays.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(1);
/// How often a room nobody watches looks again, and one at a live match's end for more of it.
const IDLE_POLL: Duration = Duration::from_millis(250);
const LIVE_POLL: Duration = Duration::from_millis(20);
/// A relay that could not start is tried again after this long.
const RELAY_RETRY: Duration = Duration::from_secs(5);
/// A relay is told again when off by more than this (it jumps at a second).
const RELAY_DRIFT: Duration = Duration::from_millis(250);

pub(crate) struct Room {
    pub(crate) id: String,
    pub(crate) hub: SharedHub,
    people: Mutex<People>,
    /// The number of the admin who set the view last by choosing it.
    /// The admin's number, page and the kind of view it chose, when one did.
    leader: Mutex<Option<Leader>>,
    /// The latest view line, to save.
    view: Mutex<Option<Arc<str>>>,
    recording: Arc<Recording>,
    state: Mutex<State>,
    wake: Condvar,
    viewers: AtomicUsize,
    idle_since: Mutex<Option<Instant>>,
    /// Where its relay would play from, if it can have one.
    offer: OnceLock<Offer>,
    /// Its relay, while the room is open and someone has been on a page of it.
    relay: Mutex<Option<RoomRelay>>,
    /// What its pages were last told of the relay.
    relay_said: Mutex<Option<String>>,
    /// What its relay was last told.
    relay_sent: Mutex<Option<stvtf::relay::Want>>,
    /// Its relay's port and password, for one started again.
    relay_was: Mutex<Option<Was>>,
    /// The last relay that could not start: when, and why, so it is tried again and logged at a
    /// pace (the relay process may be away for a while).
    relay_failed: Mutex<Option<(Instant, String)>>,
    /// The user id the view follows; the relay's camera does too.
    camera: Mutex<Option<u16>>,
    /// The permanent marker strokes on the map.
    marker: Mutex<Vec<Kept>>,
}

struct State {
    paused: bool,
    speed: f64,
    /// Play time, in milliseconds.
    at: u64,
    seek: Option<u64>,
    stop: bool,
    /// The server tick of the last snapshot played, since the last seek.
    tick: Option<u32>,
}

/// What an owner changes; unset fields stay.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Control {
    pub(crate) paused: Option<bool>,
    pub(crate) speed: Option<f64>,
    pub(crate) seek: Option<u64>,
}

/// Most points in one drawing request.
const MAX_POINTS: usize = 256;

/// How everyone in the room sees the map, as an admin sets it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub(crate) enum View {
    /// Each their own camera.
    Free,
    /// Everyone on auto zoom.
    Auto,
    /// Everyone following one player, seeing as much of the map round them as the admin: `w` by `h`.
    Follow {
        player: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        w: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        h: Option<f64>,
    },
    /// Everyone seeing what the admin sees: the map area `w` by `h` round `cx`, `cy`, turned by `rot`.
    Admin { cx: f64, cy: f64, w: f64, h: f64, rot: f64 },
}

/// A view as an admin's page sends it: `take` when they chose it, which sets the kind of view;
/// otherwise it is their camera moving, which counts only while the view is of that kind (a move
/// sent before someone chose another no longer stands). Either way that page leads from then on:
/// the latest to choose or move it. `page` tells an admin's pages apart (two tabs, say).
#[derive(Debug, Deserialize)]
pub(crate) struct ViewPost {
    #[serde(flatten)]
    pub(crate) view: View,
    #[serde(default)]
    pub(crate) take: bool,
    #[serde(default)]
    pub(crate) page: String,
}

/// The longest page name.
const MAX_PAGE: usize = 32;

/// A piece of a telestrator stroke: points in map coordinates, each with when it was drawn, in
/// milliseconds into the stroke, so pages draw it as it was drawn.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draw {
    pub(crate) stroke: u32,
    pub(crate) points: Vec<[f64; 3]>,
    /// Permanent marker: stays until cleared.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) keep: bool,
}

/// A permanent marker stroke, as drawn so far.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Kept {
    stroke: u32,
    by: u32,
    points: Vec<[f64; 3]>,
}

/// Most permanent marker points on a room's map.
pub(crate) const MAX_KEPT_POINTS: usize = 8000;

/// The longest a stroke's points can say it took, in milliseconds.
const MAX_STROKE_MS: f64 = 600_000.0;

type Leader = (u32, String, String);

/// The marker for joiners, empty too: a page back after a clear it missed drops its own.
fn kept_line(marker: &[Kept]) -> Arc<str> {
    serde_json::json!({ "type": "marker", "strokes": marker, "max": MAX_KEPT_POINTS }).to_string().into()
}

/// Whether a relay told `sent` is where `now` is without being told again: same course, within
/// [`RELAY_DRIFT`], on a server ticking every `interval` seconds.
fn on_course(sent: &stvtf::relay::Want, now: &stvtf::relay::Want, interval: f64) -> bool {
    let same = sent.demo == now.demo
        && sent.paused == now.paused
        // One of a few set values.
        && sent.speed.to_bits() == now.speed.to_bits()
        && sent.from_start == now.from_start
        && sent.camera == now.camera;
    let gone = if sent.paused { 0.0 } else { now.at.saturating_duration_since(sent.at).as_secs_f64() * sent.speed };
    let expected = f64::from(sent.tick) + gone / interval.max(1e-3);
    same && (f64::from(now.tick) - expected).abs() * interval <= RELAY_DRIFT.as_secs_f64()
}

/// What a room is, saved for the next process: where playback is, who is in it, and the view.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Saved {
    paused: bool,
    speed: f64,
    at: u64,
    people: People,
    leader: Option<Leader>,
    view: Option<String>,
    /// Saved in a file of its own (see `Rooms::save`); here in older files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) marker: Vec<Kept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay: Option<Was>,
}

/// A page in the room, by its guest cookie if it is listed, until dropped.
pub(crate) struct Watching(Arc<Room>, Option<String>);

impl Drop for Watching {
    fn drop(&mut self) {
        if let Some(guest) = &self.1 {
            lock(&self.0.people).leave(guest);
            self.0.announce_people();
        }
        if self.0.viewers.fetch_sub(1, Ordering::Relaxed) == 1 {
            *lock(&self.0.idle_since) = Some(Instant::now());
        }
    }
}

impl Room {
    /// A room playing `recording` from the start, at 1×; a live match's from where it is now.
    pub(crate) fn start(id: String, owner: &str, recording: Arc<Recording>) -> Arc<Room> {
        let at = if recording.is_live() { recording.length() } else { 0 };
        let state = State { paused: false, speed: 1.0, at, seek: (at > 0).then_some(at), stop: false, tick: None };
        Room::run(id, People::new(owner), None, None, state, recording)
    }

    /// A room playing `recording` from `at`, at `speed` (1× unless one of [`SPEEDS`]), paused or not.
    pub(crate) fn start_at(id: String, owner: &str, recording: Arc<Recording>, at: u64, speed: f64, paused: bool) -> Arc<Room> {
        let at = at.clamp(recording.start(), recording.length());
        let speed = if SPEEDS.contains(&speed) { speed } else { 1.0 };
        let state = State { paused, speed, at, seek: Some(at), stop: false, tick: None };
        Room::run(id, People::new(owner), None, None, state, recording)
    }

    /// The room `saved` was, back where it was.
    pub(crate) fn restore(id: String, saved: Saved, recording: Arc<Recording>) -> Arc<Room> {
        let at = saved.at.clamp(recording.start(), recording.length());
        let state = State { paused: saved.paused, speed: saved.speed, at, seek: Some(at), stop: false, tick: None };
        let room = Room::run(id, saved.people, saved.leader, saved.view.map(Into::into), state, recording);
        if let Some(view) = lock(&room.view).clone() {
            announce(&room.hub, "view", &view);
        }
        *lock(&room.relay_was) = saved.relay;
        *lock(&room.marker) = saved.marker;
        announce(&room.hub, "marker", &kept_line(&lock(&room.marker)));
        room.announce_people();
        room
    }

    fn run(
        id: String,
        people: People,
        leader: Option<Leader>,
        view: Option<Arc<str>>,
        state: State,
        recording: Arc<Recording>,
    ) -> Arc<Room> {
        let room = Arc::new(Room {
            id,
            hub: SharedHub::default(),
            people: Mutex::new(people),
            leader: Mutex::new(leader),
            view: Mutex::new(view),
            recording,
            state: Mutex::new(state),
            wake: Condvar::new(),
            viewers: AtomicUsize::new(0),
            idle_since: Mutex::new(Some(Instant::now())),
            offer: OnceLock::new(),
            relay: Mutex::new(None),
            relay_said: Mutex::new(None),
            relay_sent: Mutex::new(None),
            relay_was: Mutex::new(None),
            relay_failed: Mutex::new(None),
            camera: Mutex::new(None),
            marker: Mutex::new(Vec::new()),
        });
        announce(&room.hub, "marker", &kept_line(&[]));
        let player = room.clone();
        std::thread::spawn(move || player.play());
        room
    }

    /// The room as it is now, to pick up again.
    pub(crate) fn saved(&self) -> Saved {
        let st = lock(&self.state);
        Saved {
            paused: st.paused,
            speed: st.speed,
            at: st.seek.unwrap_or(st.at),
            people: lock(&self.people).clone(),
            leader: lock(&self.leader).clone(),
            view: lock(&self.view).as_deref().map(str::to_string),
            marker: lock(&self.marker).clone(),
            relay: lock(&self.relay_was).clone(),
        }
    }

    pub(crate) fn is_admin(&self, guest: &str) -> bool {
        lock(&self.people).is_admin(guest)
    }

    /// The number pages know `guest` by, and whether they are an admin.
    pub(crate) fn me(&self, guest: &str) -> (u32, bool) {
        let mut people = lock(&self.people);
        (people.id(guest), people.is_admin(guest))
    }

    pub(crate) fn rename(&self, guest: &str, name: &str) {
        lock(&self.people).rename(guest, name);
        self.announce_people();
    }

    /// `guest` makes themselves an admin, as a streamer of the site's may.
    pub(crate) fn make_admin(&self, guest: &str) {
        lock(&self.people).make_admin(guest);
        self.announce_people();
    }

    pub(crate) fn set_ink(&self, guest: &str, ink: &str) -> Result<()> {
        lock(&self.people).set_ink(guest, ink)?;
        self.announce_people();
        Ok(())
    }

    pub(crate) fn set_admin(&self, by: &str, id: u32, admin: bool) -> Result<()> {
        lock(&self.people).set_admin(by, id, admin)?;
        self.announce_people();
        Ok(())
    }

    fn announce_people(&self) {
        let line = serde_json::json!({ "type": "people", "people": lock(&self.people).shown() });
        announce(&self.hub, "people", &line.to_string().into());
    }

    pub(crate) fn control(&self, c: &Control) -> Result<()> {
        if let Some(speed) = c.speed
            && !SPEEDS.contains(&speed)
        {
            bail!("speed {speed} is not one of {SPEEDS:?}");
        }
        let mut st = lock(&self.state);
        if let Some(speed) = c.speed {
            st.speed = speed;
        }
        if let Some(paused) = c.paused {
            st.paused = paused;
        }
        if let Some(at) = c.seek {
            st.seek = Some(at.clamp(self.recording.start(), self.recording.length()));
        }
        self.announce(&st);
        self.wake.notify_all();
        Ok(())
    }

    /// Passes a piece of a stroke by `guest` on to every page.
    pub(crate) fn draw(&self, d: &Draw, guest: &str) -> Result<()> {
        if d.points.is_empty() || d.points.len() > MAX_POINTS {
            bail!("1 to {MAX_POINTS} points");
        }
        if !d.points.iter().all(|&[x, y, t]| [x, y].iter().all(|v| v.is_finite() && v.abs() < 1e6) && (0.0..=MAX_STROKE_MS).contains(&t)) {
            bail!("points must be on a map, in a stroke's time");
        }
        let by = lock(&self.people).id(guest);
        let mut line = serde_json::to_value(d)?;
        line["type"] = "draw".into();
        line["by"] = by.into();
        let line: Arc<str> = line.to_string().into();
        if !d.keep {
            send(&self.hub, &line);
            return Ok(());
        }
        let mut marker = lock(&self.marker);
        if marker.iter().map(|k| k.points.len()).sum::<usize>() + d.points.len() > MAX_KEPT_POINTS {
            bail!("The map is full of marker ({MAX_KEPT_POINTS} points): clear it first.");
        }
        match marker.iter_mut().find(|k| k.stroke == d.stroke && k.by == by) {
            Some(k) => k.points.extend(&d.points),
            None => marker.push(Kept { stroke: d.stroke, by, points: d.points.clone() }),
        }
        send_keeping(&self.hub, &line, "marker", kept_line(&marker));
        Ok(())
    }

    /// Admin `guest` wipes every stroke, marker too.
    pub(crate) fn clear(&self, guest: &str) {
        let by = lock(&self.people).id(guest);
        let mut marker = lock(&self.marker);
        marker.clear();
        let line = serde_json::json!({ "type": "clear", "by": by }).to_string().into();
        send_keeping(&self.hub, &line, "marker", kept_line(&marker));
    }

    /// Admin `guest` sets how everyone sees the map.
    pub(crate) fn set_view(&self, post: &ViewPost, guest: &str) -> Result<()> {
        let view = &post.view;
        let on_map = |vs: &[f64], w: f64, h: f64| vs.iter().all(|v| v.is_finite() && v.abs() < 1e6) && w > 0.0 && h > 0.0;
        let fits = match *view {
            View::Admin { cx, cy, w, h, rot } => on_map(&[cx, cy, w, h, rot], w, h),
            View::Follow { w: Some(w), h: Some(h), .. } => on_map(&[w, h], w, h),
            View::Follow { w: None, h: None, .. } | View::Free | View::Auto => true,
            View::Follow { .. } => false,
        };
        if !fits {
            bail!("a view must be on a map");
        }
        if post.page.len() > MAX_PAGE {
            bail!("a page name is at most {MAX_PAGE} bytes");
        }
        let by = lock(&self.people).id(guest);
        let mut leader = lock(&self.leader);
        let kind = kind(view).to_string();
        if !post.take && leader.as_ref().is_none_or(|(_, _, now)| *now != kind) {
            return Ok(());
        }
        *leader = Some((by, post.page.clone(), kind));
        let mut line = serde_json::to_value(view)?;
        line["type"] = "view".into();
        line["by"] = by.into();
        line["page"] = post.page.clone().into();
        let line: Arc<str> = line.to_string().into();
        *lock(&self.view) = Some(line.clone());
        announce(&self.hub, "view", &line);
        drop(leader);
        let camera = match *view {
            View::Follow { player, .. } => Some(player),
            _ => None,
        };
        if std::mem::replace(&mut *lock(&self.camera), camera) != camera {
            self.follow(&lock(&self.state));
        }
        Ok(())
    }

    /// A page of `guest`'s, called `name` if it says (or `site` when they signed in there),
    /// joins; an overlay is not listed.
    pub(crate) fn join(self: &Arc<Self>, guest: &str, name: Option<&str>, site: Option<&str>, listed: bool) -> Watching {
        self.viewers.fetch_add(1, Ordering::Relaxed);
        *lock(&self.idle_since) = None;
        if listed {
            // A page, not an overlay: an overlay alone needs no TF2 address.
            self.ensure_relay();
            lock(&self.people).join(guest, name, site);
            self.announce_people();
        }
        Watching(self.clone(), listed.then(|| guest.to_string()))
    }

    /// How long nobody has watched, if nobody does (on a page, or in TF2).
    pub(crate) fn idle_for(&self) -> Option<Duration> {
        if self.relay_clients() > 0 {
            return None;
        }
        lock(&self.idle_since).map(|t| t.elapsed())
    }

    /// The room can be relayed to TF2 clients, from the demos `offer` says.
    pub(crate) fn offer_relay(&self, offer: Offer) {
        let _ = self.offer.set(offer);
        if lock(&self.people).listed() > 0 {
            self.ensure_relay();
        }
    }

    /// Starts its relay if it can have one and has none. Takes the state's lock and the relay's
    /// one at a time: the player holds the state's while it points the relay.
    fn ensure_relay(&self) {
        let Some(offer) = self.offer.get() else { return };
        if lock(&self.relay).is_some() {
            return;
        }
        if lock(&self.relay_failed).as_ref().is_some_and(|(t, _)| t.elapsed() < RELAY_RETRY) {
            return;
        }
        let Some(want) = self.want(&lock(&self.state), offer, true) else { return };
        let was = lock(&self.relay_was).clone();
        match offer.relays.start(&self.id, want.clone(), was.as_ref()) {
            Ok(Some(r)) => {
                *lock(&self.relay_failed) = None;
                let mut relay = lock(&self.relay);
                // Another page started one meanwhile: this one goes.
                if relay.is_none() {
                    if let Some(was) = r.was() {
                        *lock(&self.relay_was) = Some(was);
                    }
                    *lock(&self.relay_sent) = Some(want);
                    *relay = Some(r);
                }
            }
            Ok(None) => {}
            Err(e) => {
                let why = format!("{e:#}");
                let mut failed = lock(&self.relay_failed);
                if failed.as_ref().is_none_or(|(_, before)| *before != why) {
                    eprintln!("stvtf-web: room {}: {why}", &self.id[..8.min(self.id.len())]);
                }
                *failed = Some((Instant::now(), why));
            }
        }
        self.tell_relay();
    }

    /// Where its relay should be, as playback is in `st`: at the tick last played, or the first of
    /// the demo before any (`any`).
    fn want(&self, st: &State, offer: &Offer, any: bool) -> Option<stvtf::relay::Want> {
        let tick = st.tick.or(any.then_some(0))?;
        Some(stvtf::relay::Want {
            demo: (offer.demo_of)(tick)?,
            tick,
            at: Instant::now(),
            paused: st.paused,
            speed: st.speed,
            from_start: false,
            camera: lock(&self.camera).map(|player| stvtf::relay::Camera::Follow { player }),
        })
    }

    /// Points the relay at where playback is, when it changed course: the relay plays on by itself.
    fn follow(&self, st: &State) {
        let Some(offer) = self.offer.get() else { return };
        let interval = lock(&self.hub).tick_interval();
        let relay = lock(&self.relay);
        let Some(r) = &*relay else { return };
        let Some(want) = self.want(st, offer, false) else { return };
        let mut sent = lock(&self.relay_sent);
        if sent.as_ref().is_some_and(|s| on_course(s, &want, interval)) {
            return;
        }
        r.follow(want.clone());
        *sent = Some(want);
    }

    fn relay_clients(&self) -> usize {
        lock(&self.relay).as_ref().map_or(0, RoomRelay::clients)
    }

    /// Tells the pages how to connect in TF2, when that changed.
    fn tell_relay(&self) {
        let line = lock(&self.relay).as_ref().map_or_else(|| serde_json::json!({ "type": "relay" }), RoomRelay::line);
        let line = line.to_string();
        let mut said = lock(&self.relay_said);
        if said.as_deref() != Some(line.as_str()) {
            announce(&self.hub, "relay", &line.clone().into());
            *said = Some(line);
        }
    }

    /// Looks after its relay, as the rooms are looked after: one that stopped (its demo cannot be
    /// read, say) goes, one wanted and missing (the demo not written yet, every port taken) is
    /// tried again, and the pages hear why it cannot play as soon as that is known.
    pub(crate) fn tend_relay(&self) {
        let stopped = lock(&self.relay).as_ref().is_some_and(RoomRelay::stopped);
        if stopped && let Some(r) = lock(&self.relay).take() {
            r.end();
            *lock(&self.relay_sent) = None;
        }
        let missing = lock(&self.relay).is_none();
        if missing && lock(&self.people).listed() > 0 {
            self.ensure_relay();
        }
        self.tell_relay();
    }

    /// The timeline's marks from the `from`th on, and how many there are.
    pub(crate) fn marks(&self, from: usize) -> (Vec<crate::recording::Mark>, usize) {
        self.recording.marks(from)
    }

    pub(crate) fn owned_by(&self, guest: &str) -> bool {
        lock(&self.people).is_owner(guest)
    }

    pub(crate) fn stop(&self) {
        lock(&self.state).stop = true;
        self.wake.notify_all();
        if let Some(r) = lock(&self.relay).take() {
            r.end();
        }
    }

    fn announce(&self, st: &State) {
        let line = serde_json::json!({
            "type": "playback",
            "paused": st.paused,
            "speed": st.speed,
            "at": st.at,
            "length": self.recording.length(),
            // What is kept (from `start`), and where the map began.
            "start": self.recording.start(),
            "origin": self.recording.origin(),
            "rounds": self.recording.rounds(),
            "live": self.recording.is_live(),
            // Marks so far: a page fetches the ones it lacks.
            "marks": self.recording.marks(usize::MAX).1,
        });
        announce(&self.hub, "playback", &line.to_string().into());
        self.follow(st);
    }

    /// The player: until stopped, plays the lines in time, seeking and pausing when told. At the
    /// end of a live match's recording it waits for more; at the end of any other it pauses.
    fn play(&self) {
        let rec = &self.recording;
        // Where playback is: the chunk, how many of its lines are played, and the lines after,
        // as far as the recording had them when last looked at (`looked`: how many it had then).
        let (mut chunk, mut done, mut looked) = (0, 0, None);
        let mut lines: VecDeque<(u64, Line)> = VecDeque::new();
        let mut st = lock(&self.state);
        self.announce(&st);
        let mut announced = Instant::now();
        loop {
            if st.stop {
                return;
            }
            // A live match goes on, paused or not: pages hear how long it is now.
            if rec.is_live() && announced.elapsed() >= ANNOUNCE_EVERY {
                self.announce(&st);
                self.tell_relay();
                announced = Instant::now();
            }
            // What is played was dropped (a new map): on from what is kept.
            if st.seek.is_none() && st.at < rec.start() {
                st.seek = Some(rec.start());
            }
            if let Some(target) = st.seek.take() {
                let (hub, i, played) = rec.seek(target);
                // Where it went, paused or not: the relay follows at once.
                st.tick = hub.tick();
                restore(&self.hub, hub);
                (chunk, done, looked, st.at) = (i, played, None, target.max(rec.start()));
                lines.clear();
                self.announce(&st);
                continue;
            }
            // More of this chunk, if a live one grew; else on to the next.
            if lines.is_empty() && looked != Some(rec.count()) {
                looked = Some(rec.count());
                lines = rec.chunk(chunk).unwrap_or_default().into_iter().skip(done).collect();
            }
            while lines.is_empty() && chunk + 1 < rec.chunks() {
                (chunk, done) = (chunk + 1, 0);
                lines = rec.chunk(chunk).unwrap_or_default().into();
            }
            let Some(&(t, _)) = lines.front() else {
                if rec.is_live() {
                    // At live: as far as the match is, which comes in at 1× however fast it was played.
                    if !st.paused {
                        st.at = st.at.max(rec.length());
                    }
                    if (st.speed - 1.0).abs() > f64::EPSILON {
                        st.speed = 1.0;
                        self.announce(&st);
                    }
                    st = self.sleep(st, LIVE_POLL);
                } else {
                    if !st.paused {
                        (st.paused, st.at) = (true, rec.length());
                        self.announce(&st);
                    }
                    st = self.sleep(st, IDLE_POLL);
                }
                continue;
            };
            if st.paused || (self.viewers.load(Ordering::Relaxed) == 0 && self.relay_clients() == 0) {
                st = self.sleep(st, IDLE_POLL);
                continue;
            }
            if t > st.at {
                let (speed, started, gap) = (st.speed, Instant::now(), t - st.at);
                st = self.sleep(st, wait(gap, speed).min(ANNOUNCE_EVERY));
                // Whatever was waited counts at the speed it was waited at.
                let played = (started.elapsed().as_secs_f64() * 1000.0 * speed) as u64;
                st.at = (st.at + played).min(t);
                if announced.elapsed() >= ANNOUNCE_EVERY {
                    self.announce(&st);
                    self.tell_relay();
                    announced = Instant::now();
                }
                continue;
            }
            let (_, line) = lines.pop_front().expect("a front line");
            done += 1;
            publish(&self.hub, &line);
            if let Line::Snapshot(snap) = &line {
                st.tick = Some(snap.tick);
                self.follow(&st);
            }
        }
    }

    fn sleep<'a>(&self, st: MutexGuard<'a, State>, d: Duration) -> MutexGuard<'a, State> {
        self.wake.wait_timeout(st, d).unwrap_or_else(std::sync::PoisonError::into_inner).0
    }
}

/// What kind of view `v` is.
fn kind(v: &View) -> &'static str {
    match v {
        View::Free => "free",
        View::Auto => "auto",
        View::Follow { .. } => "follow",
        View::Admin { .. } => "admin",
    }
}

/// Real time to wait out `gap` milliseconds of play at `speed`.
fn wait(gap: u64, speed: f64) -> Duration {
    Duration::from_secs_f64(gap as f64 / 1000.0 / speed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::subscribe;
    use std::fmt::Write as _;

    /// `n` feed lines, a tick (15 ms) apart.
    fn room(n: u32) -> Arc<Room> {
        let mut lines = String::new();
        for tick in 1..=n {
            writeln!(lines, "{{\"type\":\"stalemate\",\"tick\":{tick}}}").unwrap();
        }
        Room::start("r".into(), "me", Arc::new(Recording::read(lines.as_bytes(), "test").unwrap()))
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < Duration::from_secs(5), "waited for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn wait_follows_the_speed() {
        assert_eq!(wait(1000, 2.0), Duration::from_millis(500));
        assert_eq!(wait(1000, 0.25), Duration::from_secs(4));
    }

    #[test]
    fn a_room_plays_its_recording_while_watched() {
        let r = room(10);
        std::thread::sleep(Duration::from_millis(50));
        assert!(r.hub.lock().unwrap().history.is_empty(), "nobody watches yet");
        let _w = r.join("me", None, None, true);
        until("every line", || r.hub.lock().unwrap().history.len() == 10);
        r.stop();
    }

    #[test]
    fn a_paused_room_publishes_nothing() {
        let r = room(10);
        r.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
        let _w = r.join("me", None, None, true);
        std::thread::sleep(Duration::from_millis(100));
        assert!(r.hub.lock().unwrap().history.is_empty());
        r.stop();
    }

    #[test]
    fn a_seek_reprimes_the_pages() {
        let r = room(10);
        r.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
        let page = subscribe(&r.hub, false);
        r.control(&Control { seek: Some(60), ..Control::default() }).unwrap();
        until("the seek", || page.try_iter().any(|l| l.contains(r#""type":"seek""#)));
        // Ticks 1 to 4 (0 to 45 ms) are before 60 ms.
        assert_eq!(r.hub.lock().unwrap().history.len(), 4);
        r.stop();
    }

    #[test]
    fn the_end_pauses_at_the_length() {
        let r = room(3);
        let _w = r.join("me", None, None, true);
        until("the end", || lock(&r.state).paused);
        assert_eq!(lock(&r.state).at, 30);
        r.stop();
    }

    #[test]
    fn strokes_reach_every_page_and_stay_on_the_map() {
        let r = room(1);
        let page = subscribe(&r.hub, false);
        r.draw(&Draw { stroke: 7, points: vec![[1.0, 2.0, 0.0], [3.5, -4.0, 16.0]], keep: false }, "me").unwrap();
        let line = page.try_iter().find(|l| l.contains(r#""type":"draw""#)).unwrap();
        assert_eq!(&*line, r#"{"by":1,"points":[[1.0,2.0,0.0],[3.5,-4.0,16.0]],"stroke":7,"type":"draw"}"#);
        assert!(r.draw(&Draw { stroke: 7, points: vec![], keep: false }, "me").is_err());
        assert!(r.draw(&Draw { stroke: 7, points: vec![[1e9, 0.0, 0.0]], keep: false }, "me").is_err());
        assert!(r.draw(&Draw { stroke: 7, points: vec![[0.0, 0.0, -1.0]], keep: false }, "me").is_err());
        assert!(r.draw(&Draw { stroke: 7, points: vec![[0.0, 0.0, 0.0]; 257], keep: false }, "me").is_err());
        r.stop();
    }

    #[test]
    fn marker_stays_for_pages_that_join_until_cleared() {
        let r = room(1);
        let marker = |stroke, x| Draw { stroke, points: vec![[x, 0.0, 0.0]], keep: true };
        r.draw(&marker(1, 1.0), "me").unwrap();
        r.draw(&marker(1, 2.0), "me").unwrap();
        r.draw(&Draw { stroke: 2, points: vec![[9.0, 9.0, 0.0]], keep: false }, "me").unwrap();
        let joined = |r: &Room| subscribe(&r.hub, false).try_iter().filter(|l| l.contains(r#""type":"marker""#)).collect::<Vec<_>>();
        let kept = joined(&r);
        assert_eq!(&*kept[0], r#"{"max":8000,"strokes":[{"by":1,"points":[[1.0,0.0,0.0],[2.0,0.0,0.0]],"stroke":1}],"type":"marker"}"#);
        // Picked up again after a restart.
        let saved: Saved = serde_json::from_str(&serde_json::to_string(&r.saved()).unwrap()).unwrap();
        let again = Room::restore("r2".into(), saved, r.recording.clone());
        assert_eq!(joined(&again), kept);
        let page = subscribe(&r.hub, false);
        r.clear("me");
        assert!(page.try_iter().any(|l| &*l == r#"{"by":1,"type":"clear"}"#));
        assert_eq!(&*joined(&r)[0], r#"{"max":8000,"strokes":[],"type":"marker"}"#, "nothing kept, said so");
        assert_eq!(r.saved().marker.len(), 0);
        let full = Draw { stroke: 3, points: vec![[0.0, 0.0, 0.0]; 200], keep: true };
        let mut drawn = 0;
        while r.draw(&full, "me").is_ok() {
            drawn += 200;
        }
        assert_eq!(drawn, MAX_KEPT_POINTS);
        r.stop();
        again.stop();
    }

    #[test]
    fn pages_hear_who_is_in_the_room() {
        let r = room(1);
        let page = subscribe(&r.hub, false);
        let w = r.join("me", Some("coach"), None, true);
        let _v = r.join("you", None, None, true);
        let _overlay = r.join("cast", None, None, false);
        r.set_admin("me", 2, true).unwrap();
        drop(w);
        let people: Vec<_> = page.try_iter().filter(|l| l.contains(r#""type":"people""#)).collect();
        assert_eq!(
            &*people[people.len() - 1],
            r#"{"people":[{"admin":true,"id":2,"name":"Guest 2","owner":false,"signed":false}],"type":"people"}"#
        );
        assert!(r.is_admin("you"));
        r.stop();
    }

    #[test]
    fn the_view_reaches_pages_that_join_later() {
        let r = room(1);
        r.set_view(&ViewPost { view: View::Follow { player: 4, w: None, h: None }, take: true, page: "a".into() }, "me").unwrap();
        let later: Vec<_> = subscribe(&r.hub, false).try_iter().filter(|l| l.contains(r#""type":"view""#)).collect();
        assert_eq!(&*later[0], r#"{"by":1,"mode":"follow","page":"a","player":4,"type":"view"}"#);
        let zoomed = View::Follow { player: 4, w: Some(3000.0), h: Some(2000.0) };
        r.set_view(&ViewPost { view: zoomed, take: false, page: "a".into() }, "me").unwrap();
        let later: Vec<_> = subscribe(&r.hub, false).try_iter().filter(|l| l.contains(r#""type":"view""#)).collect();
        assert_eq!(&*later[0], r#"{"by":1,"h":2000.0,"mode":"follow","page":"a","player":4,"type":"view","w":3000.0}"#);
        let half = View::Follow { player: 4, w: Some(3000.0), h: None };
        assert!(r.set_view(&ViewPost { view: half, take: true, page: String::new() }, "me").is_err());
        let flat = View::Admin { cx: 0.0, cy: 0.0, w: 0.0, h: 10.0, rot: 0.0 };
        assert!(r.set_view(&ViewPost { view: flat, take: true, page: String::new() }, "me").is_err());
        r.stop();
    }

    #[test]
    fn a_camera_moving_counts_only_while_the_view_is_of_its_kind() {
        let r = room(1);
        let post = |view, take, page: &str| ViewPost { view, take, page: page.into() };
        let mine = |x: f64| View::Admin { cx: x, cy: 0.0, w: 10.0, h: 10.0, rot: 0.0 };
        let latest = || subscribe(&r.hub, false).try_iter().find(|l| l.contains(r#""type":"view""#));
        r.set_view(&post(mine(1.0), false, "a"), "me").unwrap();
        assert!(latest().is_none(), "nobody chose a view yet");
        r.set_view(&post(mine(1.0), true, "a"), "me").unwrap();
        // Any admin's page moving it leads it from then on.
        r.set_view(&post(mine(2.0), false, "c"), "you").unwrap();
        assert!(latest().unwrap().contains(r#""page":"c""#));
        // A move sent before auto zoom was chosen does not bring the admin's view back.
        r.set_view(&post(View::Auto, true, "a"), "me").unwrap();
        r.set_view(&post(mine(3.0), false, "c"), "you").unwrap();
        assert_eq!(&*latest().unwrap(), r#"{"by":1,"mode":"auto","page":"a","type":"view"}"#);
        r.stop();
    }

    #[test]
    fn a_room_plays_on_from_one_chunk_into_the_next() {
        // 0 to 15 s of lines, 0.5 s apart: two chunks, the second from 10 s.
        let mut lines = String::new();
        for tick in (0..=1000).step_by(33) {
            writeln!(lines, "{{\"type\":\"stalemate\",\"tick\":{}}}", tick + 1).unwrap();
        }
        let rec = Arc::new(Recording::read(lines.as_bytes(), "test").unwrap());
        assert!(rec.chunks() > 1);
        let r = Room::start("r".into(), "me", rec);
        let _w = r.join("me", None, None, true);
        r.control(&Control { seek: Some(9_600), speed: Some(4.0), ..Control::default() }).unwrap();
        until("the next chunk", || lock(&r.state).at > 10_500);
        assert!(!lock(&r.state).paused, "still playing");
        r.stop();
    }

    #[test]
    fn catching_up_with_a_live_match_plays_on_at_1x() {
        let rec = Arc::new(Recording::live());
        let mut time = 1_700_000_000_000;
        for _ in 0..10 {
            let text = r#"{"type":"stalemate","tick":5}"#;
            let mut stamped: stvtf::output::Stamped = serde_json::from_str(text).unwrap();
            stamped.time = Some(time);
            rec.push(&stamped, text).unwrap();
            time += 100;
        }
        let r = Room::start("r".into(), "me", rec);
        let _w = r.join("me", None, None, true);
        r.control(&Control { seek: Some(0), speed: Some(4.0), ..Control::default() }).unwrap();
        until("live again", || lock(&r.state).at >= 900);
        until("1x", || (lock(&r.state).speed - 1.0).abs() < f64::EPSILON);
        r.stop();
    }

    #[test]
    fn a_room_behind_on_the_old_map_goes_on_to_the_new() {
        let rec = Arc::new(Recording::live());
        let push = |line: &Line, time: u64| {
            let text = serde_json::to_string(line).unwrap();
            let mut stamped: stvtf::output::Stamped = serde_json::from_str(&text).unwrap();
            stamped.time = Some(time);
            rec.push(&stamped, &text).unwrap();
        };
        let t0 = 1_700_000_000_000;
        push(&crate::testing::server("cp_one"), t0);
        let r = Room::start_at("r".into(), "me", rec.clone(), 0, 1.0, true);
        push(&crate::testing::server("cp_two"), t0 + 500);
        until("the new map's start", || lock(&r.state).at == 500);
        let _w = r.join("me", None, None, true);
        r.control(&Control { paused: Some(false), ..Control::default() }).unwrap();
        until("the new map", || r.hub.lock().unwrap().map.as_deref() == Some("cp_two"));
        r.stop();
    }

    #[test]
    fn a_relay_is_told_again_only_when_the_room_changes_course() {
        use stvtf::relay::{Camera, Want};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let sent = Want { demo: "a.dem".into(), tick: 1000, at: t0, paused: false, speed: 2.0, from_start: false, camera: None };
        // A second on at 2x, 15 ms a tick: 133 ticks on.
        let now = |tick, ms| Want { tick, at: at(ms), ..sent.clone() };
        assert!(on_course(&sent, &now(1133, 1000), 0.015));
        assert!(on_course(&sent, &now(1140, 1000), 0.015), "a snapshot off");
        assert!(!on_course(&sent, &now(1000, 1000), 0.015), "held still (a live match waiting)");
        assert!(!on_course(&sent, &now(5000, 1000), 0.015), "a seek");
        assert!(!on_course(&sent, &Want { paused: true, ..now(1133, 1000) }, 0.015));
        assert!(!on_course(&sent, &Want { camera: Some(Camera::Follow { player: 3 }), ..now(1133, 1000) }, 0.015));
        assert!(!on_course(&sent, &Want { demo: "b.dem".into(), ..now(1133, 1000) }, 0.015));
        let paused = Want { paused: true, ..sent.clone() };
        assert!(on_course(&paused, &Want { at: at(5000), ..paused.clone() }, 0.015));
    }

    /// A paused room plays no snapshots: the tick its relay follows comes from where it went.
    #[test]
    fn a_seek_while_paused_moves_what_the_relay_follows() {
        let mut lines = String::new();
        for tick in (4..=4000).step_by(4) {
            writeln!(lines, "{}", serde_json::to_string(&crate::hub::tests::snapshot(tick, None, Vec::new())).unwrap()).unwrap();
        }
        let room = Room::start("r".into(), "me", Arc::new(Recording::read(lines.as_bytes(), "test").unwrap()));
        room.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
        room.control(&Control { seek: Some(30_000), ..Control::default() }).unwrap();
        // 30 s at 15 ms a tick.
        until("the tick of the moment sought", || lock(&room.state).tick.is_some_and(|t| t.abs_diff(2000) <= 4));
        room.stop();
    }

    #[test]
    fn only_listed_speeds_pass() {
        let r = room(1);
        assert!(r.control(&Control { speed: Some(3.0), ..Control::default() }).is_err());
        r.control(&Control { speed: Some(0.25), ..Control::default() }).unwrap();
        r.stop();
    }
}
