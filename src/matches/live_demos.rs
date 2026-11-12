//! A live match's `.dem` files, recorded as it is watched so rooms on it can be relayed to TF2
//! clients: one per connection to its server (a reconnect, a new map, a deploy starts another),
//! named after the server tick of its first frame. A new map drops the old one's, as the match's
//! recording does; the rest go with the match, or with the store's sweep of old files.
//!
//! Voice from outside the match that its players said (a team's Discord) is kept beside the demo
//! it was heard in (`<demo>.voice`, a line each). A demo with any is not dropped but kept
//! (`kept-…`), for a while and within a budget, to download with the comms written into it:
//! [`downloads`], [`build`], [`sweep_kept`].

use crate::lock;
use crate::matches::sessions::LIVE_PREFIX;
use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// What the names of demos kept for downloading start with.
const KEPT_PREFIX: &str = "kept-";
/// What follows a demo's name in that of its voice.
const VOICE: &str = ".voice";

/// Something a player said from outside the match, as the match's stream came to its tick: what
/// writing it into a demo takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Spoken {
    /// The server's tick it was heard at.
    pub(crate) tick: u32,
    /// The player's slot: their entity less one.
    pub(crate) slot: u8,
    /// Their Steam account.
    pub(crate) account: u32,
    /// Opus frames, base64.
    pub(crate) frames: Vec<String>,
}

/// A demo to download with its comms in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Download {
    /// What names it in the match: the server tick of its first frame.
    pub(crate) file: String,
    pub(crate) map: String,
    pub(crate) bytes: u64,
}

/// A demo's voice file.
fn voice_of(demo: &Path) -> PathBuf {
    let mut name = demo.as_os_str().to_owned();
    name.push(VOICE);
    PathBuf::from(name)
}

/// Whether anything was said in `demo`.
fn has_voice(demo: &Path) -> bool {
    std::fs::metadata(voice_of(demo)).is_ok_and(|m| m.len() > 0)
}

/// The map a demo's header names.
fn map_of(demo: &Path) -> Option<String> {
    let mut header = vec![0u8; stvtf::demo::HEADER_LEN];
    std::io::Read::read_exact(&mut File::open(demo).ok()?, &mut header).ok()?;
    Some(stvtf::demo::Header::parse(&header).ok()?.map)
}

/// Match `id`'s demo whose first frame is at tick `file`, with comms: being recorded, or kept.
fn with_comms(dir: &Path, id: &str, file: &str) -> Option<PathBuf> {
    if file.is_empty() || !file.bytes().all(|b| b.is_ascii_digit()) || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    [LIVE_PREFIX, KEPT_PREFIX].iter().map(|p| dir.join(format!("{p}{id}-{file}.dem"))).find(|p| has_voice(p))
}

/// Match `id`'s demos with comms in `dir`, the earliest first.
pub(crate) fn downloads(dir: &Path, id: &str) -> Vec<Download> {
    let mut found: Vec<(u64, Download)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let rest = [LIVE_PREFIX, KEPT_PREFIX].iter().find_map(|p| name.strip_prefix(p))?;
            let file = rest.strip_prefix(id)?.strip_prefix('-')?.strip_suffix(".dem")?;
            let path = with_comms(dir, id, file)?;
            let download = Download { file: file.to_string(), map: map_of(&path)?, bytes: e.metadata().ok()?.len() };
            Some((file.parse().ok()?, download))
        })
        .collect();
    found.sort_by_key(|(tick, _)| *tick);
    found.into_iter().map(|(_, d)| d).collect()
}

/// Match `id`'s demo `file` as TF2 plays it, the comms written into it as its players' voice:
/// what to call it, and the demo.
pub(crate) fn build(dir: &Path, id: &str, file: &str) -> Result<(String, Vec<u8>)> {
    let path = with_comms(dir, id, file).context("no such demo")?;
    let first: i64 = file.parse()?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut voices = Vec::new();
    for line in std::io::BufReader::new(File::open(voice_of(&path))?).lines() {
        // A line cut short (the match still going) is left out.
        let Ok(said) = serde_json::from_str::<Spoken>(&line?) else { continue };
        voices.push(stvtf::demo::Voiced {
            tick: i32::try_from(i64::from(said.tick) - first).unwrap_or(0),
            slot: said.slot,
            account: said.account,
            rate: 24000,
            frames: said.frames.iter().filter_map(|f| b64.decode(f).ok()).collect(),
        });
    }
    let map = map_of(&path).context("not a demo")?;
    if !map.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        bail!("a map of that name");
    }
    let src = std::io::BufReader::new(File::open(&path)?);
    let demo = stvtf::demo::with_voice(src, &voices, std::io::Cursor::new(Vec::new()))?.into_inner();
    Ok((format!("{map}-comms-{file}.dem"), demo))
}

/// Drops kept demos (and their voice) older than `max_age`, then the oldest until what is left
/// fits in `budget` bytes.
pub(crate) fn sweep_kept(dir: &Path, max_age: Duration, budget: u64) {
    let now = SystemTime::now();
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(KEPT_PREFIX) || e.path().extension().is_none_or(|x| x != "dem") {
                return None;
            }
            let meta = e.metadata().ok()?;
            let voice = std::fs::metadata(voice_of(&e.path())).map_or(0, |m| m.len());
            Some((meta.modified().ok()?, meta.len() + voice, e.path()))
        })
        .collect();
    kept.sort_by_key(|(written, ..)| std::cmp::Reverse(*written));
    let mut total = 0;
    for (written, bytes, path) in kept {
        total += bytes;
        if total > budget || now.duration_since(written).is_ok_and(|age| age > max_age) {
            total -= bytes;
            let _ = std::fs::remove_file(voice_of(&path));
            let _ = std::fs::remove_file(path);
        }
    }
}

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
        let prefix = format!("{LIVE_PREFIX}{id}-");
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

    /// The match ended: its demos go, but for those with comms, which are kept.
    pub(crate) fn forget(&self) {
        remove(&mut lock(&self.parts));
    }

    /// A player said something from outside the match: kept beside the demo being recorded.
    pub(crate) fn said(&self, said: &Spoken) {
        let parts = lock(&self.parts);
        let Some(part) = parts.last() else { return };
        let written = serde_json::to_string(said)
            .map_err(std::io::Error::other)
            .and_then(|line| writeln!(File::options().create(true).append(true).open(voice_of(&part.path))?, "{line}"));
        if let Err(e) = written {
            log!("keeping voice for a demo: {e}");
        }
    }
}

/// Forgets `parts`: a demo with comms is kept under another name, to download; the rest are
/// deleted.
fn remove(parts: &mut Vec<Part>) {
    for p in parts.drain(..) {
        let kept = p
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix(LIVE_PREFIX))
            .map(|n| p.path.with_file_name(format!("{KEPT_PREFIX}{n}")));
        if let Some(kept) = kept.filter(|_| has_voice(&p.path)) {
            let _ = std::fs::rename(voice_of(&p.path), voice_of(&kept));
            let _ = std::fs::rename(&p.path, kept);
        } else {
            let _ = std::fs::remove_file(voice_of(&p.path));
            let _ = std::fs::remove_file(p.path);
        }
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
        let path = self.dir.join(format!("{LIVE_PREFIX}{}-{first_tick}.dem", self.id));
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
    use std::time::Duration;
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
        Write::write_all(&mut demos.create("cp_a", 100).unwrap(), &header("cp_a")).unwrap();
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

    /// A demo of `map` with frames at these server ticks.
    fn write_demo(file: File, map: &str, ticks: &[i32]) {
        let mut w =
            stvtf::demo::Writer::new(std::io::BufWriter::new(file), stvtf::demo::Header::parse(&header(map)).unwrap(), 0.015).unwrap();
        w.signon(b"signon").unwrap();
        for t in ticks {
            w.message(*t, b"game").unwrap();
        }
        w.finish().unwrap();
    }

    fn spoken(tick: u32) -> Spoken {
        Spoken { tick, slot: 3, account: 5, frames: vec!["aA==".into()] }
    }

    #[test]
    fn a_map_with_comms_is_kept_for_downloading_and_one_without_is_not() {
        let dir = TempDir::new("kept-demos");
        let demos = LiveDemos::new(&dir, "m");
        assert_eq!(downloads(&dir, "m"), []);
        write_demo(demos.create("cp_a", 100).unwrap(), "cp_a", &[100, 104, 110]);
        demos.said(&spoken(103));
        demos.said(&spoken(108));
        let live = Download { file: "100".into(), map: "cp_a".into(), bytes: std::fs::metadata(dir.join("live-m-100.dem")).unwrap().len() };
        assert_eq!(downloads(&dir, "m"), std::slice::from_ref(&live), "while it is played, too");
        // The demo as TF2 plays it, the comms in it at their ticks.
        let (name, demo) = build(&dir, "m", "100").unwrap();
        assert_eq!(name, "cp_a-comms-100.dem");
        let mut r = stvtf::demo::Reader::new(std::io::Cursor::new(&demo)).unwrap();
        assert_eq!((r.header.map.as_str(), r.header.frames), ("cp_a", 5), "three frames and two of voice");
        let mut ticks = Vec::new();
        while let stvtf::demo::Next::Packet(p) = r.next_packet().unwrap() {
            if let stvtf::demo::Packet::Message { tick, .. } = p {
                ticks.push(tick);
            }
        }
        assert_eq!(ticks, [0, 3, 4, 8, 10]);
        // Another map: this one's demo and comms are kept, under another name; a demo nobody
        // talked in is not.
        write_demo(demos.create("cp_b", 10).unwrap(), "cp_b", &[10, 14]);
        assert!(!dir.join("live-m-100.dem").exists());
        assert_eq!(downloads(&dir, "m"), std::slice::from_ref(&live), "kept");
        assert!(build(&dir, "m", "100").is_ok());
        demos.forget();
        assert_eq!(downloads(&dir, "m"), [live], "the match gone, its comms stay");
        assert!(!dir.join("live-m-10.dem").exists(), "the map nobody talked in is gone");
        assert!(build(&dir, "m", "10").is_err() && build(&dir, "other", "100").is_err() && build(&dir, "m", "../x").is_err());
    }

    #[test]
    fn kept_demos_go_when_old_or_over_the_budget_the_oldest_first() {
        let dir = TempDir::new("kept-sweep");
        let day = Duration::from_hours(24);
        let now = SystemTime::now();
        for (name, age) in [("kept-a-1.dem", 20), ("kept-b-1.dem", 3), ("kept-c-1.dem", 2), ("kept-d-1.dem", 1)] {
            for path in [dir.join(name), dir.join(format!("{name}.voice"))] {
                std::fs::write(&path, vec![0u8; 1000]).unwrap();
                File::options().write(true).open(&path).unwrap().set_modified(now - day * age).unwrap();
            }
        }
        std::fs::write(dir.join("live-x-1.dem"), b"not ours to sweep").unwrap();
        // Two weeks, and room for two demos with their comms.
        sweep_kept(&dir, day * 14, 4000);
        let mut left: Vec<String> =
            std::fs::read_dir(&*dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["kept-c-1.dem", "kept-c-1.dem.voice", "kept-d-1.dem", "kept-d-1.dem.voice", "live-x-1.dem"]);
    }
}
