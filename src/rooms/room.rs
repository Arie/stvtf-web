//! A room: a recording played into its own hub, paused, sped up and sought by its owner, with
//! everyone in it seeing the same.

use crate::lock;
use crate::matches::hub::{SharedHub, announce};
use crate::matches::recording::Recording;
use crate::relays::pool::{Offer, RoomRelay, Was};
use crate::rooms::people::People;
use anyhow::Result;
use drawing::kept_line;
use play::State;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
mod beams;
mod drawing;
mod play;
mod relay;
mod view;

pub(crate) const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

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
    pub(crate) idle_since: Mutex<Option<Instant>>,
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
    /// The strokes being laid on the floors for the relay, by person and stroke, oldest first.
    drapes: Mutex<Vec<beams::PenStroke>>,
    /// The marker changed since the relay was last handed it.
    marker_dirty: AtomicBool,
}

type Leader = (u32, String, String);

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
            drapes: Mutex::new(Vec::new()),
            marker_dirty: AtomicBool::new(true),
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

    /// The timeline's marks from the `from`th on, and how many there are.
    pub(crate) fn marks(&self, from: usize) -> (Vec<crate::matches::recording::Mark>, usize) {
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
}

#[cfg(test)]
mod tests;
pub(crate) use drawing::{Draw, Kept};
pub(crate) use play::Control;
pub(crate) use view::ViewPost;
