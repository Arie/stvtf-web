//! A live match's `.dem` files, recorded as it is watched so rooms on it can be relayed to TF2
//! clients: one per connection to its server (a reconnect, a new map, a deploy starts another),
//! named after the server tick of its first frame. A new map drops the old one's, as the match's
//! recording does; the rest go with the match, or with the store's sweep of old files.

use crate::lock;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Part {
    path: PathBuf,
    map: String,
    first_tick: u32,
}

pub(crate) struct LiveDemos {
    dir: PathBuf,
    id: String,
    /// By first tick.
    parts: Mutex<Vec<Part>>,
}

impl LiveDemos {
    /// Match `id`'s demos in `dir`, with those a process before this one left there.
    pub(crate) fn new(dir: &Path, id: &str) -> Arc<LiveDemos> {
        let prefix = format!("live-{id}-");
        let mut parts: Vec<Part> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let first_tick = name.strip_prefix(&prefix)?.strip_suffix(".dem")?.parse().ok()?;
                let mut header = vec![0u8; stvtf::demo::HEADER_LEN];
                std::io::Read::read_exact(&mut File::open(e.path()).ok()?, &mut header).ok()?;
                let map = stvtf::demo::Header::parse(&header).ok()?.map;
                Some(Part { path: e.path(), map, first_tick })
            })
            .collect();
        parts.sort_by_key(|p| p.first_tick);
        Arc::new(LiveDemos { dir: dir.to_owned(), id: id.to_string(), parts: Mutex::new(parts) })
    }

    /// The demo with server tick `tick` of the map being watched: the last to start at or before
    /// it, or the first if it is before them all.
    pub(crate) fn at(&self, tick: u32) -> Option<PathBuf> {
        let parts = lock(&self.parts);
        parts.iter().rev().find(|p| p.first_tick <= tick).or(parts.first()).map(|p| p.path.clone())
    }

    /// Deletes them all: the match ended.
    pub(crate) fn forget(&self) {
        remove(&mut lock(&self.parts));
    }
}

/// Deletes `parts`' files and forgets them.
fn remove(parts: &mut Vec<Part>) {
    for p in parts.drain(..) {
        let _ = std::fs::remove_file(p.path);
    }
}

impl stvtf::demo::Files for LiveDemos {
    fn create(&self, map: &str, first_tick: u32) -> std::io::Result<File> {
        let mut parts = lock(&self.parts);
        // Another map, or the server's ticks started over (a restart): the ones before go.
        if parts.last().is_some_and(|p| p.map != map || p.first_tick > first_tick) {
            remove(&mut parts);
        }
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("live-{}-{first_tick}.dem", self.id));
        let file = File::create(&path)?;
        parts.retain(|p| p.path != path);
        parts.push(Part { path, map: map.to_string(), first_tick });
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use stvtf::demo::Files;

    fn header(map: &str) -> Vec<u8> {
        stvtf::demo::Header {
            protocol: 24,
            server: String::new(),
            client: String::new(),
            map: map.into(),
            game: "tf".into(),
            seconds: 0.0,
            ticks: 0,
            frames: 0,
            signon_bytes: 0,
        }
        .bytes()
    }

    #[test]
    fn a_tick_is_found_in_the_demo_it_falls_in() {
        let dir = TempDir::new("live-demos");
        let demos = LiveDemos::new(&dir, "m");
        assert_eq!(demos.at(5), None);
        std::io::Write::write_all(&mut demos.create("cp_a", 100).unwrap(), &header("cp_a")).unwrap();
        demos.create("cp_a", 500).unwrap();
        let (first, second) = (dir.join("live-m-100.dem"), dir.join("live-m-500.dem"));
        assert_eq!(demos.at(50), Some(first.clone()), "before them all: the first");
        assert_eq!(demos.at(499), Some(first.clone()));
        assert_eq!(demos.at(500), Some(second.clone()));
        assert_eq!(demos.at(9000), Some(second.clone()));
        // The next process finds the first, which has a header.
        assert_eq!(LiveDemos::new(&dir, "m").at(9000), Some(first.clone()));
        // A new map: the old one's go.
        demos.create("cp_b", 10).unwrap();
        assert!(!first.exists() && !second.exists());
        assert_eq!(demos.at(9000), Some(dir.join("live-m-10.dem")));
        demos.forget();
        assert_eq!(std::fs::read_dir(&*dir).unwrap().count(), 0);
    }
}
