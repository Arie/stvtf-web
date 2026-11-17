//! A match's live demos as they are written: the one being written and those before.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use super::Spoken;
use super::comms::voice_of;
use super::downloads::{load_tick, map_of};
use super::sweep::{KEPT_PREFIX, drop_or_keep};
use crate::matches::loads::Loads;
use crate::matches::sessions::LIVE_PREFIX;
use crate::report::fault;
use crate::sync::lock;

/// A demo of the match being relayed.
pub(super) struct Demo {
    pub(super) path: PathBuf,
    pub(super) map: String,
    /// The load it is of: its part's number.
    pub(super) load: usize,
    pub(super) first_tick: u32,
    /// Named without its load: how many loads before the newest of those it is.
    pub(super) back: Option<usize>,
}

/// The name of a demo of an earlier map set aside: never relayed, gone with the match.
pub(super) const PAST: &str = "past-";

#[derive(Default)]
pub(super) struct Inner {
    /// By load, then first tick.
    pub(super) demos: Vec<Demo>,
    /// Demos set aside as `past-`, and those not of the load their name says.
    pub(super) past: Vec<PathBuf>,
    /// The match's loads, as the lines its recording gets say.
    pub(super) loads: Loads,
}

pub(crate) struct LiveDemos {
    pub(super) dir: PathBuf,
    pub(super) id: String,
    pub(super) inner: Mutex<Inner>,
}

/// Match `id`'s demos in `dir`, as a restart finds them. Of those named without their load, the
/// `live-` ones are of the load the match was at, and those before it where its ticks started
/// over: in time order, one of another map or with a lower first tick than the one before starts
/// the next. The `kept-` ones are for downloading only.
pub(super) fn demos_on_disk(dir: &Path, id: &str) -> Inner {
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
    /// The load the match was at when its demos were last named without one, and its map if known:
    /// that of the first demo named with its load, else `latest`.
    pub(super) fn deployed_at(&self, latest: usize, map: Option<&str>) -> (usize, Option<String>) {
        let numbered = self.demos.iter().filter(|d| d.back.is_none()).min_by_key(|d| d.load);
        numbered.map_or((latest, map.map(str::to_string)), |d| (d.load, Some(d.map.clone())))
    }

    /// Numbers the demos named without their load back from load `at` of `map`; none of them is
    /// relayed if the newest is of another map.
    pub(super) fn number_back(&mut self, (at, map): (usize, Option<String>)) {
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
        let mut inner = demos_on_disk(dir, id);
        let newest = inner.demos.iter().filter_map(|d| d.back).max().unwrap_or(0);
        inner.number_back(inner.deployed_at(newest, None));
        inner.loads = inner.demos.last().map_or_else(Loads::default, |d| Loads::resumed(d.load, Some(d.map.clone()), Some(d.first_tick)));
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
    pub(crate) fn note_line(&self, line: &stvtf::Line) -> Option<usize> {
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

    /// Something was said from outside the match: kept beside the demo being recorded.
    pub(crate) fn append_voice(&self, said: &Spoken) {
        let inner = lock(&self.inner);
        let Some(demo) = inner.demos.last() else { return };
        let written = serde_json::to_string(said)
            .map_err(std::io::Error::other)
            .and_then(|line| writeln!(File::options().create(true).append(true).open(voice_of(&demo.path))?, "{line}"));
        if let Err(e) = written {
            fault!("keeping voice for a demo: {e}");
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
