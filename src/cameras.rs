//! Cameras casters place on maps, for a room's relay to take: where each is, what it looks at
//! and the move it makes if it has one, by map. Kept in the store (`cameras.json`).
//!
//! A map with no cameras shows those of the same map under another version's name
//! (`cp_gullywash_f9` for `cp_gullywash_final1`), from the version changed last. The first change
//! made on that map copies them to its own name.
//!
//! One stv.tf (the home, as in `voice::who`) holds the cameras for all regions. The others pass
//! each change on to it and ask it what changed, with the voice bot's key:
//!
//! - `GET /cameras?since=<version>`: all of them, unless they are still at that version.
//! - `POST /cameras/<map>`: a change, as a streamer's page posts it.
//!
//! A room also keeps cameras of its own (`Room::change_cameras`), placed by its admins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::app::is_name;
use crate::maps::cache::write_atomic;
use crate::report::fault;
use crate::sync::lock;

const FILE: &str = "cameras.json";
const ASK_EVERY: Duration = Duration::from_secs(30);
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_NAME: usize = 24;
const MAX_ON_MAP: usize = 32;
const MAX_MAPS: usize = 5000;
/// Coordinates stay within this: the game cannot send an entity's place further out.
const MAX_COORD: i32 = 16383;
/// The longest path (seconds), the most moments in one, and the furthest an arc turns (radians).
const MAX_PATH_SECS: f32 = 120.0;
const MAX_STEPS: usize = 2000;
const MAX_TURN: f32 = 8.0 * std::f32::consts::PI;
/// The id of the last path set off.
static LAST_PATH: AtomicU64 = AtomicU64::new(0);
/// The most moments a map's flights have together: they go to every page.
const MAX_ROWS_ON_MAP: usize = 6000;
/// The most across all maps: a room sends all of its flights to its pages at once, and the site
/// sends all of its flights to every region.
const MAX_ROWS: usize = 60_000;
/// The most of the home's answer that is read: enough for all the cameras it may keep.
const MAX_ANSWER: u64 = 64 << 20;

/// Where a camera is and the spot it looks at.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spot {
    pub(crate) pos: [i32; 3],
    pub(crate) aim: [i32; 3],
}

impl Spot {
    pub(crate) fn fits(&self) -> bool {
        self.pos.iter().chain(&self.aim).all(|v| v.abs() <= MAX_COORD) && self.pos != self.aim
    }
}

/// Where a relay's camera is: at a spot, and on from there along a path if set off on one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shot {
    pub(crate) spot: Spot,
    pub(crate) path: Arc<stvtf::relay::Path>,
}

/// A moment of a path as a page gives it: seconds in, where the camera is, what it looks at.
pub(crate) type Row = [f32; 7];

impl Shot {
    /// At `spot` from now, and along `rows` if given.
    pub(crate) fn new(spot: Spot, rows: Option<&[Row]>) -> Result<Shot> {
        let still =
            [0.0, spot.pos[0] as f32, spot.pos[1] as f32, spot.pos[2] as f32, spot.aim[0] as f32, spot.aim[1] as f32, spot.aim[2] as f32];
        let rows = rows.unwrap_or(std::slice::from_ref(&still));
        if !spot.fits() || rows.is_empty() || !rows_fit(rows) {
            bail!("Invalid camera.");
        }
        let steps = rows.iter().map(|r| stvtf::relay::Step { secs: r[0], pos: [r[1], r[2], r[3]], aim: [r[4], r[5], r[6]] });
        // The id is the time in milliseconds, raised if needed so no two paths share one.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
        let next = |last: u64| last.max(now.saturating_sub(1)) + 1;
        let id = LAST_PATH.try_update(Ordering::Relaxed, Ordering::Relaxed, |last| Some(next(last))).map_or(now, next);
        Ok(Shot { spot, path: Arc::new(stvtf::relay::Path { id, began: std::time::Instant::now(), steps: steps.collect() }) })
    }

    /// As a relay's camera.
    pub(crate) fn for_tf2(&self) -> stvtf::relay::Camera {
        stvtf::relay::Camera::Along(self.path.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(crate) struct Camera {
    id: u32,
    name: String,
    #[serde(flatten)]
    spot: Spot,
    /// The move it makes from its spot, if it has one.
    #[serde(default, rename = "move", skip_serializing_if = "Option::is_none")]
    going: Option<Move>,
}

/// A camera's move as a page composed it; the pages work out its moments.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Move {
    end: Spot,
    secs: f32,
    way: Way,
    /// An arc's turn round the aim, in radians.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn: Option<f32>,
    /// A flight's moments, which took `took` seconds to fly, evened out over `even`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<Vec<Row>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    took: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    even: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Way {
    Straight,
    Arc,
}

impl Move {
    fn fits(&self) -> bool {
        let within = |v: Option<f32>, most: f32| v.is_none_or(|v| v.is_finite() && v.abs() <= most);
        self.end.fits()
            && self.secs > 0.0
            && self.secs <= MAX_PATH_SECS
            && within(self.turn, MAX_TURN)
            && within(self.took, MAX_PATH_SECS)
            && within(self.even, MAX_PATH_SECS)
            && self.path.as_ref().is_none_or(|rows| !rows.is_empty() && rows_fit(rows))
    }
}

/// Whether a path's moments are in order, within its time and on the map.
fn rows_fit(rows: &[Row]) -> bool {
    let fits = |r: &Row| r.iter().all(|v| v.is_finite()) && r[1..].iter().all(|v| v.abs() <= MAX_COORD as f32) && r[1..4] != r[4..];
    rows.len() <= MAX_STEPS
        && rows.first().is_none_or(|r| r[0] >= 0.0)
        && rows.last().is_none_or(|r| r[0] <= MAX_PATH_SECS)
        && rows.windows(2).all(|w| w[0][0] <= w[1][0])
        && rows.iter().all(fits)
}

/// A change a streamer makes to a map's cameras: one new (no `id`), placed or named anew, or
/// gone (`remove`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Change {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u32>,
    #[serde(default)]
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pos: Option<[i32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aim: Option<[i32; 3]>,
    /// The move to give it; `still` takes its move away.
    #[serde(default, rename = "move", skip_serializing_if = "Option::is_none")]
    going: Option<Move>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    still: bool,
    #[serde(default)]
    remove: bool,
}

impl Change {
    fn spot(&self) -> Option<Spot> {
        Some(Spot { pos: self.pos?, aim: self.aim? })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
struct OnMap {
    /// The version it last changed at.
    changed: u64,
    cameras: Vec<Camera>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub(crate) struct Kept {
    version: u64,
    /// The next camera's id.
    next: u32,
    maps: BTreeMap<String, OnMap>,
}

/// A map's cameras, and the map they are kept under.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Found<'a> {
    map: &'a str,
    cameras: &'a [Camera],
}

type Since = Box<dyn Fn(u64) -> Result<Option<Kept>> + Send + Sync>;
type Told = Box<dyn Fn(&str, &Change) -> Result<()> + Send + Sync>;

/// The home: asked what changed since a version, and told a change to a map.
struct Home {
    since: Since,
    change: Told,
}

#[derive(Default)]
struct Inner {
    file: Option<PathBuf>,
    home: Option<Home>,
    kept: Mutex<Kept>,
}

#[derive(Clone, Default)]
pub(crate) struct Cameras(Arc<Inner>);

/// A map's name without its version: `cp_gullywash_f9` and `cp_gullywash_final1` are
/// `cp_gullywash`, `cp_granary_pro_rc8` is `cp_granary_pro`.
fn stem(map: &str) -> &str {
    let is_version = |part: &str| {
        let rest = ["final", "fix", "rc", "a", "b", "f", "v"].iter().find_map(|p| part.strip_prefix(p)).unwrap_or(part);
        let letters = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        rest != part && rest.is_empty()
            || rest.len() > letters.len() && letters.len() <= 1 && letters.bytes().all(|b| b.is_ascii_lowercase())
    };
    let mut stem = map;
    while let Some((before, last)) = stem.rsplit_once('_')
        && is_version(last)
        && before.contains('_')
    {
        stem = before;
    }
    stem
}

impl Kept {
    /// The cameras for `map`: its own, else those of the same map by another name.
    fn on(&self, map: &str) -> Option<Found<'_>> {
        let own = self.maps.get_key_value(map).filter(|(_, m)| !m.cameras.is_empty());
        let alike = || self.maps.iter().filter(|(name, m)| stem(name) == stem(map) && !m.cameras.is_empty()).max_by_key(|(_, m)| m.changed);
        let (name, on) = own.or_else(alike)?;
        Some(Found { map: name, cameras: &on.cameras })
    }

    /// Each map's own, by its name.
    pub(crate) fn by_map(&self) -> BTreeMap<&str, &[Camera]> {
        self.maps.iter().map(|(map, on)| (map.as_str(), on.cameras.as_slice())).collect()
    }

    pub(crate) fn change(&mut self, map: &str, change: &Change) -> Result<()> {
        if !is_name(map, b"_") {
            bail!("Invalid map.");
        }
        let name: String = change.name.chars().filter(|c| !c.is_control()).collect();
        let name = name.trim();
        if name.chars().count() > MAX_NAME {
            bail!("At most {MAX_NAME} characters.");
        }
        if change.spot().is_some_and(|s| !s.fits()) || change.going.as_ref().is_some_and(|m| !m.fits()) {
            bail!("Invalid camera.");
        }
        if !self.maps.contains_key(map) {
            if self.maps.len() >= MAX_MAPS {
                bail!("Too many maps.");
            }
            let from = self.on(map).map_or_else(Vec::new, |f| f.cameras.to_vec());
            self.maps.insert(map.to_string(), OnMap { changed: 0, cameras: from });
        }
        let Some(on) = self.maps.get(map) else { bail!("Invalid map.") };
        let rows = |m: Option<&Move>| m.and_then(|m| m.path.as_ref()).map_or(0, Vec::len);
        let others: usize = on.cameras.iter().filter(|c| Some(c.id) != change.id).map(|c| rows(c.going.as_ref())).sum();
        if others + rows(change.going.as_ref()) > MAX_ROWS_ON_MAP {
            bail!("Too many flights on this map.");
        }
        let elsewhere: usize =
            self.maps.iter().filter(|(name, _)| *name != map).flat_map(|(_, m)| &m.cameras).map(|c| rows(c.going.as_ref())).sum();
        if elsewhere + others + rows(change.going.as_ref()) > MAX_ROWS {
            bail!("Too many flights.");
        }
        self.version += 1;
        let Some(on) = self.maps.get_mut(map) else { bail!("Invalid map.") };
        on.changed = self.version;
        match (change.id, change.remove) {
            (Some(id), true) => on.cameras.retain(|c| c.id != id),
            (Some(id), false) => {
                let Some(c) = on.cameras.iter_mut().find(|c| c.id == id) else { bail!("No such camera.") };
                if !name.is_empty() {
                    c.name = name.to_string();
                }
                c.spot = change.spot().unwrap_or(c.spot);
                if change.still || change.going.is_some() {
                    c.going.clone_from(&change.going);
                }
            }
            (None, _) => {
                let Some(spot) = change.spot() else { bail!("Invalid camera.") };
                if on.cameras.len() >= MAX_ON_MAP {
                    bail!("At most {MAX_ON_MAP} cameras.");
                }
                self.next += 1;
                let name = if name.is_empty() { format!("Camera {}", on.cameras.len() + 1) } else { name.to_string() };
                on.cameras.push(Camera { id: self.next, name, spot, going: change.going.clone() });
            }
        }
        Ok(())
    }
}

impl Cameras {
    /// The cameras kept in `dir`. With a `home` (its address and the bot's key), they follow the
    /// cameras kept there.
    pub(crate) fn new(dir: Option<&Path>, home: Option<(&str, &str)>) -> Self {
        Self::asking(dir, home.map(|(url, key)| over_http(url, key)))
    }

    fn asking(dir: Option<&Path>, home: Option<Home>) -> Self {
        let file = dir.map(|d| d.join(FILE));
        let kept = file.as_ref().and_then(|f| serde_json::from_slice(&std::fs::read(f).ok()?).ok()).unwrap_or_default();
        Cameras(Arc::new(Inner { file, home, kept: Mutex::new(kept) }))
    }

    /// Keeps them the same as the home's, from a thread of its own.
    pub(crate) fn keep_up(&self) {
        if self.0.home.is_none() {
            return;
        }
        let this = self.clone();
        let run = move || {
            loop {
                if let Err(e) = this.sync() {
                    tracing::info!("{e:#}");
                }
                std::thread::sleep(ASK_EVERY);
            }
        };
        if let Err(e) = std::thread::Builder::new().name("cameras".into()).spawn(run) {
            fault("cannot start the cameras thread", &format!("cannot start the cameras thread: {e}"));
        }
    }

    /// `{"map", "cameras"}` for `map`, as a page takes them.
    pub(crate) fn on(&self, map: &str) -> serde_json::Value {
        let kept = lock(&self.0.kept);
        serde_json::to_value(kept.on(map).unwrap_or(Found { map, cameras: &[] })).unwrap_or_default()
    }

    /// A streamer's change to the cameras of `map`: made at the home, if there is one.
    pub(crate) fn change(&self, map: &str, change: &Change) -> Result<()> {
        if let Some(home) = &self.0.home {
            // Checked on a copy here first: the page gets this error, not the one from the home.
            lock(&self.0.kept).clone().change(map, change)?;
            (home.change)(map, change).map_err(|e| {
                tracing::info!("{e:#}");
                anyhow::anyhow!("Not saved.")
            })?;
            return self.sync();
        }
        let mut kept = lock(&self.0.kept);
        kept.change(map, change)?;
        self.save(&kept);
        Ok(())
    }

    /// All of them, unless they are still at `version`.
    pub(crate) fn since(&self, version: u64) -> Option<Kept> {
        Some(lock(&self.0.kept).clone()).filter(|k| k.version != version)
    }

    fn sync(&self) -> Result<()> {
        let Some(home) = &self.0.home else { return Ok(()) };
        let had = lock(&self.0.kept).version;
        if let Some(theirs) = (home.since)(had)? {
            let mut kept = lock(&self.0.kept);
            *kept = theirs;
            self.save(&kept);
        }
        Ok(())
    }

    fn save(&self, kept: &Kept) {
        let Some(file) = &self.0.file else { return };
        let written = serde_json::to_vec(kept).map_err(anyhow::Error::new).and_then(|body| Ok(write_atomic(file, &body)?));
        if let Err(e) = written {
            fault("cannot keep the cameras", &format!("cannot keep the cameras: {e:#}"));
        }
    }
}

/// The stv.tf at `url`, asked with the bot's `key`.
fn over_http(url: &str, key: &str) -> Home {
    let (url, bearer) = (url.trim_end_matches('/').to_string(), format!("Bearer {key}"));
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build();
    let (get, to, auth) = (agent.clone(), url.clone(), bearer.clone());
    Home {
        since: Box::new(move |version| {
            let answer = get.get(&format!("{to}/cameras?since={version}")).set("Authorization", &auth).call();
            let answer = answer.context("cannot ask for the cameras")?;
            if answer.status() == 204 {
                return Ok(None);
            }
            // Not as a string: that is cut off at 10 MB.
            Ok(Some(serde_json::from_reader(std::io::Read::take(answer.into_reader(), MAX_ANSWER))?))
        }),
        change: Box::new(move |map, change| {
            let ask = agent.post(&format!("{url}/cameras/{map}")).set("Authorization", &bearer).set("Content-Type", "application/json");
            ask.send_string(&serde_json::to_string(change)?).context("cannot pass a camera on")?;
            Ok(())
        }),
    }
}

#[cfg(test)]
mod tests;
