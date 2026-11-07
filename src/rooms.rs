//! The rooms open now. With a store (`--sessions`), each is saved as it goes, so the process that
//! replaces this one (a deploy) picks it up when its pages reconnect: a pause of a second or two.

use crate::lock;
use crate::recording::Recording;
use crate::room::{Room, Saved};
use crate::sessions::{Session, Sessions, Store, Viewer};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

/// A saved room nobody came back for is dropped after this long.
const STALE: Duration = Duration::from_hours(1);

#[derive(Default)]
pub(crate) struct Rooms {
    map: Mutex<HashMap<String, Arc<Room>>>,
    /// Where they are saved, if anywhere.
    store: Option<Store>,
    /// What each plays.
    sources: Mutex<HashMap<String, Source>>,
    /// Demos' recordings rooms picked up play, while one does: rooms on the same share it.
    shared: Mutex<HashMap<String, Weak<Recording>>>,
    /// Live rooms keep their match watched.
    watching: Mutex<HashMap<String, Viewer>>,
    /// Private playbacks: one viewer's own, from a match page; not saved.
    private: Mutex<std::collections::HashSet<String>>,
}

/// Private playbacks at once, at most.
pub(crate) const MAX_PRIVATE: usize = 200;

/// What a room plays: a fixed match's recording, there again in the next process; a demos.tf
/// demo's output, saved beside the rooms on it since its match is not; or a live match's
/// recording, which the match keeps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Source {
    Pinned(String),
    Demo(u64),
    Live(String),
}

#[derive(Serialize, Deserialize)]
struct SavedRoom {
    source: Source,
    room: Saved,
}

/// A room's file in the store, and a demo's beside the rooms on it.
fn file(id: &str) -> String {
    format!("room-{id}")
}
fn demo_file(demo: u64) -> String {
    format!("demo-{demo}.gz")
}

impl Rooms {
    pub(crate) fn new(store: Option<Store>) -> Rooms {
        Rooms { store, ..Rooms::default() }
    }

    /// A room on match `from`, owned by `owner`; one on a live match starts at live.
    pub(crate) fn open(&self, from: &Arc<Session>, recording: Arc<Recording>, owner: &str) -> Result<Arc<Room>> {
        let id = crate::sessions::random_id();
        let source = match from.demo_id() {
            Some(demo) => Source::Demo(demo),
            None if from.is_pinned() => Source::Pinned(from.id.clone()),
            None => Source::Live(from.id.clone()),
        };
        if let (Source::Demo(demo), Some(store)) = (&source, &self.store)
            && !store.has(&demo_file(*demo))
        {
            let Some(output) = from.output() else { bail!("the demo is still being fetched") };
            store.put(&demo_file(*demo), &output)?;
        }
        let room = Room::start(id, owner, recording);
        self.watch(&room.id, from, &source);
        self.add(&room, source);
        Ok(room)
    }

    /// A private playback of match `from` for `owner`, from `at`: theirs alone, not saved.
    pub(crate) fn open_private(
        &self,
        from: &Arc<Session>,
        recording: Arc<Recording>,
        owner: &str,
        at: u64,
        speed: f64,
        paused: bool,
    ) -> Result<Arc<Room>> {
        if lock(&self.private).len() >= MAX_PRIVATE {
            bail!("too many watching on their own");
        }
        let room = Room::start_at(crate::sessions::random_id(), owner, recording, at, speed, paused);
        if from.demo_id().is_none() && !from.is_pinned() {
            self.watch(&room.id, from, &Source::Live(from.id.clone()));
        }
        lock(&self.private).insert(room.id.clone());
        lock(&self.map).insert(room.id.clone(), room.clone());
        Ok(room)
    }

    /// Rooms that count towards the match limit: not private playbacks.
    pub(crate) fn shared_count(&self) -> usize {
        self.count() - lock(&self.private).len()
    }

    /// A room on a live match keeps it watched.
    fn watch(&self, room: &str, from: &Arc<Session>, source: &Source) {
        if matches!(source, Source::Live(_)) {
            lock(&self.watching).insert(room.to_string(), Viewer::new(from.clone()));
        }
    }

    /// The recording rooms on `key` play, or `make`'s.
    fn shared(&self, key: &str, make: impl FnOnce() -> Option<Arc<Recording>>) -> Option<Arc<Recording>> {
        let mut shared = lock(&self.shared);
        shared.retain(|_, r| r.strong_count() > 0);
        if let Some(r) = shared.get(key).and_then(Weak::upgrade) {
            return Some(r);
        }
        let r = make()?;
        shared.insert(key.to_string(), Arc::downgrade(&r));
        Some(r)
    }

    fn add(&self, room: &Arc<Room>, source: Source) {
        lock(&self.sources).insert(room.id.clone(), source);
        lock(&self.map).insert(room.id.clone(), room.clone());
        self.save(room);
    }

    /// Room `id`: open here, or saved by the process this one replaced and picked up now.
    pub(crate) fn get(&self, id: &str, sessions: &Sessions) -> Option<Arc<Room>> {
        if let Some(room) = lock(&self.map).get(id) {
            return Some(room.clone());
        }
        let store = self.store.as_ref()?;
        if !crate::sessions::is_id(id) {
            return None;
        }
        let saved: SavedRoom = serde_json::from_slice(&store.take(&file(id)).ok()?).ok()?;
        let recording = match &saved.source {
            Source::Pinned(match_id) | Source::Live(match_id) => sessions.get(match_id)?.recording().ok()?,
            Source::Demo(demo) => self.shared(&format!("demo:{demo}"), || {
                let gz = store.take(&demo_file(*demo)).ok()?;
                Some(Arc::new(Recording::read(flate2::read::GzDecoder::new(gz.as_slice()), &format!("demo {demo}")).ok()?))
            })?,
        };
        let mut map = lock(&self.map);
        // Another page of it may have picked it up meanwhile.
        if let Some(room) = map.get(id) {
            return Some(room.clone());
        }
        let room = Room::restore(id.to_string(), saved.room, recording);
        if let Source::Live(match_id) = &saved.source
            && let Some(m) = sessions.get(match_id)
        {
            self.watch(id, &m, &saved.source);
        }
        lock(&self.sources).insert(id.to_string(), saved.source);
        map.insert(id.to_string(), room.clone());
        eprintln!("stvtf-web: room {} picked up", &id[..8]);
        Some(room)
    }

    pub(crate) fn count(&self) -> usize {
        lock(&self.map).len()
    }

    fn save(&self, room: &Room) {
        let Some(store) = &self.store else { return };
        let Some(source) = lock(&self.sources).get(&room.id).cloned() else { return };
        if let Source::Demo(demo) = source {
            store.touch(&demo_file(demo));
        }
        let saved = SavedRoom { source, room: room.saved() };
        if let Err(e) = serde_json::to_vec(&saved).map_err(Into::into).and_then(|plain| store.put(&file(&room.id), &plain)) {
            eprintln!("stvtf-web: saving room {}: {e:#}", &room.id[..8]);
        }
    }

    /// Saves every room as it is now.
    pub(crate) fn persist(&self) {
        let rooms: Vec<Arc<Room>> = lock(&self.map).values().cloned().collect();
        for room in rooms {
            self.save(&room);
        }
    }

    /// Ends the rooms nobody has watched for `linger`, and forgets them.
    pub(crate) fn reap(&self, linger: Duration) {
        let mut ended = Vec::new();
        lock(&self.map).retain(|id, room| {
            let keep = room.idle_for().is_none_or(|t| t < linger);
            if !keep {
                room.stop();
                ended.push(id.clone());
            }
            keep
        });
        for id in ended {
            let mut sources = lock(&self.sources);
            let source = sources.remove(&id);
            lock(&self.watching).remove(&id);
            lock(&self.private).remove(&id);
            if let Some(store) = &self.store {
                store.forget(&file(&id));
                // A demo's file goes with the last room on it.
                if let Some(Source::Demo(demo)) = source
                    && !sources.values().any(|s| *s == Source::Demo(demo))
                {
                    store.forget(&demo_file(demo));
                }
            }
            eprintln!("stvtf-web: room {} ended", &id[..8]);
        }
    }

    /// Drops saved rooms nobody came back for, and demos no room plays.
    pub(crate) fn sweep(&self) {
        let Some(store) = &self.store else { return };
        let Ok(entries) = std::fs::read_dir(&store.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let old =
                entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() > STALE);
            if (name.starts_with("room-") || name.starts_with("demo-") || name.starts_with("live-")) && old {
                store.forget(&name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use std::fmt::Write as _;

    /// Sessions with a pinned demo rooms can play, and the folder its recording is in.
    fn demo() -> (Sessions, TempDir) {
        let dir = TempDir::new("rooms");
        let path = dir.join("demo.jsonl");
        let mut lines = String::new();
        for t in 1..=100 {
            writeln!(lines, "{{\"type\":\"stalemate\",\"tick\":{t}}}").unwrap();
        }
        std::fs::write(&path, lines).unwrap();
        let sessions = Sessions::new(false, 2, 20.0);
        sessions.pin("demo", move |s| s.set_file(path));
        while !sessions.get("demo").unwrap().recordable() {
            std::thread::sleep(Duration::from_millis(1));
        }
        (sessions, dir)
    }

    fn store(dir: &TempDir) -> Store {
        Store { dir: dir.join("store"), keys: None }
    }

    #[test]
    fn an_unwatched_room_ends_and_is_forgotten() {
        let (sessions, dir) = demo();
        let rooms = Rooms::new(Some(store(&dir)));
        let m = sessions.get("demo").unwrap();
        let r = rooms.open(&m, m.recording().unwrap(), "me").unwrap();
        let w = r.join("me", None, None, true);
        rooms.reap(Duration::ZERO);
        assert_eq!(rooms.count(), 1, "watched");
        drop(w);
        rooms.reap(Duration::ZERO);
        assert_eq!(rooms.count(), 0);
        assert!(!dir.join("store").join(file(&r.id)).exists());
        r.stop();
    }

    #[test]
    fn the_next_process_picks_a_room_up_where_it_was() {
        let (sessions, dir) = demo();
        let rooms = Rooms::new(Some(store(&dir)));
        let m = sessions.get("demo").unwrap();
        let r = rooms.open(&m, m.recording().unwrap(), "me").unwrap();
        let _w = r.join("me", Some("coach"), None, true);
        r.control(&crate::room::Control { paused: Some(true), speed: Some(2.0), seek: Some(600) }).unwrap();
        rooms.persist();
        r.stop();

        let next = Rooms::new(Some(store(&dir)));
        let again = next.get(&r.id, &sessions).expect("picked up");
        let saved = again.saved();
        assert_eq!((saved_at(&saved), again.me("me")), (600, (1, true)));
        assert!(serde_json::to_string(&saved).unwrap().contains(r#""name":"coach""#));
        assert!(next.get(&"0".repeat(32), &sessions).is_none());
        again.stop();
    }

    #[test]
    fn a_live_match_is_recorded_from_the_start_and_a_room_on_it_starts_at_live() {
        let sessions = Sessions::new(false, 4, 20.0);
        let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
        let recording = m.recording().unwrap();
        // A while of the match (play time goes by the wall clock).
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(30));
            crate::hub::publish(&m.hub, &serde_json::from_str(r#"{"type":"stalemate","tick":5}"#).unwrap());
        }
        while recording.length() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let rooms = Rooms::default();
        let room = rooms.open(&m, m.recording().unwrap(), "me").unwrap();
        assert!(saved_at(&room.saved()) > 0, "starts at live");
        assert!(m.watched(), "the room keeps the match watched");
        rooms.reap(Duration::ZERO);
        assert!(!m.watched());
        room.stop();
        sessions.stop_all();
    }

    #[test]
    fn a_demo_s_file_is_shared_by_its_rooms_and_goes_with_the_last() {
        let (sessions, dir) = demo();
        let store = store(&dir);
        store.put(&demo_file(7), b"gz").unwrap();
        let rooms = Rooms::new(Some(store.clone()));
        let recording = sessions.get("demo").unwrap().recording().unwrap();
        let (a, b) = (Room::start("a".repeat(32), "me", recording.clone()), Room::start("b".repeat(32), "me", recording));
        rooms.add(&a, Source::Demo(7));
        rooms.add(&b, Source::Demo(7));
        let wa = a.join("me", None, None, true);
        let wb = b.join("me", None, None, true);
        drop(wa);
        rooms.reap(Duration::ZERO);
        assert!(store.has(&demo_file(7)), "b still plays it");
        drop(wb);
        rooms.reap(Duration::ZERO);
        assert!(!store.has(&demo_file(7)));
    }

    fn saved_at(s: &Saved) -> u64 {
        serde_json::to_value(s).unwrap()["at"].as_u64().unwrap()
    }
}
