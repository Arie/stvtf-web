//! Cameras casters place on maps, for a room's relay to take: where each is and what it looks at,
//! by map. Kept in the store (`cameras.json`).
//!
//! A map with none has those of the same map under another version's name (`cp_gullywash_f9` for
//! `cp_gullywash_final1`), the latest changed; the first change made there keeps them under its
//! own name.
//!
//! One stv.tf holds them for all (the home, as `voice::who`'s). The others pass each change on
//! to it and ask it what changed, with the voice bot's key:
//!
//! - `GET /cameras?since=<version>`: all of them, when changed since.
//! - `POST /cameras/<map>`: a change, as a streamer's page posts it.
//!
//! A room keeps its own besides (`Room::change_cameras`), placed by its admins.

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
/// A map's coordinates stay within this.
const MAX_COORD: i32 = 32768;
/// The longest path (seconds), the most moments in one, and the furthest an arc turns (radians).
const MAX_PATH_SECS: f32 = 120.0;
const MAX_STEPS: usize = 2000;
const MAX_TURN: f32 = 8.0 * std::f32::consts::PI;
/// The id of the last path set off.
static LAST_PATH: AtomicU64 = AtomicU64::new(0);
/// The most moments a map's flights have together: they go to every page.
const MAX_ROWS_ON_MAP: usize = 6000;

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
        // The time, and never one twice: each sets off anew.
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
    /// Where it goes from its spot, if set off.
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
    /// Where it goes from its spot; or no more (`still`).
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
    /// Those kept in `dir`; with a `home` (its address and the bot's key), the same as there.
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

    /// A streamer's change to `map`'s: made at the home, if there is one.
    pub(crate) fn change(&self, map: &str, change: &Change) -> Result<()> {
        if let Some(home) = &self.0.home {
            // Checked here first: what the home says of it is not for a page.
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

    /// All of them, when changed since `version`.
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
            Ok(Some(serde_json::from_str(&answer.into_string()?)?))
        }),
        change: Box::new(move |map, change| {
            let ask = agent.post(&format!("{url}/cameras/{map}")).set("Authorization", &bearer).set("Content-Type", "application/json");
            ask.send_string(&serde_json::to_string(change)?).context("cannot pass a camera on")?;
            Ok(())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(name: &str, x: i32) -> Change {
        Change { id: None, name: name.into(), pos: Some([x, 0, 1500]), aim: Some([0, 0, 400]), going: None, still: false, remove: false }
    }

    fn names(c: &Cameras, map: &str) -> (String, Vec<String>) {
        let on = c.on(map);
        let names = on["cameras"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect();
        (on["map"].as_str().unwrap().to_string(), names)
    }

    #[test]
    fn a_maps_version_is_not_part_of_its_stem() {
        for (map, is) in [
            ("cp_gullywash_f9", "cp_gullywash"),
            ("cp_gullywash_final1", "cp_gullywash"),
            ("koth_product_final", "koth_product"),
            ("cp_granary_pro_rc8", "cp_granary_pro"),
            ("koth_clearcut_b15d", "koth_clearcut"),
            ("cp_sunshine", "cp_sunshine"),
            ("pl_upward_f12", "pl_upward"),
            ("koth_bagel_rc10", "koth_bagel"),
            ("cp_process_f12_fix", "cp_process"),
            ("ctf_2fort", "ctf_2fort"),
            ("cp_5gorge", "cp_5gorge"),
            ("koth_b", "koth_b"),
        ] {
            assert_eq!(stem(map), is, "{map}");
        }
    }

    #[test]
    fn cameras_are_kept_by_map_and_a_new_version_of_a_map_starts_with_the_last_ones() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cameras::new(Some(dir.path()), None);
        c.change("cp_gullywash_f9", &placed("Mid", 100)).unwrap();
        c.change("cp_gullywash_f9", &placed("", 200)).unwrap();
        c.change("cp_process_f12", &placed("Other", 1)).unwrap();
        assert_eq!(names(&c, "cp_gullywash_f9"), ("cp_gullywash_f9".into(), vec!["Mid".into(), "Camera 2".into()]));
        assert_eq!(names(&c, "cp_gullywash_final1").0, "cp_gullywash_f9", "the same map by another name");
        assert_eq!(names(&c, "cp_snakewater_final1"), ("cp_snakewater_final1".into(), vec![]));
        // Changed there: its own from then on, the older version's as they were.
        let mid = c.on("cp_gullywash_final1")["cameras"][0]["id"].as_u64().unwrap() as u32;
        c.change(
            "cp_gullywash_final1",
            &Change { id: Some(mid), name: "Middle".into(), pos: None, aim: None, going: None, still: false, remove: false },
        )
        .unwrap();
        c.change(
            "cp_gullywash_final1",
            &Change { id: Some(mid + 1), name: String::new(), pos: None, aim: None, going: None, still: false, remove: true },
        )
        .unwrap();
        assert_eq!(names(&c, "cp_gullywash_final1"), ("cp_gullywash_final1".into(), vec!["Middle".into()]));
        assert_eq!(names(&c, "cp_gullywash_f9").1, ["Mid", "Camera 2"]);
        assert_eq!(names(&c, "cp_gullywash_f10").0, "cp_gullywash_final1", "the latest changed");
        assert_eq!(names(&Cameras::new(Some(dir.path()), None), "cp_gullywash_final1").1, ["Middle"], "read back");
    }

    #[test]
    fn a_camera_off_the_map_or_named_too_long_is_refused() {
        let c = Cameras::default();
        assert!(c.change("../etc", &placed("a", 1)).is_err());
        assert!(c.change("cp_a", &placed(&"n".repeat(MAX_NAME + 1), 1)).is_err());
        assert!(c.change("cp_a", &placed("a", 99_999)).is_err());
        assert!(
            c.change("cp_a", &Change { id: Some(4), name: "a".into(), pos: None, aim: None, going: None, still: false, remove: false })
                .is_err()
        );
        assert!(
            c.change("cp_a", &Change { id: None, name: "a".into(), pos: None, aim: None, going: None, still: false, remove: false })
                .is_err()
        );
        for n in 0..MAX_ON_MAP {
            c.change("cp_b", &placed("", n as i32)).unwrap();
        }
        assert!(c.change("cp_b", &placed("", 1)).is_err());
    }

    #[test]
    fn a_region_passes_changes_on_to_the_home_and_has_what_the_home_has() {
        let home = Cameras::default();
        let (h1, h2) = (home.clone(), home.clone());
        let region = Cameras::asking(
            None,
            Some(Home { since: Box::new(move |v| Ok(h1.since(v))), change: Box::new(move |map, change| h2.change(map, change)) }),
        );
        home.change("cp_a", &placed("Theirs", 1)).unwrap();
        region.change("cp_a", &placed("Ours", 2)).unwrap();
        assert_eq!(names(&home, "cp_a").1, ["Theirs", "Ours"]);
        assert_eq!(names(&region, "cp_a").1, ["Theirs", "Ours"]);
        assert!(region.change("cp_a", &placed("a", 99_999)).is_err());
        home.change("cp_a", &placed("Later", 3)).unwrap();
        region.sync().unwrap();
        assert_eq!(names(&region, "cp_a").1.len(), 3);
        assert_eq!(home.since(lock(&region.0.kept).version), None);
    }

    #[test]
    fn a_camera_keeps_the_move_it_was_given_until_told_it_stays() {
        let mut k = Kept::default();
        let change = |text: &str| serde_json::from_str::<Change>(text).unwrap();
        let arc = r#"{"end":{"pos":[0,900,900],"aim":[0,0,300]},"secs":8,"way":"arc","turn":1.5}"#;
        k.change("cp_a", &change(&format!(r#"{{"name":"Orbit","pos":[900,0,900],"aim":[0,0,300],"move":{arc}}}"#))).unwrap();
        let said = |k: &Kept| serde_json::to_value(k.by_map()["cp_a"]).unwrap()[0]["move"].clone();
        assert_eq!(said(&k), serde_json::json!({"end":{"pos":[0,900,900],"aim":[0,0,300]},"secs":8.0,"way":"arc","turn":1.5}));
        // Placed anew or named anew, it keeps it.
        k.change("cp_a", &change(r#"{"id":1,"name":"Round","pos":[800,0,900],"aim":[0,0,300]}"#)).unwrap();
        assert_eq!(said(&k)["way"], "arc");
        // A flight's moments, each the seconds in, where it is and what it looks at.
        let flight = r#"{"end":{"pos":[0,900,900],"aim":[0,0,300]},"secs":2,"way":"straight","took":2,"even":0.25,
            "path":[[0,900,0,900,0,0,300],[2,0,900,900,0,0,300]]}"#;
        k.change("cp_a", &change(&format!(r#"{{"id":1,"move":{flight}}}"#))).unwrap();
        assert_eq!(said(&k)["path"][1][2], 900.0);
        for bad in [r#""secs":0"#, r#""secs":500"#, r#""turn":99"#, r#""path":[]"#, r#""path":[[1,0,0,1,0,0,0],[0,0,0,1,0,0,0]]"#] {
            let odd = format!(
                r#"{{"id":1,"move":{{"end":{{"pos":[0,900,900],"aim":[0,0,300]}},"way":"arc",{}{bad}}}}}"#,
                if bad.contains("secs") { "" } else { r#""secs":2,"# }
            );
            assert!(k.change("cp_a", &change(&odd)).is_err(), "{bad}");
        }
        k.change("cp_a", &change(r#"{"id":1,"still":true}"#)).unwrap();
        assert!(said(&k).is_null());
        // The flights of a map's cameras go to every page: no more of them than that takes.
        let long: Vec<String> = (0..1800).map(|n| format!("[{},900,0,900,0,0,300]", f64::from(n) / 30.0)).collect();
        let flown =
            format!(r#""move":{{"end":{{"pos":[0,900,900],"aim":[0,0,300]}},"secs":60,"way":"straight","path":[{}]}}"#, long.join(","));
        for n in 0..3 {
            k.change("cp_a", &change(&format!(r#"{{"name":"f{n}","pos":[900,0,900],"aim":[0,0,300],{flown}}}"#))).unwrap();
        }
        let more = k.change("cp_a", &change(&format!(r#"{{"name":"f3","pos":[900,0,900],"aim":[0,0,300],{flown}}}"#)));
        assert_eq!(more.unwrap_err().to_string(), "Too many flights on this map.");
        // One flown again is counted once.
        k.change("cp_a", &change(&format!(r#"{{"id":2,{flown}}}"#))).unwrap();
    }

    #[test]
    fn a_spot_is_a_relays_camera_that_stays_or_goes_along_a_path_given() {
        let spot = Spot { pos: [1000, 0, 1400], aim: [0, 1000, 400] };
        let stvtf::relay::Camera::Along(still) = Shot::new(spot, None).unwrap().for_tf2() else { panic!() };
        let at = stvtf::relay::Step { secs: 0.0, pos: [1000.0, 0.0, 1400.0], aim: [0.0, 1000.0, 400.0] };
        assert_eq!(still.steps, [at]);
        let rows = [[0.0, 1000.0, 0.0, 1400.0, 0.0, 1000.0, 400.0], [2.5, 900.5, 0.0, 1400.0, 0.0, 1000.0, 400.0]];
        let moving = Shot::new(spot, Some(&rows)).unwrap();
        assert_eq!((moving.path.steps.len(), moving.path.steps[1].secs, moving.path.steps[1].pos[0]), (2, 2.5, 900.5));
        assert_ne!(moving.path.id, 0);
        // Each sets off anew, two at one moment too.
        let ids: Vec<u64> = (0..50).map(|_| Shot::new(spot, None).unwrap().path.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]) && ids[0] > moving.path.id);
        assert!(!Spot { pos: [1, 1, 1], aim: [1, 1, 1] }.fits());
        // Out of order, too long, off the map, looking at itself, not a number, none.
        let bad: [&[Row]; 6] = [
            &[rows[1], rows[0]],
            &[rows[0], [121.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]],
            &[[0.0, 99_999.0, 0.0, 0.0, 1.0, 0.0, 0.0]],
            &[[0.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0]],
            &[[f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]],
            &[],
        ];
        for rows in bad {
            assert!(Shot::new(spot, Some(rows)).is_err(), "{rows:?}");
        }
    }
}
