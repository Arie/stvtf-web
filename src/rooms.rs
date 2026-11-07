//! The rooms open now. With a store (`--sessions`), each is saved as it goes, so the process that
//! replaces this one (a deploy) picks it up when its pages reconnect: a pause of a second or two.

use crate::lock;
use crate::recording::Recording;
use crate::room::{Room, Saved};
use crate::sessions::{Session, Sessions, Store};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
}

/// What a room plays: a fixed match's recording, there again in the next process; or a demo's
/// output, saved beside the room since its match is not.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Source {
    Pinned(String),
    Demo,
}

#[derive(Serialize, Deserialize)]
struct SavedRoom {
    source: Source,
    room: Saved,
}

/// A room's file in the store, and its demo's beside it.
fn file(id: &str) -> String {
    format!("room-{id}")
}
fn demo_file(id: &str) -> String {
    format!("room-{id}.gz")
}

impl Rooms {
    pub(crate) fn new(store: Option<Store>) -> Rooms {
        Rooms { store, ..Rooms::default() }
    }

    /// A room on match `from`, owned by `owner`.
    pub(crate) fn open(&self, from: &Session, recording: Arc<Recording>, owner: &str) -> Result<Arc<Room>> {
        let id = crate::sessions::random_id();
        let source = if from.is_pinned() { Source::Pinned(from.id.clone()) } else { Source::Demo };
        if let (Source::Demo, Some(store)) = (&source, &self.store) {
            let Some(output) = from.output() else { bail!("the demo is still being fetched") };
            store.put(&demo_file(&id), &output)?;
        }
        let room = Room::start(id, owner, recording);
        lock(&self.sources).insert(room.id.clone(), source);
        lock(&self.map).insert(room.id.clone(), room.clone());
        self.save(&room);
        Ok(room)
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
            Source::Pinned(match_id) => sessions.get(match_id)?.recording().ok()?,
            Source::Demo => {
                let gz = store.take(&demo_file(id)).ok()?;
                Arc::new(Recording::read(flate2::read::GzDecoder::new(gz.as_slice()), &format!("room {id}")).ok()?)
            }
        };
        let mut map = lock(&self.map);
        // Another page of it may have picked it up meanwhile.
        if let Some(room) = map.get(id) {
            return Some(room.clone());
        }
        let room = Room::restore(id.to_string(), saved.room, recording);
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
            lock(&self.sources).remove(&id);
            if let Some(store) = &self.store {
                store.forget(&file(&id));
                store.forget(&demo_file(&id));
            }
            eprintln!("stvtf-web: room {} ended", &id[..8]);
        }
    }

    /// Drops saved rooms nobody came back for.
    pub(crate) fn sweep(&self) {
        let Some(store) = &self.store else { return };
        let Ok(entries) = std::fs::read_dir(&store.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let old =
                entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() > STALE);
            if name.starts_with("room-") && old {
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

    fn saved_at(s: &Saved) -> u64 {
        serde_json::to_value(s).unwrap()["at"].as_u64().unwrap()
    }
}
