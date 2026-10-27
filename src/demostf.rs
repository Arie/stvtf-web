//! A demos.tf demo played as a match: fetched and decoded once, kept in memory only as its
//! gzipped stvtf output, and played through once at game speed. The demo file itself is dropped
//! as soon as it is decoded, and the output when the match ends.

use crate::hub::{SharedHub, publish};
use crate::lock;
use crate::replay::play_lines;
use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Deserialize;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use stvtf::Line;
use stvtf::output::{Stamped, Status};

const API: &str = "https://api.demos.tf/demos/";
/// A whole evening on one demo is a few hundred MiB.
const MAX_DEMO: u64 = 1 << 30;

/// One demo is fetched and decoded at a time: each holds the whole file in memory meanwhile.
pub(crate) static FETCHING: Mutex<()> = Mutex::new(());

/// What demos.tf says about a demo; only where its file is.
#[derive(Deserialize)]
struct Info {
    url: String,
}

/// Where demos.tf keeps demo `id`: on one of its own hosts, so a token cannot make us fetch
/// anything else.
fn demo_url(id: u64) -> Result<String> {
    let body = ureq::get(&format!("{API}{id}")).call().with_context(|| format!("demos.tf has no demo {id}"))?.into_string()?;
    let info: Info = serde_json::from_str(&body).with_context(|| format!("demos.tf's answer about demo {id}"))?;
    let host = info.url.strip_prefix("https://").and_then(|rest| rest.split('/').next()).unwrap_or_default();
    if host != "demos.tf" && !host.ends_with(".demos.tf") {
        bail!("demos.tf keeps demo {id} somewhere else: {}", info.url);
    }
    Ok(info.url)
}

/// Demo `id` as gzipped stvtf output, with `snapshot_hz` snapshots a second; None when told to
/// `stop` while other demos were fetched first.
fn fetch(id: u64, snapshot_hz: f32, stop: &AtomicBool) -> Result<Option<Vec<u8>>> {
    let _one_at_a_time = lock(&FETCHING);
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let mut demo = Vec::new();
    // One byte over the limit tells a demo that is too big from one that just fits.
    ureq::get(&demo_url(id)?).call()?.into_reader().take(MAX_DEMO + 1).read_to_end(&mut demo)?;
    if demo.len() as u64 > MAX_DEMO {
        bail!("demo {id} is over {} MiB", MAX_DEMO >> 20);
    }
    // Kept for the whole match: half the size of a fast level (a 24-minute demo is under 6 MiB).
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut written = Ok(());
    stvtf::decode::demo_file(&demo, snapshot_hz, &mut |line| {
        if written.is_ok() {
            written = serde_json::to_writer(&mut gz, &Stamped { line, time: None }).map_err(Into::into).and_then(|()| writeln!(gz));
        }
    })
    .with_context(|| format!("decoding demo {id}"))?;
    written?;
    Ok(Some(gz.finish()?))
}

/// Plays demos.tf demo `id` into `hub` once, holding still while `play` says nobody watches,
/// until it ends or `stop`. The pages hear when it ends, or why it could not be played.
pub(crate) fn play(hub: &SharedHub, id: u64, snapshot_hz: f32, play: &dyn Fn() -> bool, stop: &AtomicBool) -> Result<()> {
    let played = fetch(id, snapshot_hz, stop).and_then(|output| match output {
        Some(output) => play_lines(hub, GzDecoder::new(output.as_slice()), &format!("demo {id}"), play, stop).map(drop),
        None => Ok(()),
    });
    match &played {
        Ok(()) if stop.load(Ordering::Relaxed) => {}
        Ok(()) => {
            let ended = Status::Disconnected { reason: "the demo ended".into(), decode_errors: 0, packets: 0, lost: 0 };
            publish(hub, &Line::Status(ended));
        }
        Err(e) => publish(hub, &Line::Status(Status::GaveUp { reason: format!("{e:#}") })),
    }
    played
}
