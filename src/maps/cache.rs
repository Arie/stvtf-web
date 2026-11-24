//! Map geometry built from fastdl's BSPs, and the maps' overviews, kept on disk.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::de::IgnoredAny;

use crate::app::is_name;
use crate::maps::bsp;

const WARM: &str = include_str!("../../maps.txt");
const FASTDL: &str = "https://fastdl.serveme.tf/maps/";
const MAX_BSP: u64 = 512 << 20;
const DEMOSTF: &str = "https://maps.demos.tf/";
const MAX_OVERVIEW: u64 = 8 << 20;
const DAY: Duration = Duration::from_hours(24);

/// The most played maps, built when the server starts.
pub(crate) fn warm_maps() -> impl Iterator<Item = &'static str> {
    WARM.lines().filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// Builds a map that is not in the cache yet only when `fetch`: otherwise anyone could have every
/// map on fastdl downloaded and kept here.
pub(crate) fn map_geometry(cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    map_geometry_from(FASTDL, cache, name, fetch)
}

fn map_geometry_from(fastdl: &str, cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    if !is_name(name, b"_") {
        bail!("cannot use that map name");
    }
    std::fs::create_dir_all(cache)?;
    let json = geometry_path(cache, name);
    if let Ok(body) = std::fs::read(&json) {
        return Ok(body);
    }
    if !fetch {
        bail!("cannot find a match on {name}");
    }
    let body = built_from_bsp(fastdl, cache, name, |data| Ok(serde_json::to_vec(&bsp::geometry(data)?)?))?;
    write_atomic(&json, &body)?;
    Ok(body)
}

/// What `build` makes of the map's BSP, read from the cache or else downloaded from fastdl. The
/// BSP is deleted afterwards: only a version bump in a cached file's name needs it again, and the
/// most played maps add up to GBs.
fn built_from_bsp(fastdl: &str, cache: &Path, name: &str, build: impl Fn(&[u8]) -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    let bsp_path = cache.join(format!("{name}.bsp"));
    let data = if let Ok(d) = std::fs::read(&bsp_path) {
        d
    } else {
        let mut d = Vec::new();
        // One byte over the limit tells a map that is too big from one that just fits.
        ureq::get(&format!("{fastdl}{name}.bsp")).timeout(MAP_WITHIN).call()?.into_reader().take(MAX_BSP + 1).read_to_end(&mut d)?;
        if d.len() as u64 > MAX_BSP {
            bail!("cannot take {name}.bsp: over {} MiB", MAX_BSP >> 20);
        }
        write_atomic(&bsp_path, &d)?;
        d
    };
    // The BSP is removed even when the build fails: a broken download would otherwise fail every
    // request for this map. The next request fetches it again.
    let made = build(&data);
    let _ = std::fs::remove_file(&bsp_path);
    made
}

/// What blocks a camera's sight on a map (`bsp::sight`), gzipped: built like its geometry, when
/// first asked for.
pub(crate) fn map_sight(cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    map_sight_from(FASTDL, cache, name, fetch)
}

fn map_sight_from(fastdl: &str, cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    if !is_name(name, b"_") {
        bail!("cannot use that map name");
    }
    let path = cache.join(format!("{name}.sight.v1.gz"));
    if let Ok(body) = std::fs::read(&path) {
        return Ok(body);
    }
    if !fetch {
        bail!("cannot find a match on {name}");
    }
    std::fs::create_dir_all(cache)?;
    let body = built_from_bsp(fastdl, cache, name, |data| {
        let mut gz = GzEncoder::new(Vec::new(), Compression::best());
        gz.write_all(&bsp::sight(data)?)?;
        Ok(gz.finish()?)
    })?;
    write_atomic(&path, &body)?;
    Ok(body)
}

/// `map_geometry` gzipped, kept next to the JSON so a map is compressed once, not on every load.
pub(crate) fn map_geometry_gzip(cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    if !is_name(name, b"_") {
        bail!("cannot use that map name");
    }
    let mut path = geometry_path(cache, name).into_os_string();
    path.push(".gz");
    if let Ok(body) = std::fs::read(&path) {
        return Ok(body);
    }
    let json = map_geometry(cache, name, fetch)?;
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(&json)?;
    let body = gz.finish()?;
    // Without the copy the next load just compresses it again.
    let _ = write_atomic(Path::new(&path), &body);
    Ok(body)
}

/// Where a map's geometry is cached. Bump the version in its name when the extracted data
/// changes, so cached maps are redone.
pub(crate) fn geometry_path(cache: &Path, name: &str) -> PathBuf {
    cache.join(format!("{name}.v10.json"))
}

/// The map overviews. `overviews.json` lists them all, by map: the rendered set the cache has
/// (see `rendered_bounds`) and demos.tf's. `<map>.large.webp` is a rendered image, `<map>.webp`
/// demos.tf's, and `boundaries.json` demos.tf's own list. demos.tf's are kept in the cache so
/// viewers do not each load them from demos.tf's own server. Its list is fetched again after a
/// day; while demos.tf is down, what the cache has is served.
///
/// The rendered ones have addresses of their own because browsers keep these files for a day: a
/// page with yesterday's list must not get a new image under an old name, to place by old bounds.
pub(crate) fn overview(cache: &Path, file: &str) -> Result<Vec<u8>> {
    overview_from(DEMOSTF, cache, file)
}

/// How long a map's file and an overview may take to fetch.
const MAP_WITHIN: Duration = Duration::from_mins(2);
const OVERVIEW_WITHIN: Duration = Duration::from_secs(20);

/// What an overview file is.
enum Overview<'a> {
    /// `overviews.json`.
    List,
    /// `<map>.large.webp`.
    Rendered(&'a str),
    /// `boundaries.json` and `<map>.webp`.
    Demostf,
}

fn overview_file(file: &str) -> Result<Overview<'_>> {
    match file {
        "overviews.json" => Ok(Overview::List),
        _ => match file.strip_suffix(".large.webp") {
            Some(map) if is_name(map, b"_") => Ok(Overview::Rendered(map)),
            Some(_) => bail!("cannot find that overview"),
            None => overview_map(file).map(|_| Overview::Demostf),
        },
    }
}

/// The map that demos.tf's overview image `file` is of, None for its list; an error for any other file.
fn overview_map(file: &str) -> Result<Option<&str>> {
    match file.strip_suffix(".webp") {
        _ if file == "boundaries.json" => Ok(None),
        Some(map) if is_name(map, b"_") => Ok(Some(map)),
        _ => bail!("cannot find that overview"),
    }
}

/// `file` from the cache, if it is there and need not be fetched again.
pub(crate) fn overview_kept(cache: &Path, file: &str) -> Option<Vec<u8>> {
    match overview_file(file).ok()? {
        Overview::List => demostf_kept(cache, "boundaries.json").map(|list| with_rendered(&list, &rendered_bounds(cache))),
        Overview::Rendered(map) => rendered_image(cache, map),
        Overview::Demostf => demostf_kept(cache, file),
    }
}

fn overview_from(demostf: &str, cache: &Path, file: &str) -> Result<Vec<u8>> {
    match overview_file(file)? {
        Overview::List => {
            let rendered = rendered_bounds(cache);
            match demostf_from(demostf, cache, "boundaries.json") {
                Ok(list) => Ok(with_rendered(&list, &rendered)),
                // The rendered set needs nothing from demos.tf.
                Err(_) if !rendered.is_empty() => Ok(with_rendered(b"{}", &rendered)),
                Err(e) => Err(e),
            }
        }
        Overview::Rendered(map) => rendered_image(cache, map).ok_or_else(|| anyhow::anyhow!("cannot find that overview")),
        Overview::Demostf => demostf_from(demostf, cache, file),
    }
}

/// The rendered overviews (maps.tf2.lol's bundles, unpacked into `rendered` in the cache:
/// `large/<map>.webp` and `json/<map>.json`): each map that has both, with the world box its
/// image covers, x and y of the lowest corner and then of the highest.
fn rendered_bounds(cache: &Path) -> BTreeMap<String, [f64; 4]> {
    #[derive(serde::Deserialize)]
    struct Info {
        world: World,
    }
    #[derive(serde::Deserialize)]
    struct World {
        min: [f64; 2],
        max: [f64; 2],
    }
    let dir = cache.join("rendered");
    let Ok(infos) = std::fs::read_dir(dir.join("json")) else { return BTreeMap::new() };
    infos
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let map = path.file_name()?.to_str()?.strip_suffix(".json")?;
            if !is_name(map, b"_") || !dir.join(format!("large/{map}.webp")).is_file() {
                return None;
            }
            let Info { world } = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            Some((map.to_string(), [world.min[0], world.min[1], world.max[0], world.max[1]]))
        })
        .collect()
}

/// A map's rendered overview, if the cache has it with its world box.
fn rendered_image(cache: &Path, map: &str) -> Option<Vec<u8>> {
    let dir = cache.join("rendered");
    dir.join(format!("json/{map}.json")).is_file().then(|| std::fs::read(dir.join(format!("large/{map}.webp"))).ok())?
}

/// demos.tf's list of overviews with the rendered ones added, marked `rendered`, each instead of
/// demos.tf's of the same name.
fn with_rendered(demostf: &[u8], rendered: &BTreeMap<String, [f64; 4]>) -> Vec<u8> {
    if rendered.is_empty() {
        return demostf.to_vec();
    }
    let mut list: BTreeMap<String, serde_json::Value> = serde_json::from_slice(demostf).unwrap_or_default();
    for (map, [x0, y0, x1, y1]) in rendered {
        let entry = serde_json::json!({"boundary_min": {"x": x0, "y": y0}, "boundary_max": {"x": x1, "y": y1}, "rendered": true});
        list.insert(map.clone(), entry);
    }
    serde_json::to_vec(&list).unwrap_or_else(|_| demostf.to_vec())
}

/// demos.tf's `file` from the cache, if it is there and need not be fetched again.
fn demostf_kept(cache: &Path, file: &str) -> Option<Vec<u8>> {
    let image = overview_map(file).ok()?;
    let path = cache.join("demostf").join(file);
    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?.elapsed().unwrap_or_default();
    (image.is_some() || age < DAY).then(|| std::fs::read(&path).ok())?
}

fn demostf_from(demostf: &str, cache: &Path, file: &str) -> Result<Vec<u8>> {
    let image = overview_map(file)?;
    if let Some(kept) = demostf_kept(cache, file) {
        return Ok(kept);
    }
    let remote = if image.is_some() { format!("images/{file}") } else { file.to_string() };
    let dir = cache.join("demostf");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(file);
    // Only the maps demos.tf has: any other name would be a request to demos.tf every time.
    if image.is_some_and(|map| !demostf_maps_from(demostf, cache).iter().any(|m| m == map)) {
        bail!("cannot find that overview");
    }
    let mut body = Vec::new();
    let fetched = ureq::get(&format!("{demostf}{remote}"))
        .timeout(OVERVIEW_WITHIN)
        .call()
        .map_err(anyhow::Error::from)
        .and_then(|r| Ok(r.into_reader().take(MAX_OVERVIEW + 1).read_to_end(&mut body)?));
    match fetched {
        Ok(n) if n as u64 <= MAX_OVERVIEW => {
            write_atomic(&path, &body)?;
            Ok(body)
        }
        Ok(_) => bail!("cannot take {file}: over {} MiB", MAX_OVERVIEW >> 20),
        Err(e) => std::fs::read(&path).map_err(|_| e),
    }
}

/// The maps demos.tf has an overview of, from its `boundaries.json`.
pub(crate) fn demostf_maps(cache: &Path) -> Vec<String> {
    demostf_maps_from(DEMOSTF, cache)
}

#[expect(clippy::zero_sized_map_values, reason = "only the keys are read; the values are skipped unparsed")]
fn demostf_maps_from(demostf: &str, cache: &Path) -> Vec<String> {
    demostf_from(demostf, cache, "boundaries.json")
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, IgnoredAny>>(&b).ok())
        .map_or_default(|m| m.into_keys().collect())
}

/// Writes through a temporary file and a rename: a cached file that can be read is complete,
/// even after a crash halfway through writing it. Each write has its own temporary file, so two
/// at once (a gzipped map is written without a lock) cannot rename the other's half-written one.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.{}.tmp", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
    let written = std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        // A half-written temporary file would otherwise stay for good.
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maps::bsp::tests::process;
    use crate::sync::lock;
    use crate::testing::{TempDir, web_server};

    #[test]
    fn write_atomic_replaces_the_file() {
        let dir = TempDir::new("atomic");
        let path = geometry_path(&dir, "m");
        std::fs::write(&path, b"old, longer contents").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
    }

    #[test]
    fn a_failed_write_atomic_leaves_no_temporary_file() {
        let dir = TempDir::new("atomicfail");
        // A file cannot be renamed over a directory that has something in it.
        let path = geometry_path(&dir, "m");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("inside"), b"").unwrap();
        assert!(write_atomic(&path, b"new").is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
    }

    #[test]
    fn only_overview_files_are_fetched() {
        let cache = TempDir::new("overview");
        for bad in ["../boundaries.json", "x.json", "a/b.webp", ".webp", "boundaries.webp.json"] {
            assert!(overview(&cache, bad).is_err(), "{bad}");
        }
        // A cached image is served without asking demos.tf.
        std::fs::create_dir_all(cache.join("demostf")).unwrap();
        std::fs::write(cache.join("demostf/cp_test.webp"), b"RIFF").unwrap();
        assert_eq!(overview(&cache, "cp_test.webp").unwrap(), b"RIFF");
        // A map demos.tf does not list is not asked for (a fresh list is not fetched again).
        std::fs::write(cache.join("demostf/boundaries.json"), br#"{"cp_test": {}}"#).unwrap();
        assert_eq!(demostf_maps(&cache), ["cp_test"]);
        assert!(overview(&cache, "cp_nothere.webp").is_err());
    }

    #[test]
    fn only_maps_being_watched_are_built_from_fastdl() {
        let cache = TempDir::new("onmap");
        // Not cached and not being watched: an error, without asking fastdl.
        assert!(map_geometry(&cache, "cp_nothere", false).unwrap_err().to_string().contains("cannot find a match"));
        std::fs::write(geometry_path(&cache, "cp_cached"), b"{}").unwrap();
        assert_eq!(map_geometry(&cache, "cp_cached", false).unwrap(), b"{}");
    }

    #[test]
    fn warm_maps_are_valid_names() {
        let names: Vec<&str> = warm_maps().collect();
        assert!(names.len() > 10);
        assert!(names.iter().all(|n| is_name(n, b"_")), "{names:?}");
    }

    #[test]
    fn broken_cached_bsp_is_removed() {
        let cache = TempDir::new("test");
        let bsp = cache.join("broken.bsp");
        std::fs::write(&bsp, b"VBSP\0\0\0\0").unwrap();
        assert!(map_geometry(&cache, "broken", true).is_err());
        // So the next request downloads it again instead of failing on it forever.
        assert!(!bsp.exists());
    }

    fn make_old(path: &Path) {
        let two_days_ago = std::time::SystemTime::now() - 2 * DAY;
        std::fs::File::options().write(true).open(path).unwrap().set_modified(two_days_ago).unwrap();
    }

    /// A cache with the rendered overview of `map`, its image `image` and its box -100..100 by -50..50.
    fn with_rendered_map(cache: &Path, map: &str, image: &[u8]) {
        for dir in ["rendered/large", "rendered/json"] {
            std::fs::create_dir_all(cache.join(dir)).unwrap();
        }
        std::fs::write(cache.join(format!("rendered/large/{map}.webp")), image).unwrap();
        let info =
            br#"{"image": {"width": 100, "height": 50}, "world": {"min": [-100.0, -50.0], "max": [100.0, 50.0], "units_per_pixel": 2.0}}"#;
        std::fs::write(cache.join(format!("rendered/json/{map}.json")), info).unwrap();
    }

    #[test]
    fn a_rendered_overview_goes_before_demostfs() {
        let cache = TempDir::new("rendered");
        std::fs::create_dir(cache.join("demostf")).unwrap();
        let demostf = br#"{"cp_both": {"old": 1}, "cp_old": {"old": 1}}"#;
        std::fs::write(cache.join("demostf/boundaries.json"), demostf).unwrap();
        std::fs::write(cache.join("demostf/cp_both.webp"), b"old").unwrap();
        with_rendered_map(&cache, "cp_both", b"new");
        with_rendered_map(&cache, "cp_new", b"new");
        // An image without its box cannot be placed, so it is not listed or served.
        std::fs::write(cache.join("rendered/large/cp_nobox.webp"), b"new").unwrap();

        let list: serde_json::Value = serde_json::from_slice(&overview(&cache, "overviews.json").unwrap()).unwrap();
        let rendered =
            serde_json::json!({"boundary_min": {"x": -100.0, "y": -50.0}, "boundary_max": {"x": 100.0, "y": 50.0}, "rendered": true});
        assert_eq!(list, serde_json::json!({"cp_both": rendered, "cp_new": rendered, "cp_old": {"old": 1}}));
        assert_eq!(overview_kept(&cache, "overviews.json"), Some(overview(&cache, "overviews.json").unwrap()));
        // A page with an older list still gets demos.tf's own, under their old names.
        assert_eq!(overview(&cache, "boundaries.json").unwrap(), demostf);
        for (file, want) in [("cp_both.large.webp", &b"new"[..]), ("cp_new.large.webp", b"new"), ("cp_both.webp", b"old")] {
            assert_eq!(overview(&cache, file).unwrap(), want, "{file}");
            assert_eq!(overview_kept(&cache, file).unwrap(), want, "{file}");
        }
        for missing in ["cp_nobox.large.webp", "cp_old.large.webp", "../cp_new.large.webp", ".large.webp"] {
            assert!(overview_kept(&cache, missing).is_none(), "{missing}");
            assert!(overview(&cache, missing).is_err(), "{missing}");
        }
    }

    #[test]
    fn rendered_overviews_are_listed_while_demostf_is_down() {
        let cache = TempDir::new("renderedalone");
        with_rendered_map(&cache, "cp_new", b"new");
        let list = overview_from("http://127.0.0.1:1/", &cache, "overviews.json").unwrap();
        let list: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&list).unwrap();
        assert_eq!(list.keys().collect::<Vec<_>>(), ["cp_new"]);
        assert_eq!(overview_from("http://127.0.0.1:1/", &cache, "cp_new.large.webp").unwrap(), b"new");
    }

    #[test]
    fn an_overview_is_fetched_once_and_kept() {
        let cache = TempDir::new("fetched");
        let (base, asked) = web_server(vec![
            ("/boundaries.json", br#"{"cp_listed": {}}"#.to_vec()),
            ("/images/cp_listed.webp", b"RIFF".to_vec()),
            ("/images/cp_unlisted.webp", b"RIFF".to_vec()),
        ]);
        for _ in 0..2 {
            assert_eq!(overview_from(&base, &cache, "cp_listed.webp").unwrap(), b"RIFF");
        }
        assert_eq!(*lock(&asked), ["/boundaries.json", "/images/cp_listed.webp"]);
        assert!(overview_from(&base, &cache, "cp_unlisted.webp").is_err());
        assert_eq!(lock(&asked).len(), 2, "a map demos.tf does not list is not asked for");
    }

    #[test]
    fn the_list_of_overviews_is_fetched_again_after_a_day() {
        let cache = TempDir::new("stale");
        let (base, asked) = web_server(vec![("/boundaries.json", b"new".to_vec())]);
        let list = cache.join("demostf/boundaries.json");
        std::fs::create_dir_all(list.parent().unwrap()).unwrap();
        std::fs::write(&list, b"old").unwrap();
        assert_eq!(overview_from(&base, &cache, "boundaries.json").unwrap(), b"old", "still fresh");
        assert!(lock(&asked).is_empty());
        make_old(&list);
        assert_eq!(overview_from(&base, &cache, "boundaries.json").unwrap(), b"new");
        assert_eq!(std::fs::read(&list).unwrap(), b"new");
    }

    #[test]
    fn while_demos_tf_is_down_a_cached_list_is_served() {
        let cache = TempDir::new("down");
        let (base, _) = web_server(Vec::new());
        assert!(overview_from(&base, &cache, "boundaries.json").is_err(), "and nothing is cached yet");
        let list = cache.join("demostf/boundaries.json");
        std::fs::write(&list, b"old").unwrap();
        make_old(&list);
        assert_eq!(overview_from(&base, &cache, "boundaries.json").unwrap(), b"old");
    }

    #[test]
    fn an_overview_over_the_limit_is_refused_and_not_kept() {
        let cache = TempDir::new("big");
        let limit = usize::try_from(MAX_OVERVIEW).unwrap();
        let (base, _) = web_server(vec![("/boundaries.json", vec![b'x'; limit + 1])]);
        let err = overview_from(&base, &cache, "boundaries.json").unwrap_err().to_string();
        assert!(err.contains("over 8 MiB"), "{err}");
        assert!(!cache.join("demostf/boundaries.json").exists());
    }

    #[test]
    fn a_map_that_cannot_be_downloaded_is_an_error_and_leaves_nothing() {
        let cache = TempDir::new("nomap");
        let (base, asked) = web_server(Vec::new());
        assert!(map_geometry_from(&base, &cache, "cp_gone", true).is_err());
        assert_eq!(*lock(&asked), ["/cp_gone.bsp"]);
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    }

    #[test]
    fn a_broken_download_is_not_kept() {
        let cache = TempDir::new("baddownload");
        let (base, asked) = web_server(vec![("/cp_bad.bsp", b"not a bsp".to_vec())]);
        for _ in 0..2 {
            assert!(map_geometry_from(&base, &cache, "cp_bad", true).is_err());
        }
        assert_eq!(lock(&asked).len(), 2, "the next request fetches it again");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    }

    /// Downloading, building, keeping the geometry and dropping the BSP, and the gzipped copy of
    /// it.
    #[test]
    fn a_map_is_downloaded_and_built_once() {
        let Some(data) = process() else { return };
        let cache = TempDir::new("built");
        let (base, asked) = web_server(vec![("/cp_built.bsp", data)]);
        let json = map_geometry_from(&base, &cache, "cp_built", true).unwrap();
        assert!(!cache.join("cp_built.bsp").exists(), "only a bump of the suffix needs it again");
        assert!(serde_json::from_slice::<serde_json::Value>(&json).unwrap()["floors"].as_array().is_some_and(|f| !f.is_empty()));
        assert_eq!(map_geometry_from(&base, &cache, "cp_built", false).unwrap(), json, "read back, even with no match on it");
        assert_eq!(lock(&asked).len(), 1);

        let gz = map_geometry_gzip(&cache, "cp_built", false).unwrap();
        let mut unzipped = Vec::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_end(&mut unzipped).unwrap();
        assert_eq!(unzipped, json);
        std::fs::remove_file(geometry_path(&cache, "cp_built")).unwrap();
        assert_eq!(map_geometry_gzip(&cache, "cp_built", false).unwrap(), gz, "the gzipped copy is kept");

        // What blocks sight on it: built from the BSP fetched again, and kept.
        assert!(map_sight_from(&base, &cache, "cp_built", false).is_err(), "not without a match on it");
        let sight = map_sight_from(&base, &cache, "cp_built", true).unwrap();
        assert!(!cache.join("cp_built.bsp").exists());
        let mut body = Vec::new();
        flate2::read::GzDecoder::new(&sight[..]).read_to_end(&mut body).unwrap();
        let head = body.iter().position(|b| *b == b'\n').unwrap();
        let said: serde_json::Value = serde_json::from_slice(&body[..head]).unwrap();
        let voxels: u64 = said["dims"].as_array().unwrap().iter().map(|d| d.as_u64().unwrap()).product();
        assert_eq!((body.len() - head - 1) as u64, voxels.div_ceil(8), "a bit per voxel");
        assert!(body[head + 1..].iter().any(|b| *b != 0) && said["vox"].as_f64().unwrap() >= 32.0);
        assert_eq!(map_sight_from(&base, &cache, "cp_built", false).unwrap(), sight);
        assert_eq!(lock(&asked).len(), 2);
    }
}
