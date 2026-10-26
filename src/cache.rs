//! Map geometry built from fastdl's BSPs and demos.tf's overviews, kept on disk.

use crate::{bsp, is_name};
use anyhow::{Result, bail};
use serde::de::IgnoredAny;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
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
    if !is_name(name, b"_") {
        bail!("bad map name");
    }
    std::fs::create_dir_all(cache)?;
    // Bump the suffix when the extracted data changes, so cached maps are redone.
    let json = cache.join(format!("{name}.v7.json"));
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
        ureq::get(&format!("{FASTDL}{name}.bsp")).call()?.into_reader().take(MAX_BSP + 1).read_to_end(&mut d)?;
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

/// demos.tf's map overviews, `boundaries.json` and `<map>.webp`, kept in the cache so viewers do
/// not each load them from demos.tf's own server. The list is fetched again after a day; while
/// demos.tf is down, what the cache has is served.
pub(crate) fn overview(cache: &Path, file: &str) -> Result<Vec<u8>> {
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
    if image.is_some_and(|map| !demostf_maps(cache).iter().any(|m| m == map)) {
        bail!("no such overview");
    }
    let mut body = Vec::new();
    let fetched = ureq::get(&format!("{DEMOSTF}{remote}"))
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
#[expect(clippy::zero_sized_map_values, reason = "only the keys are read; the values are skipped unparsed")]
pub(crate) fn demostf_maps(cache: &Path) -> Vec<String> {
    overview(cache, "boundaries.json")
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, IgnoredAny>>(&b).ok())
        .map(|m| m.into_keys().collect())
        .unwrap_or_default()
}

/// Writes through a temporary file and a rename: a cached file that can be read is complete,
/// even after a crash halfway through writing it.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

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
}
