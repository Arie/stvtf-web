//! The rooms open now. With a store (`--sessions`), each is saved as it goes, so the process that
//! replaces this one (a deploy) picks it up when its pages reconnect: a pause of a second or two.

use crate::lock;
use crate::matches::recording::Recording;
use crate::matches::sessions::{Session, Sessions, Store, Viewer, demo_dem};
use crate::relays::pool::{DemoOf, Offer, Pool};
use crate::rooms::logged;
use crate::rooms::room::{Room, Saved};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

/// A saved room nobody came back for is dropped after this long.
const STALE: Duration = Duration::from_hours(1);
/// A room file not saved or touched for this long has no process playing it (they do every second).
const QUIET: Duration = Duration::from_secs(5);
/// A live match's `.dem` file not written for this long is past what its recording keeps.
const LIVE_DEMO_KEPT: Duration = Duration::from_hours(3);

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
    /// The ports rooms are relayed on, if they are (`--relay-ports`).
    relays: Option<Arc<Pool>>,
    /// Where the maps' geometry is kept, for the relays to draw strokes on their floors.
    maps: Option<PathBuf>,
    /// A hash of each room file as last written here: one the same again is only touched.
    written: Mutex<HashMap<String, u64>>,
    /// Who opened each room, for the limits per person, and when.
    by: Mutex<HashMap<String, (String, Instant)>>,
    /// Quiet room files found to have no relay.
    unrelayed: Mutex<std::collections::HashSet<String>>,
}

/// Private playbacks at once, at most.
pub(crate) const MAX_PRIVATE: usize = 200;
/// Rooms and private playbacks one person may have opened at once.
pub(crate) const ROOMS_PER_PERSON: usize = 2;
pub(crate) const PRIVATE_PER_PERSON: usize = 3;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    by: Option<String>,
}

/// A room's file in the store, its marker's, and a demo's beside the rooms on it.
fn file(id: &str) -> String {
    format!("room-{id}")
}
fn marker_file(id: &str) -> String {
    format!("room-{id}-marker")
}
fn demo_file(demo: u64) -> String {
    format!("demo-{demo}.gz")
}

impl Rooms {
    pub(crate) fn new(store: Option<Store>, relays: Option<Arc<Pool>>) -> Rooms {
        Rooms { store, relays, ..Rooms::default() }
    }

    /// Their relays draw strokes on the floors of the maps kept in `cache`.
    pub(crate) fn with_maps(mut self, cache: PathBuf) -> Rooms {
        self.maps = Some(cache);
        self
    }

    /// Tells `room` where the maps' geometry is, for the floors it draws on.
    fn give_maps(&self, room: &Room) {
        if let Some(maps) = &self.maps {
            room.set_maps(maps.clone());
        }
    }

    /// Lets `room` be relayed to TF2 clients, from where `demo_of` says its demos are.
    fn offer(&self, room: &Room, demo_of: Option<DemoOf>) {
        if let (Some(relays), Some(demo_of)) = (&self.relays, demo_of) {
            room.offer_relay(Offer { relays: relays.clone(), demo_of });
        }
    }

    /// Where the demos of what `source` plays are.
    fn demo_of(&self, source: &Source, sessions: &Sessions) -> Option<DemoOf> {
        match source {
            Source::Pinned(id) | Source::Live(id) => sessions.get(id)?.demo_of(self.store.as_ref()),
            Source::Demo(demo) => {
                let path = self.store.as_ref()?.dir.join(demo_dem(*demo));
                Some(Arc::new(move |_| path.exists().then(|| path.clone())))
            }
        }
    }

    /// The room `owner` opened on match `from` and still has, if any (not a private playback).
    pub(crate) fn owned(&self, from: &Session, owner: &str) -> Option<Arc<Room>> {
        let source = Self::source(from);
        // One lock at a time: others take the map before the rest.
        let rooms: Vec<Arc<Room>> = lock(&self.map).values().cloned().collect();
        let private = lock(&self.private).clone();
        let sources = lock(&self.sources).clone();
        rooms.into_iter().find(|r| sources.get(&r.id) == Some(&source) && !private.contains(&r.id) && r.owned_by(owner))
    }

    /// What a room on match `from` plays.
    fn source(from: &Session) -> Source {
        match from.demo_id() {
            Some(demo) => Source::Demo(demo),
            None if from.is_pinned() => Source::Pinned(from.id.clone()),
            None => Source::Live(from.id.clone()),
        }
    }

    /// A room on match `from`, owned by `owner` and counting against `by` (none: a streamer); one
    /// on a live match starts at live.
    pub(crate) fn open(&self, from: &Arc<Session>, recording: Arc<Recording>, owner: &str, by: Option<&str>) -> Result<Arc<Room>> {
        self.make_room_for(by, false)?;
        let id = crate::matches::sessions::random_id();
        let source = Self::source(from);
        if let (Source::Demo(demo), Some(store)) = (&source, &self.store)
            && !store.has(&demo_file(*demo))
        {
            let Some(output) = from.output() else { bail!("the demo is still being fetched") };
            store.put(&demo_file(*demo), &output)?;
        }
        let room = Room::start(id, owner, recording);
        self.give_maps(&room);
        self.opened_by(&room.id, by);
        self.watch(&room.id, from, &source);
        self.offer(&room, from.demo_of(self.store.as_ref()));
        self.add(&room, source);
        Ok(room)
    }

    fn opened_by(&self, room: &str, by: Option<&str>) {
        if let Some(by) = by {
            lock(&self.by).insert(room.to_string(), (by.to_string(), Instant::now()));
        }
    }

    /// Whether `by` may open another room (or private playback): fewer than the limit of theirs,
    /// or their oldest goes. A room only once nobody has watched it for [`crate::matches::sessions::MAKES_ROOM`].
    fn make_room_for(&self, by: Option<&str>, private: bool) -> Result<()> {
        let Some(by) = by else { return Ok(()) };
        let privates = lock(&self.private).clone();
        let mut theirs: Vec<(Instant, String)> = lock(&self.by)
            .iter()
            .filter(|(id, (b, _))| b == by && privates.contains(*id) == private)
            .map(|(id, (_, t))| (*t, id.clone()))
            .collect();
        let max = if private { PRIVATE_PER_PERSON } else { ROOMS_PER_PERSON };
        if theirs.len() < max {
            return Ok(());
        }
        theirs.sort();
        let map = lock(&self.map).clone();
        let oldest = theirs
            .into_iter()
            .map(|(_, id)| id)
            .find(|id| private || map.get(id).is_none_or(|r| r.idle_for().is_some_and(|t| t >= crate::matches::sessions::MAKES_ROOM)));
        let Some(id) = oldest else { bail!("You have {max} rooms open already: close one first.") };
        if let Some(room) = lock(&self.map).remove(&id) {
            room.stop();
        }
        self.forget_room(&id);
        Ok(())
    }

    /// A private playback of match `from` for `owner`, from `at`: theirs alone, not saved. It
    /// counts against `by`, as for [`Rooms::open`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_private(
        &self,
        from: &Arc<Session>,
        recording: Arc<Recording>,
        owner: &str,
        at: u64,
        speed: f64,
        paused: bool,
        by: Option<&str>,
    ) -> Result<Arc<Room>> {
        self.make_room_for(by, true)?;
        if lock(&self.private).len() >= MAX_PRIVATE {
            bail!("too many watching on their own");
        }
        let room = Room::start_at(crate::matches::sessions::random_id(), owner, recording, at, speed, paused);
        self.give_maps(&room);
        self.opened_by(&room.id, by);
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
        if !crate::matches::sessions::is_id(id) {
            return None;
        }
        let mut saved: SavedRoom = serde_json::from_slice(&store.take(&file(id)).ok()?).ok()?;
        if let Some(marker) = store.take(&marker_file(id)).ok().and_then(|m| serde_json::from_slice(&m).ok()) {
            saved.room.marker = marker;
        }
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
        self.give_maps(&room);
        self.offer(&room, self.demo_of(&saved.source, sessions));
        if let Source::Live(match_id) = &saved.source
            && let Some(m) = sessions.get(match_id)
        {
            self.watch(id, &m, &saved.source);
        }
        lock(&self.sources).insert(id.to_string(), saved.source);
        self.opened_by(id, saved.by.as_deref());
        map.insert(id.to_string(), room.clone());
        eprintln!("stvtf-web: room {} picked up", logged(id));
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
            store.touch(&demo_dem(demo));
        }
        let by = lock(&self.by).get(&room.id).map(|(b, _)| b.clone());
        let mut saved = SavedRoom { source, room: room.saved(), by };
        // Apart: not rewritten with where playback is every second.
        let marker = std::mem::take(&mut saved.room.marker);
        let written = serde_json::to_vec(&saved)
            .map_err(Into::into)
            .and_then(|plain| self.put_changed(store, &file(&room.id), &plain))
            .and_then(|()| self.put_changed(store, &marker_file(&room.id), &serde_json::to_vec(&marker)?));
        if let Err(e) = written {
            eprintln!("stvtf-web: saving room {}: {e:#}", logged(&room.id));
        }
    }

    /// Writes `plain` as `name`, or only touches it (for the sweep) if it holds that already.
    fn put_changed(&self, store: &Store, name: &str, plain: &[u8]) -> Result<()> {
        use std::hash::{BuildHasher, BuildHasherDefault};
        let hash = BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().hash_one(plain);
        if lock(&self.written).get(name) == Some(&hash) && store.has(name) {
            store.touch(name);
            return Ok(());
        }
        store.put(name, plain)?;
        lock(&self.written).insert(name.to_string(), hash);
        Ok(())
    }

    /// Picks up the saved rooms with a relay that no process plays any more: the one a deploy
    /// replaced kept them till it went, and TF2 clients may be in them with no page to ask.
    pub(crate) fn pick_up_relayed(&self, sessions: &Sessions) {
        let (Some(store), Some(_)) = (&self.store, &self.relays) else { return };
        let Ok(entries) = std::fs::read_dir(&store.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_prefix("room-").filter(|id| crate::matches::sessions::is_id(id)) else { continue };
            let age = entry.metadata().and_then(|m| m.modified()).map(|t| SystemTime::now().duration_since(t).unwrap_or_default());
            if !age.is_ok_and(|a| a > QUIET && a < STALE) || lock(&self.map).contains_key(id) || lock(&self.unrelayed).contains(id) {
                continue;
            }
            let relayed =
                store.take(&name).ok().and_then(|d| serde_json::from_slice::<SavedRoom>(&d).ok()).is_some_and(|s| Room::relayed(&s.room));
            if relayed {
                self.get(id, sessions);
            } else {
                lock(&self.unrelayed).insert(id.to_string());
            }
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
        let rooms: Vec<Arc<Room>> = lock(&self.map).values().cloned().collect();
        for room in rooms {
            room.tend_relay();
        }
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
            self.forget_room(&id);
        }
    }

    /// Forgets room `id`, ended, and its files.
    fn forget_room(&self, id: &str) {
        let mut sources = lock(&self.sources);
        let source = sources.remove(id);
        lock(&self.watching).remove(id);
        lock(&self.private).remove(id);
        lock(&self.by).remove(id);
        lock(&self.written).retain(|name, _| !name.starts_with(&file(id)));
        if let Some(store) = &self.store {
            store.forget(&file(id));
            store.forget(&marker_file(id));
            // A demo's file goes with the last room on it.
            if let Some(Source::Demo(demo)) = source
                && !sources.values().any(|s| *s == Source::Demo(demo))
            {
                store.forget(&demo_file(demo));
            }
        }
        eprintln!("stvtf-web: room {} ended", logged(id));
    }

    /// Drops saved rooms nobody came back for, demos no room plays, and the files of live matches
    /// gone: those not written or used for an hour. A live match's `.dem` files go after the
    /// recording's three hours (one on a long map is written to all along; an earlier one, from
    /// before a reconnect, still holds part of what the recording keeps). Run at start and from
    /// time to time.
    pub(crate) fn sweep(&self) {
        let Some(store) = &self.store else { return };
        let Ok(entries) = std::fs::read_dir(&store.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let age = entry.metadata().and_then(|m| m.modified()).map(|t| SystemTime::now().duration_since(t).unwrap_or_default());
            let dem = std::path::Path::new(&name).extension().is_some_and(|e| e == "dem");
            let keep = if name.starts_with("live-") && dem { LIVE_DEMO_KEPT } else { STALE };
            let ours = name.starts_with("room-") || name.starts_with("demo-") || name.starts_with("live-");
            if ours && age.is_ok_and(|a| a > keep) {
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
        let rooms = Rooms::new(Some(store(&dir)), None);
        let m = sessions.get("demo").unwrap();
        let r = rooms.open(&m, m.recording().unwrap(), "me", None).unwrap();
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
        let rooms = Rooms::new(Some(store(&dir)), None);
        let m = sessions.get("demo").unwrap();
        let r = rooms.open(&m, m.recording().unwrap(), "me", None).unwrap();
        let _w = r.join("me", Some("coach"), None, true);
        r.control(&crate::rooms::room::Control { paused: Some(true), speed: Some(2.0), seek: Some(600) }).unwrap();
        rooms.persist();
        r.stop();

        let next = Rooms::new(Some(store(&dir)), None);
        let again = next.get(&r.id, &sessions).expect("picked up");
        let saved = again.saved();
        assert_eq!((saved_at(&saved), again.me("me")), (600, (1, true)));
        assert!(serde_json::to_string(&saved).unwrap().contains(r#""name":"coach""#));
        assert!(next.get(&"0".repeat(32), &sessions).is_none());
        again.stop();
    }

    #[test]
    fn a_room_s_files_are_written_only_as_they_change_and_its_marker_apart() {
        use std::os::unix::fs::MetadataExt;
        let (sessions, dir) = demo();
        let rooms = Rooms::new(Some(store(&dir)), None);
        let m = sessions.get("demo").unwrap();
        let r = rooms.open(&m, m.recording().unwrap(), "me", None).unwrap();
        r.control(&crate::rooms::room::Control { paused: Some(true), ..Default::default() }).unwrap();
        r.draw(&crate::rooms::room::Draw { stroke: 1, points: vec![[5.0, 5.0, 0.0]; 100], keep: true, end: false }, "me").unwrap();
        rooms.persist();
        let file_of = |name: String| dir.join("store").join(name);
        let inode = |name: String| std::fs::metadata(file_of(name)).unwrap().ino();
        let (room, marker) = (inode(file(&r.id)), inode(marker_file(&r.id)));
        assert!(!std::fs::read_to_string(file_of(file(&r.id))).unwrap().contains("points"), "the marker is apart");
        // Nothing changed: only touched.
        let old = SystemTime::now() - Duration::from_secs(600);
        std::fs::File::options().write(true).open(file_of(file(&r.id))).unwrap().set_modified(old).unwrap();
        rooms.persist();
        assert_eq!((inode(file(&r.id)), inode(marker_file(&r.id))), (room, marker));
        let touched = std::fs::metadata(file_of(file(&r.id))).unwrap().modified().unwrap();
        assert!(touched > old + Duration::from_secs(60));
        // More marker: its file only.
        r.draw(&crate::rooms::room::Draw { stroke: 2, points: vec![[6.0, 6.0, 0.0]], keep: true, end: false }, "me").unwrap();
        rooms.persist();
        assert_eq!(inode(file(&r.id)), room);
        assert_ne!(inode(marker_file(&r.id)), marker);
        r.stop();

        let next = Rooms::new(Some(store(&dir)), None);
        assert_eq!(next.get(&r.id, &sessions).expect("picked up").saved().marker.len(), 2, "the marker came along");
    }

    #[test]
    fn a_relayed_room_is_picked_up_once_no_process_plays_it() {
        let (sessions, dir) = demo();
        let rooms = Rooms::new(Some(store(&dir)), None);
        let m = sessions.get("demo").unwrap();
        let (relayed, plain) =
            (rooms.open(&m, m.recording().unwrap(), "me", None).unwrap(), rooms.open(&m, m.recording().unwrap(), "me", None).unwrap());
        rooms.persist();
        let path = |id: &str| dir.join("store").join(file(id));
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(path(&relayed.id)).unwrap()).unwrap();
        v["room"]["relay"] = serde_json::json!({ "port": 28100, "password": "pw" });
        std::fs::write(path(&relayed.id), v.to_string()).unwrap();
        relayed.stop();
        plain.stop();
        let port = std::net::UdpSocket::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port();
        let pool = Some(Arc::new(Pool::Local(crate::relays::pool::Relays::new("127.0.0.1", [port]))));
        let next = Rooms::new(Some(store(&dir)), pool);
        next.pick_up_relayed(&sessions);
        assert_eq!(next.count(), 0, "still played by the process before");
        let quiet = SystemTime::now() - Duration::from_secs(10);
        for id in [&relayed.id, &plain.id] {
            std::fs::File::options().write(true).open(path(id)).unwrap().set_modified(quiet).unwrap();
        }
        next.pick_up_relayed(&sessions);
        assert_eq!(next.count(), 1, "the relayed one only");
        assert!(lock(&next.map).contains_key(&relayed.id));
        for r in lock(&next.map).values() {
            r.stop();
        }
    }

    #[test]
    fn one_person_s_third_room_ends_their_oldest_unwatched_and_fourth_playback_their_oldest() {
        let (sessions, dir) = demo();
        let rooms = Rooms::new(Some(store(&dir)), None);
        let m = sessions.get("demo").unwrap();
        let open = |by| rooms.open(&m, m.recording().unwrap(), "me", Some(by));
        let (a, b) = (open("ip:192.0.2.9").unwrap(), open("ip:192.0.2.9").unwrap());
        let _in_b = b.join("me", None, None, true);
        assert!(open("ip:192.0.2.9").err().unwrap().to_string().contains("close one first"), "both just opened");
        assert!(open("steam:7").is_ok(), "someone else");
        *lock(&a.idle_since) = Some(Instant::now().checked_sub(crate::matches::sessions::MAKES_ROOM).unwrap());
        let c = open("ip:192.0.2.9").unwrap();
        assert!(rooms.get(&a.id, &sessions).is_none(), "the unwatched one went");
        assert!(rooms.get(&b.id, &sessions).is_some() && rooms.get(&c.id, &sessions).is_some());
        // Private playbacks: the oldest goes, watched or not.
        let private = || rooms.open_private(&m, m.recording().unwrap(), "me", 0, 1.0, false, Some("ip:192.0.2.9")).unwrap();
        let first = private();
        let _watching = first.join("me", None, None, false);
        let (_, _) = (private(), private());
        private();
        assert!(!lock(&rooms.map).contains_key(&first.id));
        assert_eq!(lock(&rooms.private).len(), 3);
        for r in lock(&rooms.map).values() {
            r.stop();
        }
    }

    #[test]
    fn a_live_match_is_recorded_from_the_start_and_a_room_on_it_starts_at_live() {
        let sessions = Sessions::new(false, 4, 20.0);
        let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
        let recording = m.recording().unwrap();
        // A while of the match (play time goes by the wall clock).
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(30));
            crate::matches::hub::publish(&m.hub, &serde_json::from_str(r#"{"type":"stalemate","tick":5}"#).unwrap());
        }
        while recording.length() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let rooms = Rooms::default();
        let room = rooms.open(&m, m.recording().unwrap(), "me", None).unwrap();
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
        let rooms = Rooms::new(Some(store.clone()), None);
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
