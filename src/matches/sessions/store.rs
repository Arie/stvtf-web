//! Where matches are saved for the next process, and the files kept beside them.

use super::session::Session;
use crate::lock;
use crate::maps::cache::write_atomic;
use crate::matches::recording::{Save, Sink};
use crate::web::token;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

/// A file per match, so the containers of a deploy, briefly running side by side, never write
/// over each other's list. Encrypted with the token key when there is one.
///
/// The newest process owns the folder: it writes its mark to [`CURRENT`] on start, and from then
/// on the release it replaces saves only the matches it starts, for the new one to pick up when
/// their pages reconnect to it, and not the others, so it cannot bring back a match the new one
/// ended.
#[derive(Clone)]
pub struct Store {
    pub dir: PathBuf,
    pub keys: Option<Arc<token::Keys>>,
}

/// A running match as saved for the next process: what it takes to watch it again, and its feed.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SavedMatch {
    pub(super) server: SocketAddr,
    pub(super) password: String,
    pub(super) owner: Option<String>,
    #[serde(default)]
    pub(super) reservation: Option<u64>,
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The feed's history lines, as sent to pages.
    #[serde(default)]
    pub(super) history: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) voice_invite: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::voice::mumble::saved")]
    pub(super) mumble: Vec<crate::voice::mumble::MumbleJob>,
}

/// The file in the store holding the mark of the process it belongs to.
pub(super) const CURRENT: &str = "current";

impl Store {
    pub(super) fn path(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// Takes the folder over from whichever process had it.
    pub(super) fn claim(&self, mark: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(write_atomic(&self.path(CURRENT), mark.as_bytes())?)
    }

    /// Whether a newer process took the folder over from the one with `mark`.
    pub(super) fn taken_over(&self, mark: &str) -> bool {
        std::fs::read(self.path(CURRENT)).is_ok_and(|current| current != mark.as_bytes())
    }

    /// Saves a match, unless it was stopped (ended, or all of them stopped at exit), plays a demo
    /// (the next process could not pick up where it is), or its map and feed are as last saved.
    pub(super) fn save(&self, s: &Session) -> Result<()> {
        let mut last_saved = lock(&s.saving);
        if s.stop.load(Ordering::Relaxed) || s.demo.is_some() {
            return Ok(());
        }
        let (changes, map, history) = {
            let h = lock(&s.hub);
            if *last_saved == Some(h.changes) {
                return Ok(());
            }
            (h.changes, h.map.clone(), h.history.iter().map(ToString::to_string).collect())
        };
        let saved = SavedMatch {
            server: s.server,
            password: s.password.clone(),
            owner: s.owner.clone(),
            reservation: s.reservation(),
            map,
            history,
            by: s.by.get().cloned(),
            voice_invite: s.comms.invite.get().cloned(),
            mumble: lock(&s.comms.mumble).clone(),
        };
        self.put(&s.id, &serde_json::to_vec(&saved)?)?;
        *last_saved = Some(changes);
        Ok(())
    }

    pub(crate) fn forget(&self, id: &str) {
        let _ = std::fs::remove_file(self.path(id));
    }

    /// Writes `plain` as `name`, sealed when there are keys.
    pub(crate) fn put(&self, name: &str, plain: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let data = self.keys.as_ref().map_or_else(|| plain.to_vec(), |k| k.seal(plain));
        Ok(write_atomic(&self.path(name), &data)?)
    }

    /// Adds `data` to the end of `name`, as it is: not sealed (a match's lines, which `SourceTV`
    /// shows anyone).
    pub(crate) fn append(&self, name: &str, data: &[u8]) -> Result<()> {
        use std::io::Write as _;
        std::fs::create_dir_all(&self.dir)?;
        Ok(std::fs::File::options().create(true).append(true).open(self.path(name))?.write_all(data)?)
    }

    /// `data` as all of `name`, as it is, as for `append`.
    pub(crate) fn replace(&self, name: &str, data: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(write_atomic(&self.path(name), data)?)
    }

    /// What `append` or `replace` wrote as `name`.
    pub(crate) fn read(&self, name: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.path(name))?)
    }

    /// Whether there is a `name`.
    pub(crate) fn has(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// Forgets the files `which` names that were not written or touched for `max_age`.
    pub(crate) fn forget_older_than(&self, max_age: Duration, which: impl Fn(&str) -> bool) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if which(&name) && age(&entry).is_ok_and(|a| a > max_age) {
                self.forget(&name);
            }
        }
    }

    /// Marks `name` as in use now, so a sweep of old files leaves it.
    pub(crate) fn touch(&self, name: &str) {
        let _ = std::fs::File::options().write(true).open(self.path(name)).and_then(|f| f.set_modified(SystemTime::now()));
    }

    /// What `put` wrote as `name`.
    pub(crate) fn take(&self, name: &str) -> Result<Vec<u8>> {
        let data = std::fs::read(self.path(name))?;
        Ok(match &self.keys {
            Some(k) => k.unseal(&data)?,
            None => data,
        })
    }

    /// The match `save` wrote as `id`.
    pub(super) fn load(&self, id: &str) -> Result<SavedMatch> {
        Ok(serde_json::from_slice(&self.take(id)?)?)
    }
}

/// The `.dem` a recorded match's file was made from, if it was put beside it: `x.dem` for
/// `x.jsonl.gz` or `x.jsonl`.
pub(super) fn dem_beside(file: &std::path::Path) -> PathBuf {
    let name = file.file_name().map_or_default(|n| n.to_string_lossy().into_owned());
    let stem = name.strip_suffix(".gz").unwrap_or(&name);
    let stem = stem.strip_suffix(".jsonl").unwrap_or(stem);
    file.with_file_name(format!("{stem}.dem"))
}

/// What the names of a demos.tf demo's files in the store start with.
pub(crate) const DEMO_PREFIX: &str = "demo-";
/// What the names of a live match's files in the store start with.
pub(crate) const LIVE_PREFIX: &str = "live-";

/// A demos.tf demo's file in the store, kept for the next to play it where relays do.
pub(crate) fn demo_dem(demo: u64) -> String {
    format!("{DEMO_PREFIX}{demo}.dem")
}

/// What a demos.tf demo decodes to (gzipped stvtf output), kept for the next to play it.
pub(crate) fn demo_output(demo: u64) -> String {
    format!("{DEMO_PREFIX}{demo}.gz")
}

/// How long ago a file in the store was written or touched.
pub(crate) fn age(entry: &std::fs::DirEntry) -> std::io::Result<Duration> {
    entry.metadata()?.modified().map(|t| SystemTime::now().duration_since(t).unwrap_or_default())
}

/// Whether `name` is a demos.tf demo's file kept for the next to play it: not one half fetched.
pub(crate) fn is_kept_demo(name: &str) -> bool {
    name.starts_with(DEMO_PREFIX) && std::path::Path::new(name).extension().is_none_or(|e| e != "part")
}

/// Drops the demos.tf demos' files in `dir` not played for `max_age`, and the longest unplayed
/// past `budget` bytes of them; not one played within `fresh` (a match or a room may be on it).
pub(crate) fn sweep_demos(dir: &std::path::Path, max_age: Duration, budget: u64, fresh: Duration) {
    let mut kept: Vec<(Duration, u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| is_kept_demo(&e.file_name().to_string_lossy()))
        .filter_map(|e| Some((age(&e).ok()?, e.metadata().ok()?.len(), e.path())))
        .collect();
    kept.sort_by_key(|(age, ..)| *age);
    let mut total = 0;
    for (age, bytes, path) in kept {
        total += bytes;
        if age > fresh && (total > budget || age > max_age) {
            total -= bytes;
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A live match's recording beside its file.
pub(crate) fn live_file(id: &str) -> String {
    format!("{LIVE_PREFIX}{id}.gz")
}

/// Chunks to the end of file `name`, or all of it anew; the first that fails says so.
pub(crate) fn to_file(store: &Store, name: &str) -> Sink {
    let (store, name, mut said) = (store.clone(), name.to_string(), false);
    Box::new(move |save: &Save| {
        let saved = match save {
            Save::Add(gz) => store.append(&name, gz),
            Save::All(gz) => store.replace(&name, gz),
        };
        if let Err(e) = saved
            && !std::mem::replace(&mut said, true)
        {
            log!("saving {name}: {e:#}");
        }
    })
}
