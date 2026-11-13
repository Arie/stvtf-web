//! A live match's `.dem` files, recorded as it is watched so rooms on it can be relayed to TF2
//! clients: one per connection to its server (a reconnect, a new map, a deploy starts another),
//! `live-<match>-<load>-<tick>.dem`: the server tick of its first frame, and the load it is of,
//! its part's number in the recording (one rule, [`Loads`], fed the lines the recording gets).
//! Those of earlier loads stay, so rooms on them can be relayed, till the recording drops their
//! part or the match ends. The names of the version before, `live-<match>-<tick>.dem`, are given
//! loads by their order in time.
//!
//! Voice from outside the match that its players said (a team's Discord) is kept beside the demo
//! it was heard in (`<demo>.voice`, a line each). A demo with any is not dropped but kept
//! (`kept-…`, once no room can be on it: its part dropped, or the match ended), for a while and
//! within a budget, to download with the comms written into it: [`downloads`], [`build`],
//! [`sweep_kept`]. Till then it is downloaded under its `live-` name.

use crate::lock;
use crate::matches::loads::Loads;
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

/// A demo of the match being relayed.
struct Demo {
    path: PathBuf,
    map: String,
    /// The load it is of: its part's number.
    load: usize,
    first_tick: u32,
    /// Named by the version in production, without its load: how many loads before the newest of
    /// those it is.
    back: Option<usize>,
}

/// Where the version in production put a demo of an earlier map: never relayed, gone with the match.
const PAST: &str = "past-";

#[derive(Default)]
struct Inner {
    /// By load, then first tick.
    demos: Vec<Demo>,
    /// The version in production's demos of earlier maps, and its own not of the load it says.
    past: Vec<PathBuf>,
    /// The match's loads, as the lines its recording gets say.
    loads: Loads,
}

pub(crate) struct LiveDemos {
    dir: PathBuf,
    id: String,
    inner: Mutex<Inner>,
}

/// `path`'s name with `kept-` for `live-`, if it has that.
fn kept_name(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?.strip_prefix(LIVE_PREFIX)?;
    Some(path.with_file_name(format!("{KEPT_PREFIX}{name}")))
}

/// A demo no room can be on any more: kept for downloading under a `kept-` name if anything was
/// said in it, else gone.
fn drop_or_keep(path: &Path) {
    if !has_voice(path) {
        let _ = std::fs::remove_file(voice_of(path));
        let _ = std::fs::remove_file(path);
    } else if let Some(kept) = kept_name(path) {
        let _ = std::fs::rename(voice_of(path), voice_of(&kept));
        let _ = std::fs::rename(path, kept);
    }
}

/// Match `id`'s demos in `dir`, as a restart finds them. Of those named without their load, the
/// `live-` ones are of the load the match was at, and those before it where its ticks started
/// over: in time order, one of another map or with a lower first tick than the one before starts
/// the next. The `kept-` ones are for downloading only.
fn found(dir: &Path, id: &str) -> Inner {
    let mut inner = Inner::default();
    let mut unnumbered = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let live = name.starts_with(LIVE_PREFIX);
        let Some(rest) = [LIVE_PREFIX, KEPT_PREFIX].iter().find_map(|p| name.strip_prefix(p)) else { continue };
        let Some(file) = rest.strip_prefix(id).and_then(|r| r.strip_prefix('-')).and_then(|r| r.strip_suffix(".dem")) else { continue };
        if live && file.starts_with(PAST) {
            inner.past.push(e.path());
            continue;
        }
        let (Some((load, first_tick)), Some(map)) = (load_tick(file), map_of(&e.path())) else { continue };
        let demo = Demo { path: e.path(), map, load, first_tick, back: None };
        if file.contains('-') {
            inner.demos.push(demo);
        } else if live {
            let written = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            unnumbered.push((written, demo));
        }
    }
    unnumbered.sort_by_key(|(written, d)| (*written, d.first_tick));
    let mut group = 0;
    for i in 0..unnumbered.len() {
        let (before, d) = (i.checked_sub(1).map(|b| &unnumbered[b].1), &unnumbered[i].1);
        group += usize::from(before.is_some_and(|b| b.map != d.map || d.first_tick < b.first_tick));
        unnumbered[i].1.load = group;
    }
    inner.demos.extend(unnumbered.into_iter().map(|(_, d)| Demo { back: Some(group - d.load), ..d }));
    inner
}

impl Inner {
    /// The load the match was at when the version in production left off, and its map if known:
    /// that of the first demo named with its load (the reconnect after the deploy), else `latest`.
    fn deployed_at(&self, latest: usize, map: Option<&str>) -> (usize, Option<String>) {
        let numbered = self.demos.iter().filter(|d| d.back.is_none()).min_by_key(|d| d.load);
        numbered.map_or((latest, map.map(str::to_string)), |d| (d.load, Some(d.map.clone())))
    }

    /// Numbers the demos named without their load back from load `at` of `map`; none of them is
    /// relayed if the newest is of another map.
    fn number_back(&mut self, (at, map): (usize, Option<String>)) {
        let newest = self.demos.iter().find(|d| d.back == Some(0)).map(|d| d.map.clone());
        let wrong = map.is_some() && newest.is_some() && map != newest;
        let (gone, demos): (Vec<Demo>, Vec<Demo>) =
            std::mem::take(&mut self.demos).into_iter().partition(|d| d.back.is_some_and(|back| wrong || back > at));
        self.past.extend(gone.into_iter().map(|d| d.path));
        self.demos = demos;
        for d in &mut self.demos {
            if let Some(back) = d.back {
                d.load = at - back;
            }
        }
        self.demos.sort_by_key(|d| (d.load, d.first_tick));
    }
}

impl LiveDemos {
    /// Match `id`'s demos in `dir`, with those a process before this one left there; its loads go
    /// on from the latest of them, till [`LiveDemos::go_on_from`] says better.
    pub(crate) fn new(dir: &Path, id: &str) -> Arc<LiveDemos> {
        let mut inner = found(dir, id);
        let newest = inner.demos.iter().filter_map(|d| d.back).max().unwrap_or(0);
        inner.number_back(inner.deployed_at(newest, None));
        inner.loads = inner.demos.last().map_or_else(Loads::default, |d| Loads::from(d.load, Some(d.map.clone()), Some(d.first_tick)));
        Arc::new(LiveDemos { dir: dir.to_owned(), id: id.to_string(), inner: Mutex::new(inner) })
    }

    /// The match's loads go on from `loads`, its recording's, resumed: the demos named without
    /// their load are of its latest load, and those before it.
    pub(crate) fn go_on_from(&self, loads: Loads) {
        let mut inner = lock(&self.inner);
        let at = inner.deployed_at(loads.latest(), loads.map());
        inner.number_back(at);
        inner.loads = loads;
    }

    /// A line of the match, as its recording gets it: the load a snapshot is of.
    pub(crate) fn line(&self, line: &stvtf::Line) -> Option<usize> {
        let loads = &mut lock(&self.inner).loads;
        match line {
            stvtf::Line::Server(s) => loads.server(&s.map),
            stvtf::Line::Snapshot(s) => return Some(loads.snapshot(s.tick).0),
            _ => {}
        }
        None
    }

    /// The match's loads now.
    #[cfg(test)]
    pub(crate) fn loads(&self) -> Loads {
        lock(&self.inner).loads.clone()
    }

    /// The demo with server tick `tick` of load `n`: the last of its demos to start at or before
    /// it, or the first if it is before them all.
    pub(crate) fn at(&self, n: usize, tick: u32) -> Option<PathBuf> {
        let inner = lock(&self.inner);
        let mut of = inner.demos.iter().filter(|d| d.load == n);
        let first = of.next()?;
        Some(of.take_while(|d| d.first_tick <= tick).last().unwrap_or(first).path.clone())
    }

    /// The loads before `n` are not kept any more: their demos go, but for those with comms,
    /// which are kept.
    pub(crate) fn keep_from(&self, n: usize) {
        let mut inner = lock(&self.inner);
        if inner.demos.first().is_none_or(|d| d.load >= n) {
            return;
        }
        let at = inner.demos.partition_point(|d| d.load < n);
        for d in inner.demos.drain(..at) {
            drop_or_keep(&d.path);
        }
    }

    /// The match ended: its demos go, those of loads before too, but for those with comms, which
    /// are kept.
    pub(crate) fn forget(&self) {
        let mut inner = lock(&self.inner);
        for d in inner.demos.drain(..) {
            drop_or_keep(&d.path);
        }
        for path in inner.past.drain(..) {
            drop_or_keep(&path);
        }
    }

    /// A player said something from outside the match: kept beside the demo being recorded.
    pub(crate) fn said(&self, said: &Spoken) {
        let inner = lock(&self.inner);
        let Some(demo) = inner.demos.last() else { return };
        let written = serde_json::to_string(said)
            .map_err(std::io::Error::other)
            .and_then(|line| writeln!(File::options().create(true).append(true).open(voice_of(&demo.path))?, "{line}"));
        if let Err(e) = written {
            log!("keeping voice for a demo: {e}");
        }
    }
}

impl stvtf::demo::Files for LiveDemos {
    fn create(&self, map: &str, first_tick: u32) -> std::io::Result<File> {
        let mut inner = lock(&self.inner);
        // Made just before its first frame reaches the recording: the load that frame will be of.
        let load = inner.loads.would_be(map, first_tick);
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{LIVE_PREFIX}{}-{load}-{first_tick}.dem", self.id));
        let file = File::create(&path)?;
        inner.demos.retain(|d| d.path != path);
        inner.demos.push(Demo { path, map: map.to_string(), load, first_tick, back: None });
        inner.demos.sort_by_key(|d| (d.load, d.first_tick));
        Ok(file)
    }
}

#[cfg(test)]
mod tests;
