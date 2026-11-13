//! A room: a recording played into its own hub, paused, sped up and sought by its owner, with
//! everyone in it seeing the same.

use crate::lock;
use crate::matches::hub::{SharedHub, announce};
use crate::matches::recording::Recording;
use crate::relays::pool::{Offer, RoomRelay, Was};
use crate::rooms::people::People;
use anyhow::Result;
use drawing::kept_line;
use markers::Markers;
use play::State;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
mod arc;
mod beams;
mod circle;
mod drawing;
mod gap;
mod light;
mod markers;
mod note;
mod part;
mod play;
mod relay;
mod view;

pub(crate) const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

/// Coordinates pages may send: well past any map (Source's reach 16384).
const MAP_REACH: f64 = 1e6;

/// Whether `v` is a coordinate pages may send.
fn on_map(v: f64) -> bool {
    v.is_finite() && v.abs() < MAP_REACH
}

pub(crate) struct Room {
    pub(crate) id: String,
    pub(crate) hub: SharedHub,
    people: Mutex<People>,
    /// The admin's number, page and the kind of view it chose, when one did.
    leader: Mutex<Option<Leader>>,
    /// The latest view line, to save.
    view: Mutex<Option<Arc<str>>>,
    /// The view before a player was followed, and who led it: a follow ended goes back to it.
    before_follow: Mutex<Option<(Leader, Arc<str>)>>,
    recording: Arc<Recording>,
    state: Mutex<State>,
    wake: Condvar,
    viewers: AtomicUsize,
    pub(crate) idle_since: Mutex<Option<Instant>>,
    /// Where its relay would play from, if it can have one.
    offer: OnceLock<Offer>,
    /// Where the maps' geometry is kept: the floors arcs and the relay's strokes are laid on.
    maps: OnceLock<std::path::PathBuf>,
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
    /// The permanent marker strokes, by the map they are on.
    marker: Mutex<Markers>,
    /// The map whose marker the pages and the relay were last told (the one drawings land on).
    marker_load: AtomicUsize,
    /// The marker changed since the relay was last handed it.
    marker_dirty: AtomicBool,
    /// The voice handed to its relay, whether it has one or not.
    #[cfg(test)]
    relayed: Mutex<Vec<stvtf::relay::VoiceIn>>,
}

/// Who leads the view: the admin's number, their page, and the kind of view it chose. Saved as
/// `[by, page, kind]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "(u32, String, String)", into = "(u32, String, String)")]
struct Leader {
    by: u32,
    page: String,
    kind: String,
}

impl From<(u32, String, String)> for Leader {
    fn from((by, page, kind): (u32, String, String)) -> Self {
        Leader { by, page, kind }
    }
}

impl From<Leader> for (u32, String, String) {
    fn from(l: Leader) -> Self {
        (l.by, l.page, l.kind)
    }
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
    /// Saved in a file of its own (see `Rooms::save`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) markers: Markers,
    /// The latest map's, in files from before maps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    marker: Vec<Kept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay: Option<Was>,
    /// The map of the match it is on, by number; none in files from before: the latest.
    #[serde(default, rename = "part")]
    load: Option<usize>,
    /// It goes on to the next map by itself (see `State::follows_live`).
    #[serde(default, rename = "follows")]
    follows_live: bool,
    /// Its map was over, and whether it follows live decided (see `State::over`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    decided: bool,
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
    /// A room playing `recording` from the start, at 1×; a live match's from where it is now (the
    /// map just played, while the next has none: at its end, so it goes on with the match).
    pub(crate) fn start(id: String, owner: &str, recording: Arc<Recording>) -> Arc<Room> {
        let at = if recording.is_live() { recording.length() } else { 0 };
        let load = recording.watchable().load;
        let seek = (at > 0).then_some(at);
        let state = State { paused: false, speed: 1.0, at, seek, stop: false, tick: None, load, follows_live: false, over: None };
        Room::run(id, People::new(owner), None, None, state, recording)
    }

    /// A room playing `recording` from `at`, at `speed` (1× unless one of [`SPEEDS`]), paused or
    /// not.
    pub(crate) fn start_at(id: String, owner: &str, recording: Arc<Recording>, at: u64, speed: f64, paused: bool) -> Arc<Room> {
        let at = at.clamp(recording.start(), recording.length());
        let speed = if SPEEDS.contains(&speed) { speed } else { 1.0 };
        let load = recording.watchable().load;
        let state = State { paused, speed, at, seek: Some(at), stop: false, tick: None, load, follows_live: false, over: None };
        Room::run(id, People::new(owner), None, None, state, recording)
    }

    /// The room `saved` was, back where it was.
    pub(crate) fn restore(id: String, saved: Saved, recording: Arc<Recording>) -> Arc<Room> {
        let at = saved.at.clamp(recording.start(), recording.length());
        let watchable = recording.watchable().load;
        let load = saved.load.filter(|n| *n <= recording.latest().load).unwrap_or(watchable);
        // Whether it follows live was decided on its map before: it is not again.
        let over = saved.decided.then_some(load);
        let (paused, speed, follows_live) = (saved.paused, saved.speed, saved.follows_live);
        let state = State { paused, speed, at, seek: Some(at), stop: false, tick: None, load, follows_live, over };
        let room = Room::run(id, saved.people, saved.leader, saved.view.map(Into::into), state, recording);
        if let Some(view) = lock(&room.view).clone() {
            announce(&room.hub, "view", &view);
        }
        *lock(&room.relay_was) = saved.relay;
        let mut markers = saved.markers;
        if !saved.marker.is_empty() {
            markers.entry(watchable).or_insert(saved.marker);
        }
        *lock(&room.marker) = markers;
        announce(&room.hub, "marker", &kept_line(&room.current_marker()));
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
        let load = state.load;
        let room = Arc::new(Room {
            id,
            hub: SharedHub::default(),
            people: Mutex::new(people),
            leader: Mutex::new(leader),
            view: Mutex::new(view),
            before_follow: Mutex::new(None),
            recording,
            state: Mutex::new(state),
            wake: Condvar::new(),
            viewers: AtomicUsize::new(0),
            idle_since: Mutex::new(Some(Instant::now())),
            offer: OnceLock::new(),
            maps: OnceLock::new(),
            relay: Mutex::new(None),
            relay_said: Mutex::new(None),
            relay_sent: Mutex::new(None),
            relay_was: Mutex::new(None),
            relay_failed: Mutex::new(None),
            camera: Mutex::new(None),
            marker: Mutex::new(Markers::new()),
            marker_load: AtomicUsize::new(load),
            marker_dirty: AtomicBool::new(true),
            #[cfg(test)]
            relayed: Mutex::new(Vec::new()),
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
            marker: Vec::new(),
            markers: lock(&self.marker).clone(),
            relay: lock(&self.relay_was).clone(),
            load: Some(st.load),
            follows_live: st.follows_live,
            decided: st.over == Some(st.load),
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

    /// The timeline's marks, those of its map, from the `from`th of the match on, and how many
    /// the match has.
    pub(crate) fn marks(&self, from: usize) -> (Vec<crate::matches::recording::Mark>, usize) {
        let part = self.part(&lock(&self.state));
        self.recording.marks_in(part.load, from)
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
}

#[cfg(test)]
mod follow_tests;
#[cfg(test)]
mod gap_tests;
#[cfg(test)]
mod move_tests;
#[cfg(test)]
mod part_tests;
#[cfg(test)]
mod stream_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod voice_tests;
pub(crate) use arc::ArcDraw;
pub(crate) use circle::CircleDraw;
pub(crate) use drawing::{Draw, Kept};
pub(crate) use light::LightDraw;
pub(crate) use markers::read_marker;
pub(crate) use note::NoteDraw;
pub(crate) use part::MapChoice;
pub(crate) use play::Control;
pub(crate) use view::ViewPost;
