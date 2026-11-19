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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

    /// As a relay's camera: its pitch (down positive) and yaw in degrees.
    pub(crate) fn for_tf2(&self) -> stvtf::relay::Camera {
        let d: [f64; 3] = std::array::from_fn(|a| f64::from(self.aim[a] - self.pos[a]));
        let yaw = d[1].atan2(d[0]).to_degrees().round();
        let pitch = (-d[2]).atan2(d[0].hypot(d[1])).to_degrees().round();
        stvtf::relay::Camera::Fixed { pos: self.pos, pitch: pitch as i16, yaw: yaw as i16 }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(crate) struct Camera {
    id: u32,
    name: String,
    #[serde(flatten)]
    spot: Spot,
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

    fn change(&mut self, map: &str, change: &Change) -> Result<()> {
        if !is_name(map, b"_") {
            bail!("Invalid map.");
        }
        let name: String = change.name.chars().filter(|c| !c.is_control()).collect();
        let name = name.trim();
        if name.chars().count() > MAX_NAME {
            bail!("At most {MAX_NAME} characters.");
        }
        if change.spot().is_some_and(|s| !s.fits()) {
            bail!("Invalid camera.");
        }
        if !self.maps.contains_key(map) {
            if self.maps.len() >= MAX_MAPS {
                bail!("Too many maps.");
            }
            let from = self.on(map).map_or_else(Vec::new, |f| f.cameras.to_vec());
            self.maps.insert(map.to_string(), OnMap { changed: 0, cameras: from });
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
            }
            (None, _) => {
                let Some(spot) = change.spot() else { bail!("Invalid camera.") };
                if on.cameras.len() >= MAX_ON_MAP {
                    bail!("At most {MAX_ON_MAP} cameras.");
                }
                self.next += 1;
                let name = if name.is_empty() { format!("Camera {}", on.cameras.len() + 1) } else { name.to_string() };
                on.cameras.push(Camera { id: self.next, name, spot });
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
        Change { id: None, name: name.into(), pos: Some([x, 0, 1500]), aim: Some([0, 0, 400]), remove: false }
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
        c.change("cp_gullywash_final1", &Change { id: Some(mid), name: "Middle".into(), pos: None, aim: None, remove: false }).unwrap();
        c.change("cp_gullywash_final1", &Change { id: Some(mid + 1), name: String::new(), pos: None, aim: None, remove: true }).unwrap();
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
        assert!(c.change("cp_a", &Change { id: Some(4), name: "a".into(), pos: None, aim: None, remove: false }).is_err());
        assert!(c.change("cp_a", &Change { id: None, name: "a".into(), pos: None, aim: None, remove: false }).is_err());
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
    fn a_spot_is_a_relays_camera_looking_at_its_aim() {
        let down = Spot { pos: [0, 0, 1500], aim: [0, 0, 400] };
        assert_eq!(down.for_tf2(), stvtf::relay::Camera::Fixed { pos: [0, 0, 1500], pitch: 90, yaw: 0 });
        let angled = Spot { pos: [1000, 0, 1400], aim: [0, 1000, 400] };
        assert_eq!(angled.for_tf2(), stvtf::relay::Camera::Fixed { pos: [1000, 0, 1400], pitch: 35, yaw: 135 });
        assert!(!Spot { pos: [1, 1, 1], aim: [1, 1, 1] }.fits());
    }
}
