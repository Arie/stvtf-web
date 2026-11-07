//! A room: a recording played into its own hub, paused, sped up and sought by its owner, with
//! everyone in it seeing the same.

use crate::hub::{SharedHub, announce, publish, restore, send};
use crate::lock;
use crate::recording::Recording;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub(crate) const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
/// How often pages hear where playback is while it plays.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(1);
/// How often a room nobody watches looks again.
const IDLE_POLL: Duration = Duration::from_millis(250);

pub(crate) struct Room {
    pub(crate) id: String,
    pub(crate) hub: SharedHub,
    /// The owner's guest cookie.
    owner: String,
    recording: Arc<Recording>,
    state: Mutex<State>,
    wake: Condvar,
    viewers: AtomicUsize,
    idle_since: Mutex<Option<Instant>>,
}

struct State {
    paused: bool,
    speed: f64,
    /// Play time, in milliseconds.
    at: u64,
    seek: Option<u64>,
    stop: bool,
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

/// A piece of a telestrator stroke, in map coordinates.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draw {
    pub(crate) stroke: u32,
    pub(crate) points: Vec<[f64; 2]>,
}

/// Counts a page in the room until dropped.
pub(crate) struct Watching(Arc<Room>);

impl Drop for Watching {
    fn drop(&mut self) {
        if self.0.viewers.fetch_sub(1, Ordering::Relaxed) == 1 {
            *lock(&self.0.idle_since) = Some(Instant::now());
        }
    }
}

impl Room {
    /// A room playing `recording` from the start, at 1×.
    pub(crate) fn start(id: String, owner: String, recording: Arc<Recording>) -> Arc<Room> {
        let room = Arc::new(Room {
            id,
            hub: SharedHub::default(),
            owner,
            recording,
            state: Mutex::new(State { paused: false, speed: 1.0, at: 0, seek: None, stop: false }),
            wake: Condvar::new(),
            viewers: AtomicUsize::new(0),
            idle_since: Mutex::new(Some(Instant::now())),
        });
        let player = room.clone();
        std::thread::spawn(move || player.play());
        room
    }

    pub(crate) fn is_owner(&self, guest: &str) -> bool {
        !guest.is_empty() && guest == self.owner
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
            st.seek = Some(at.min(self.recording.length));
        }
        self.announce(&st);
        self.wake.notify_all();
        Ok(())
    }

    /// Passes a piece of a stroke on to every page.
    pub(crate) fn draw(&self, d: &Draw) -> Result<()> {
        if d.points.is_empty() || d.points.len() > MAX_POINTS {
            bail!("1 to {MAX_POINTS} points");
        }
        if !d.points.iter().flatten().all(|v| v.is_finite() && v.abs() < 1e6) {
            bail!("points must be on a map");
        }
        let mut line = serde_json::to_value(d)?;
        line["type"] = "draw".into();
        send(&self.hub, &line.to_string().into());
        Ok(())
    }

    pub(crate) fn watch(self: &Arc<Self>) -> Watching {
        self.viewers.fetch_add(1, Ordering::Relaxed);
        *lock(&self.idle_since) = None;
        Watching(self.clone())
    }

    /// How long nobody has watched, if nobody does.
    pub(crate) fn idle_for(&self) -> Option<Duration> {
        lock(&self.idle_since).map(|t| t.elapsed())
    }

    pub(crate) fn stop(&self) {
        lock(&self.state).stop = true;
        self.wake.notify_all();
    }

    fn announce(&self, st: &State) {
        let line = serde_json::json!({
            "type": "playback",
            "paused": st.paused,
            "speed": st.speed,
            "at": st.at,
            "length": self.recording.length,
            "rounds": self.recording.rounds,
        });
        announce(&self.hub, &line.to_string().into());
    }

    /// The player: until stopped, plays the lines in time, seeking and pausing when told.
    fn play(&self) {
        let rec = &self.recording;
        let (mut chunk, mut lines) = (0, VecDeque::from(rec.chunk(0).unwrap_or_default()));
        let mut st = lock(&self.state);
        self.announce(&st);
        let mut announced = Instant::now();
        loop {
            if st.stop {
                return;
            }
            if let Some(target) = st.seek.take() {
                let (hub, rest, i) = rec.seek(target);
                restore(&self.hub, hub);
                (chunk, lines, st.at) = (i, rest.into(), target);
                self.announce(&st);
                continue;
            }
            while lines.is_empty() && chunk + 1 < rec.chunks() {
                chunk += 1;
                lines = rec.chunk(chunk).unwrap_or_default().into();
            }
            let Some(&(t, _)) = lines.front() else {
                if !st.paused {
                    (st.paused, st.at) = (true, rec.length);
                    self.announce(&st);
                }
                st = self.sleep(st, IDLE_POLL);
                continue;
            };
            if st.paused || self.viewers.load(Ordering::Relaxed) == 0 {
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
                    announced = Instant::now();
                }
                continue;
            }
            let (_, line) = lines.pop_front().expect("a front line");
            publish(&self.hub, &line);
        }
    }

    fn sleep<'a>(&self, st: MutexGuard<'a, State>, d: Duration) -> MutexGuard<'a, State> {
        self.wake.wait_timeout(st, d).unwrap_or_else(std::sync::PoisonError::into_inner).0
    }
}

/// The rooms open now.
#[derive(Default)]
pub(crate) struct Rooms {
    map: Mutex<HashMap<String, Arc<Room>>>,
}

impl Rooms {
    pub(crate) fn open(&self, recording: Arc<Recording>, owner: &str) -> Arc<Room> {
        let room = Room::start(crate::sessions::random_id(), owner.to_string(), recording);
        lock(&self.map).insert(room.id.clone(), room.clone());
        room
    }

    pub(crate) fn get(&self, id: &str) -> Option<Arc<Room>> {
        lock(&self.map).get(id).cloned()
    }

    pub(crate) fn count(&self) -> usize {
        lock(&self.map).len()
    }

    /// Ends the rooms nobody has watched for `linger`.
    pub(crate) fn reap(&self, linger: Duration) {
        lock(&self.map).retain(|id, room| {
            let keep = room.idle_for().is_none_or(|t| t < linger);
            if !keep {
                room.stop();
                eprintln!("stvtf-web: room {} ended", &id[..8]);
            }
            keep
        });
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
        Room::start("r".into(), "me".into(), Arc::new(Recording::read(lines.as_bytes(), "test").unwrap()))
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
        let _w = r.watch();
        until("every line", || r.hub.lock().unwrap().history.len() == 10);
        r.stop();
    }

    #[test]
    fn a_paused_room_publishes_nothing() {
        let r = room(10);
        r.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
        let _w = r.watch();
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
        let _w = r.watch();
        until("the end", || lock(&r.state).paused);
        assert_eq!(lock(&r.state).at, 30);
        r.stop();
    }

    #[test]
    fn an_unwatched_room_ends() {
        let rooms = Rooms::default();
        let r = rooms.open(room(1).recording.clone(), "me");
        let w = r.watch();
        rooms.reap(Duration::ZERO);
        assert_eq!(rooms.count(), 1, "watched");
        drop(w);
        rooms.reap(Duration::ZERO);
        assert_eq!(rooms.count(), 0);
        assert!(lock(&r.state).stop);
    }

    #[test]
    fn strokes_reach_every_page_and_stay_on_the_map() {
        let r = room(1);
        let page = subscribe(&r.hub, false);
        r.draw(&Draw { stroke: 7, points: vec![[1.0, 2.0], [3.5, -4.0]] }).unwrap();
        let line = page.try_iter().find(|l| l.contains(r#""type":"draw""#)).unwrap();
        assert_eq!(&*line, r#"{"points":[[1.0,2.0],[3.5,-4.0]],"stroke":7,"type":"draw"}"#);
        assert!(r.draw(&Draw { stroke: 7, points: vec![] }).is_err());
        assert!(r.draw(&Draw { stroke: 7, points: vec![[1e9, 0.0]] }).is_err());
        assert!(r.draw(&Draw { stroke: 7, points: vec![[0.0, 0.0]; 257] }).is_err());
        r.stop();
    }

    #[test]
    fn only_listed_speeds_pass() {
        let r = room(1);
        assert!(r.control(&Control { speed: Some(3.0), ..Control::default() }).is_err());
        r.control(&Control { speed: Some(0.25), ..Control::default() }).unwrap();
        r.stop();
    }
}
