//! The demos of a match there are to download.

use std::fs::File;
use std::path::Path;
use std::time::SystemTime;

use serde::Serialize;

use super::comms::with_comms;
use super::sweep::KEPT_PREFIX;
use crate::matches::sessions::LIVE_PREFIX;

/// A demo to download with its comms in it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Download {
    /// What names it in the match: `<load>-<tick>` of its first frame (`<tick>` alone, for a
    /// name without a load).
    pub(crate) file: String,
    pub(crate) map: String,
    pub(crate) bytes: u64,
    /// How long it plays, by its header: 0 while it is still written.
    pub(crate) seconds: u32,
    /// When it was last written.
    #[serde(skip)]
    pub(crate) written: Option<SystemTime>,
}

impl Download {
    /// The load it is of.
    pub(crate) fn load(&self) -> Option<usize> {
        load_tick(&self.file).map(|(load, _)| load)
    }
}

/// A demo's header.
fn header_of(demo: &Path) -> Option<stvtf::demo::Header> {
    let mut header = vec![0u8; stvtf::demo::HEADER_LEN];
    std::io::Read::read_exact(&mut File::open(demo).ok()?, &mut header).ok()?;
    stvtf::demo::Header::parse(&header).ok()
}

/// The map a demo's header names.
pub(super) fn map_of(demo: &Path) -> Option<String> {
    Some(header_of(demo)?.map)
}

/// The load and first tick a demo's `file` (what follows the match in its name) says.
pub(super) fn load_tick(file: &str) -> Option<(usize, u32)> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (load, tick) = file.split_once('-').unwrap_or(("0", file));
    (digits(load) && digits(tick)).then_some((load.parse().ok()?, tick.parse().ok()?))
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
            let (header, meta) = (header_of(&path)?, e.metadata().ok()?);
            // A length that is no number, or none, is 0.
            let seconds = header.seconds.max(0.0) as u32;
            let download = Download { file: file.to_string(), map: header.map, bytes: meta.len(), seconds, written: meta.modified().ok() };
            Some((load_tick(file)?, download))
        })
        .collect();
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, d)| d).collect()
}
