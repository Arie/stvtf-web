//! Live demos let go of: kept ones once old, and those of matches gone.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::comms::{VOICE, has_voice, picks_file, voice_of};
use crate::matches::sessions::{LIVE_PREFIX, age};

/// What the names of demos kept for downloading start with.
pub(super) const KEPT_PREFIX: &str = "kept-";

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
            // `kept-<match>-<file>.dem`: its picks file (whose voice is whose) goes with it.
            let of = path.file_stem().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix(KEPT_PREFIX)).and_then(|n| n.split_once('-'));
            if let Some((id, file)) = of {
                let _ = std::fs::remove_file(picks_file(dir, id, file));
            }
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The match a live match's demo, or the voice kept beside one, is of: `live-<match>-….dem`
/// (`.voice`).
pub(super) fn live_demo_of(name: &str) -> Option<&str> {
    let demo = name.strip_suffix(VOICE).unwrap_or(name);
    if Path::new(demo).extension().is_none_or(|e| e != "dem") {
        return None;
    }
    Some(name.strip_prefix(LIVE_PREFIX)?.split_once('-').map_or("", |(id, _)| id))
}

/// Whether `name` is a live match's demo, or the voice kept beside one.
pub(crate) fn is_live_demo(name: &str) -> bool {
    live_demo_of(name).is_some()
}

/// Drops the live matches' demos (and their voice) in `dir` not written for `max_age`, but for
/// those of a match still running (`running`, by id): it drops those of the maps its recording
/// dropped itself.
pub(crate) fn sweep_live(dir: &Path, running: &HashSet<String>, max_age: Duration) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = live_demo_of(&name) else { continue };
        if !running.contains(id) && age(&e).is_ok_and(|a| a > max_age) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// `path`'s name with `kept-` for `live-`, if it has that.
pub(super) fn kept_name(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?.strip_prefix(LIVE_PREFIX)?;
    Some(path.with_file_name(format!("{KEPT_PREFIX}{name}")))
}

/// A demo no room can be on any more: kept for downloading under a `kept-` name if anything was
/// said in it, else gone.
pub(super) fn drop_or_keep(path: &Path) {
    if !has_voice(path) {
        let _ = std::fs::remove_file(voice_of(path));
        let _ = std::fs::remove_file(path);
    } else if let Some(kept) = kept_name(path)
        && std::fs::rename(path, &kept).is_ok()
    {
        let _ = std::fs::rename(voice_of(path), voice_of(&kept));
    }
}
