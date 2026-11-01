//! Map geometry built from fastdl's BSPs and demos.tf's overviews, kept on disk.

use crate::{bsp, is_name};
use anyhow::{Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::de::IgnoredAny;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The most played maps, built when the server starts.
pub(crate) const WARM: &str = include_str!("../maps.txt");
const FASTDL: &str = "https://fastdl.serveme.tf/maps/";
const MAX_BSP: u64 = 512 << 20;
const DEMOSTF: &str = "https://maps.demos.tf/";
const MAX_OVERVIEW: u64 = 8 << 20;
const DAY: Duration = Duration::from_hours(24);

/// Builds a map that is not in the cache yet only when `fetch`: otherwise anyone could have every
/// map on fastdl downloaded and kept here.
pub(crate) fn map_geometry(cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    map_geometry_from(FASTDL, cache, name, fetch)
}

fn map_geometry_from(fastdl: &str, cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    if !is_name(name, b"_") {
        bail!("bad map name");
    }
    std::fs::create_dir_all(cache)?;
    let json = geometry_path(cache, name);
    if let Ok(body) = std::fs::read(&json) {
        return Ok(body);
    }
    if !fetch {
        bail!("no match is on {name}");
    }
    let bsp_path = cache.join(format!("{name}.bsp"));
    let data = if let Ok(d) = std::fs::read(&bsp_path) {
        d
    } else {
        let mut d = Vec::new();
        // One byte over the limit tells a map that is too big from one that just fits.
        ureq::get(&format!("{fastdl}{name}.bsp")).call()?.into_reader().take(MAX_BSP + 1).read_to_end(&mut d)?;
        if d.len() as u64 > MAX_BSP {
            bail!("{name}.bsp is over {} MiB", MAX_BSP >> 20);
        }
        write_atomic(&bsp_path, &d)?;
        d
    };
    let body = match bsp::geometry(&data) {
        Ok(g) => serde_json::to_vec(&g)?,
        Err(e) => {
            // A broken download would otherwise fail every request for this map; the next one fetches it again.
            let _ = std::fs::remove_file(&bsp_path);
            return Err(e);
        }
    };
    write_atomic(&json, &body)?;
    // Only a bump of the suffix needs the BSP again, and the most played maps add up to GBs.
    let _ = std::fs::remove_file(&bsp_path);
    Ok(body)
}

/// `map_geometry` gzipped, kept next to the JSON so a map is compressed once, not on every load.
pub(crate) fn map_geometry_gzip(cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    if !is_name(name, b"_") {
        bail!("bad map name");
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

/// Where a map's geometry is cached. Bump the suffix when the extracted data changes, so cached
/// maps are redone.
fn geometry_path(cache: &Path, name: &str) -> PathBuf {
    cache.join(format!("{name}.v7.json"))
}

/// demos.tf's map overviews, `boundaries.json` and `<map>.webp`, kept in the cache so viewers do
/// not each load them from demos.tf's own server. The list is fetched again after a day; while
/// demos.tf is down, what the cache has is served.
pub(crate) fn overview(cache: &Path, file: &str) -> Result<Vec<u8>> {
    overview_from(DEMOSTF, cache, file)
}

fn overview_from(demostf: &str, cache: &Path, file: &str) -> Result<Vec<u8>> {
    let image = match file.strip_suffix(".webp") {
        _ if file == "boundaries.json" => None,
        Some(map) if is_name(map, b"_") => Some(map),
        _ => bail!("no such overview"),
    };
    let remote = if image.is_some() { format!("images/{file}") } else { file.to_string() };
    let dir = cache.join("demostf");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(file);
    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().map(|t| t.elapsed().unwrap_or_default());
    if age.is_some_and(|a| image.is_some() || a < DAY) {
        return Ok(std::fs::read(&path)?);
    }
    // Only the maps demos.tf has: any other name would be a request to demos.tf every time.
    if image.is_some_and(|map| !demostf_maps_from(demostf, cache).iter().any(|m| m == map)) {
        bail!("no such overview");
    }
    let mut body = Vec::new();
    let fetched = ureq::get(&format!("{demostf}{remote}"))
        .call()
        .map_err(anyhow::Error::from)
        .and_then(|r| Ok(r.into_reader().take(MAX_OVERVIEW + 1).read_to_end(&mut body)?));
    match fetched {
        Ok(n) if n as u64 <= MAX_OVERVIEW => {
            write_atomic(&path, &body)?;
            Ok(body)
        }
        Ok(_) => bail!("{file} is over {} MiB", MAX_OVERVIEW >> 20),
        Err(e) => std::fs::read(&path).map_err(|_| e),
    }
}

/// The maps demos.tf has an overview of, from its `boundaries.json`.
pub(crate) fn demostf_maps(cache: &Path) -> Vec<String> {
    demostf_maps_from(DEMOSTF, cache)
}

#[expect(clippy::zero_sized_map_values, reason = "only the keys are read; the values are skipped unparsed")]
fn demostf_maps_from(demostf: &str, cache: &Path) -> Vec<String> {
    overview_from(demostf, cache, "boundaries.json")
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, IgnoredAny>>(&b).ok())
        .map(|m| m.into_keys().collect())
        .unwrap_or_default()
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
        // A half-written file under its own name would otherwise stay for good.
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::tests::process;
    use crate::lock;
    use crate::testing::{TempDir, web_server};

    #[test]
    fn write_atomic_replaces_the_file() {
        let dir = TempDir::new("atomic");
        let path = dir.join("m.v7.json");
        std::fs::write(&path, b"old, longer contents").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
    }

    #[test]
    fn a_failed_write_atomic_leaves_no_temporary_file() {
        let dir = TempDir::new("atomicfail");
        // A file cannot be renamed over a directory that has something in it.
        let path = dir.join("m.v7.json");
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
        assert!(map_geometry(&cache, "cp_nothere", false).unwrap_err().to_string().contains("no match"));
        std::fs::write(cache.join("cp_cached.v7.json"), b"{}").unwrap();
        assert_eq!(map_geometry(&cache, "cp_cached", false).unwrap(), b"{}");
    }

    #[test]
    fn warm_maps_are_valid_names() {
        let names: Vec<&str> = WARM.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
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

    /// Downloading, building, keeping the geometry and dropping the BSP, and the gzipped copy of it.
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
        std::fs::remove_file(cache.join("cp_built.v7.json")).unwrap();
        assert_eq!(map_geometry_gzip(&cache, "cp_built", false).unwrap(), gz, "the gzipped copy is kept");
    }
}
