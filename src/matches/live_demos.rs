//! A live match's `.dem` files, recorded as it is watched so rooms on it can be relayed to TF2
//! clients: one per connection to its server (a reconnect, a new map, a deploy starts another),
//! `live-<match>-<load>-<tick>.dem`: the server tick of its first frame, and the number of the
//! load it belongs to (0 the first), a load being a map loaded (another map, or ticks starting
//! over). Those of earlier loads stay, so rooms on them can be relayed; all go with the match, or
//! with the store's sweep of old files. The names of the version before, `live-<match>-<tick>.dem`,
//! are of load 0.
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
    /// What names it in the match: `<load>-<tick>` of its first frame (`<tick>` alone, in the
    /// names of the version before).
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

/// The load and first tick a demo's `file` (what follows the match in its name) says.
fn load_tick(file: &str) -> Option<(usize, u32)> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (load, tick) = file.split_once('-').unwrap_or(("0", file));
    (digits(load) && digits(tick)).then_some((load.parse().ok()?, tick.parse().ok()?))
}

/// Match `id`'s demo `file`, with comms: being recorded, or kept.
fn with_comms(dir: &Path, id: &str, file: &str) -> Option<PathBuf> {
    if load_tick(file).is_none() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    [LIVE_PREFIX, KEPT_PREFIX].iter().map(|p| dir.join(format!("{p}{id}-{file}.dem"))).find(|p| has_voice(p))
}

/// Match `id`'s demos with comms in `dir`, the earliest load first.
pub(crate) fn downloads(dir: &Path, id: &str) -> Vec<Download> {
    let mut found: Vec<((usize, u32), Download)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let rest = [LIVE_PREFIX, KEPT_PREFIX].iter().find_map(|p| name.strip_prefix(p))?;
            let file = rest.strip_prefix(id)?.strip_prefix('-')?.strip_suffix(".dem")?;
            let path = with_comms(dir, id, file)?;
            let download = Download { file: file.to_string(), map: map_of(&path)?, bytes: e.metadata().ok()?.len() };
            Some((load_tick(file)?, download))
        })
        .collect();
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, d)| d).collect()
}

/// Match `id`'s demo `file` as TF2 plays it, the comms written into it as its players' voice:
/// what to call it, and the demo.
pub(crate) fn build(dir: &Path, id: &str, file: &str) -> Result<(String, Vec<u8>)> {
    let path = with_comms(dir, id, file).context("no such demo")?;
    let first = i64::from(load_tick(file).context("no such demo")?.1);
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
    /// Which load of the match it is of, 0 the first.
    load: usize,
    first_tick: u32,
}

pub(crate) struct LiveDemos {
    dir: PathBuf,
    id: String,
    /// By load, then first tick.
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
                let (load, first_tick) = load_tick(name.strip_prefix(&prefix)?.strip_suffix(".dem")?)?;
                Some(Part { map: map_of(&e.path())?, path: e.path(), load, first_tick })
            })
            .collect();
        parts.sort_by_key(|p| (p.load, p.first_tick));
        Arc::new(LiveDemos { dir: dir.to_owned(), id: id.to_string(), parts: Mutex::new(parts) })
    }

    /// The demo with server tick `tick` of the `again`-th load (0 the first) of `map`: the last of
    /// its demos to start at or before it, or the first if it is before them all.
    pub(crate) fn at(&self, map: &str, again: usize, tick: u32) -> Option<PathBuf> {
        let parts = lock(&self.parts);
        let mut loads: Vec<usize> = parts.iter().filter(|p| p.map == map).map(|p| p.load).collect();
        loads.dedup();
        let load = *loads.get(again)?;
        let mut of = parts.iter().filter(|p| p.load == load);
        let first = of.next()?;
        Some(of.take_while(|p| p.first_tick <= tick).last().unwrap_or(first).path.clone())
    }

    /// The match ended: its demos go, those of loads before too, but for those with comms, which
    /// are kept.
    pub(crate) fn forget(&self) {
        for p in lock(&self.parts).drain(..) {
            let name = p.path.file_name().and_then(|n| n.to_str());
            let kept = name.and_then(|n| n.strip_prefix(LIVE_PREFIX)).map(|n| p.path.with_file_name(format!("{KEPT_PREFIX}{n}")));
            if let Some(kept) = kept.filter(|_| has_voice(&p.path)) {
                let _ = std::fs::rename(voice_of(&p.path), voice_of(&kept));
                let _ = std::fs::rename(&p.path, kept);
                continue;
            }
            let _ = std::fs::remove_file(voice_of(&p.path));
            let _ = std::fs::remove_file(p.path);
        }
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

impl stvtf::demo::Files for LiveDemos {
    fn create(&self, map: &str, first_tick: u32) -> std::io::Result<File> {
        let mut parts = lock(&self.parts);
        // Another map, or the server's ticks started over (a restart): a new load.
        let load = match parts.last() {
            Some(p) if p.map != map || p.first_tick > first_tick => p.load + 1,
            Some(p) => p.load,
            None => 0,
        };
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{LIVE_PREFIX}{}-{load}-{first_tick}.dem", self.id));
        let file = File::create(&path)?;
        parts.retain(|p| p.path != path);
        parts.push(Part { path, map: map.to_string(), load, first_tick });
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
        assert_eq!(demos.at("cp_a", 0, 5), None);
        Write::write_all(&mut demos.create("cp_a", 100).unwrap(), &header("cp_a")).unwrap();
        demos.create("cp_a", 500).unwrap();
        let (first, second) = (dir.join("live-m-0-100.dem"), dir.join("live-m-0-500.dem"));
        assert_eq!(demos.at("cp_a", 0, 50), Some(first.clone()), "before them all: the first");
        assert_eq!(demos.at("cp_a", 0, 499), Some(first.clone()));
        assert_eq!(demos.at("cp_a", 0, 500), Some(second.clone()));
        assert_eq!(demos.at("cp_a", 0, 9000), Some(second));
        // The next process finds the first, which has a header.
        assert_eq!(LiveDemos::new(&dir, "m").at("cp_a", 0, 9000), Some(first));
    }

    #[test]
    fn each_load_of_a_map_keeps_its_demos() {
        let dir = TempDir::new("live-demos");
        let demos = LiveDemos::new(&dir, "m");
        for (map, tick) in [("cp_a", 100), ("cp_a", 500), ("cp_b", 10), ("cp_a", 20)] {
            Write::write_all(&mut demos.create(map, tick).unwrap(), &header(map)).unwrap();
        }
        assert_eq!(demos.at("cp_a", 0, 499), Some(dir.join("live-m-0-100.dem")));
        assert_eq!(demos.at("cp_a", 0, 9000), Some(dir.join("live-m-0-500.dem")), "a reconnect: the same load");
        assert_eq!(demos.at("cp_b", 0, 9000), Some(dir.join("live-m-1-10.dem")));
        assert_eq!(demos.at("cp_a", 1, 9000), Some(dir.join("live-m-2-20.dem")), "the same map again: its own");
        assert_eq!(demos.at("cp_c", 0, 1), None);
        let next = LiveDemos::new(&dir, "m");
        assert_eq!(next.at("cp_a", 1, 25), Some(dir.join("live-m-2-20.dem")), "the next process finds them");
        demos.forget();
        assert_eq!(std::fs::read_dir(&*dir).unwrap().count(), 0);
    }

    #[test]
    fn a_demo_named_by_the_version_before_is_the_first_load() {
        let dir = TempDir::new("live-demos");
        std::fs::write(dir.join("live-m-100.dem"), header("cp_a")).unwrap();
        assert_eq!(LiveDemos::new(&dir, "m").at("cp_a", 0, 150), Some(dir.join("live-m-100.dem")));
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
        let live = Download { file: "0-100".into(), map: "cp_a".into(), bytes: std::fs::metadata(dir.join("live-m-0-100.dem")).unwrap().len() };
        assert_eq!(downloads(&dir, "m"), std::slice::from_ref(&live), "while it is played, too");
        // The demo as TF2 plays it, the comms in it at their ticks.
        let (name, demo) = build(&dir, "m", "0-100").unwrap();
        assert_eq!(name, "cp_a-comms-0-100.dem");
        let mut r = stvtf::demo::Reader::new(std::io::Cursor::new(&demo)).unwrap();
        assert_eq!((r.header.map.as_str(), r.header.frames), ("cp_a", 5), "three frames and two of voice");
        let mut ticks = Vec::new();
        while let stvtf::demo::Next::Packet(p) = r.next_packet().unwrap() {
            if let stvtf::demo::Packet::Message { tick, .. } = p {
                ticks.push(tick);
            }
        }
        assert_eq!(ticks, [0, 3, 4, 8, 10]);
        // Another map: this one's stay as they are. The match gone, those with comms are kept
        // under another name; a demo nobody talked in is not.
        write_demo(demos.create("cp_b", 10).unwrap(), "cp_b", &[10, 14]);
        assert_eq!(downloads(&dir, "m"), std::slice::from_ref(&live), "still there");
        assert!(build(&dir, "m", "0-100").is_ok());
        demos.forget();
        assert!(dir.join("kept-m-0-100.dem").exists() && !dir.join("live-m-0-100.dem").exists());
        assert_eq!(downloads(&dir, "m"), [live], "the match gone, its comms stay");
        assert!(!dir.join("live-m-1-10.dem").exists(), "the map nobody talked in is gone");
        assert!(build(&dir, "m", "1-10").is_err() && build(&dir, "other", "0-100").is_err() && build(&dir, "m", "../x").is_err());
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
