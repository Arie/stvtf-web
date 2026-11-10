//! A demos.tf demo played as a match: fetched and decoded once, kept in memory only as its
//! gzipped stvtf output, and played through once at game speed. The demo file itself is dropped
//! as soon as it is decoded, and the output when the match ends.

use crate::matches::hub::{SharedHub, publish};
use crate::matches::replay::play_lines;
use crate::{is_name, lock};
use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Deserialize;
use std::io::{BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
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

fn demo_url(id: u64) -> Result<String> {
    demo_url_from(API, id)
}

fn demo_url_from(api: &str, id: u64) -> Result<String> {
    let body = ureq::get(&format!("{api}{id}")).call().with_context(|| format!("demos.tf has no demo {id}"))?.into_string()?;
    file_url(id, &body)
}

/// Where demos.tf's answer `body` says demo `id` is: on one of its own hosts, so a token cannot make
/// us fetch anything else.
fn file_url(id: u64, body: &str) -> Result<String> {
    let info: Info = serde_json::from_str(body).with_context(|| format!("demos.tf's answer about demo {id}"))?;
    let host = info.url.strip_prefix("https://").and_then(|rest| rest.split('/').next()).unwrap_or_default();
    // Only a plain name: anything else (`?`, `#`, `@`, a port) can put the real host elsewhere.
    if !is_name(host, b".-") || (host != "demos.tf" && !host.ends_with(".demos.tf")) {
        bail!("demos.tf keeps demo {id} somewhere else: {}", info.url);
    }
    Ok(info.url)
}

/// Demo `id` as gzipped stvtf output, with `snapshot_hz` snapshots a second; None when told to
/// `stop` while other demos were fetched first. `keep` gets the demo file.
fn fetch(id: u64, snapshot_hz: f32, stop: &AtomicBool, keep: &dyn Fn(&[u8])) -> Result<Option<Vec<u8>>> {
    let _one_at_a_time = lock(&FETCHING);
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let started = Instant::now();
    let demo = download(&demo_url(id)?, id, MAX_DEMO)?;
    let fetched = started.elapsed();
    keep(&demo);
    let output = decode(id, &demo, snapshot_hz)?;
    eprintln!(
        "stvtf-web: demo {id}: {} MiB fetched in {:.1} s, decoded to {} MiB in {:.1} s",
        demo.len() >> 20,
        fetched.as_secs_f64(),
        output.len() >> 20,
        started.elapsed().saturating_sub(fetched).as_secs_f64()
    );
    Ok(Some(output))
}

fn download(url: &str, id: u64, limit: u64) -> Result<Vec<u8>> {
    let mut demo = Vec::new();
    // One byte over the limit tells a demo that is too big from one that just fits.
    ureq::get(url).call()?.into_reader().take(limit + 1).read_to_end(&mut demo)?;
    if demo.len() as u64 > limit {
        bail!("demo {id} is over {} MiB", limit >> 20);
    }
    Ok(demo)
}

/// A demo file as gzipped stvtf output.
fn decode(id: u64, demo: &[u8], snapshot_hz: f32) -> Result<Vec<u8>> {
    // Kept for the whole match: half the size of a fast level (a 24-minute demo is under 6 MiB).
    // Buffered: flate2 runs the compressor on every write, and serde writes a line in many small pieces.
    let mut gz = BufWriter::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut written = Ok(());
    stvtf::decode::demo_file(demo, snapshot_hz, &mut |line| {
        if written.is_ok() {
            written = serde_json::to_writer(&mut gz, &Stamped { line, time: None }).map_err(Into::into).and_then(|()| writeln!(gz));
        }
    })
    .with_context(|| format!("decoding demo {id}"))?;
    written?;
    Ok(gz.into_inner().map_err(std::io::IntoInnerError::into_error)?.finish()?)
}

/// Plays demos.tf demo `id` into `hub` once, holding still while `play` says nobody watches,
/// until it ends or `stop`; `keep_demo` gets the demo file once fetched, `keep` the output once
/// decoded.
pub(crate) fn play(
    hub: &SharedHub,
    id: u64,
    snapshot_hz: f32,
    play: &dyn Fn() -> bool,
    stop: &AtomicBool,
    keep_demo: &dyn Fn(&[u8]),
    keep: &dyn Fn(Arc<Vec<u8>>),
) -> Result<()> {
    let played = fetch(id, snapshot_hz, stop, keep_demo).and_then(|output| match output {
        Some(output) => {
            let output = Arc::new(output);
            keep(output.clone());
            play_lines(hub, GzDecoder::new(output.as_slice()), &format!("demo {id}"), play, stop).map(drop)
        }
        None => Ok(()),
    });
    tell_pages(hub, &played, stop);
    played
}

/// The pages hear when a demo ended, or why it could not be played.
fn tell_pages(hub: &SharedHub, played: &Result<()>, stop: &AtomicBool) {
    match played {
        Ok(()) if stop.load(Ordering::Relaxed) => {}
        Ok(()) => {
            let ended = Status::Disconnected { reason: "the demo ended".into(), decode_errors: 0, packets: 0, lost: 0 };
            publish(hub, &Line::Status(ended));
        }
        Err(e) => publish(hub, &Line::Status(Status::GaveUp { reason: format!("{e:#}") })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::web_server;

    #[test]
    fn demos_are_fetched_only_from_demos_tf() {
        for url in ["https://demos.tf/1.dem", "https://static.demos.tf/1.dem", "https://a.b.demos.tf/x/1.dem"] {
            let body = format!(r#"{{"url":"{url}"}}"#);
            assert_eq!(file_url(1, &body).unwrap(), url);
        }
        for url in [
            "http://demos.tf/1.dem",
            "https://evil.example/1.dem",
            "https://evildemos.tf/1.dem",
            "https://demos.tf.evil.example/1.dem",
            "https://evil.example?.demos.tf/1.dem",
            "https://evil.example#.demos.tf/1.dem",
            "https://evil.example\\\\.demos.tf/1.dem",
            "https://evil.example%2f.demos.tf/1.dem",
            "https://demos.tf@evil.example/1.dem",
            "https://evil.example@static.demos.tf/1.dem",
            "https://demos.tf:8443/1.dem",
            "",
        ] {
            let body = format!(r#"{{"url":"{url}"}}"#);
            assert!(file_url(1, &body).is_err(), "{url}");
        }
        assert!(file_url(1, "not json").is_err());
        assert!(file_url(1, "{}").is_err());
    }

    #[test]
    fn a_demo_stopped_before_its_turn_is_not_fetched_and_says_nothing() {
        let hub = SharedHub::default();
        play(&hub, 1, 20.0, &|| true, &AtomicBool::new(true), &|_| {}, &drop).unwrap();
        assert_eq!(crate::matches::hub::subscribe(&hub, false).try_iter().count(), 0);
    }

    /// A demo from stvtf's own tests, which CI checks out next to this repository.
    fn recorded() -> Vec<u8> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../stvtf/tests/fixtures/tc_hydro.dem")).unwrap()
    }

    fn lines(output: &[u8]) -> Vec<Stamped> {
        let mut text = String::new();
        GzDecoder::new(output).read_to_string(&mut text).unwrap();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn a_demo_is_kept_as_gzipped_lines_that_play_as_a_match() {
        let output = decode(1, &recorded(), 20.0).unwrap();
        let lines = lines(&output);
        assert!(lines.iter().any(|l| matches!(&l.line, Line::Server(s) if s.map == "tc_hydro")));
        assert!(lines.iter().any(|l| matches!(l.line, Line::Snapshot(_))));
        assert!(lines.iter().all(|l| l.time.is_none()), "timed by its ticks");

        // Until the map is known: at game speed the whole demo would take minutes.
        let hub = SharedHub::default();
        let stop = AtomicBool::new(false);
        let until_the_map = || {
            stop.store(lock(&hub).map.is_some(), Ordering::Relaxed);
            true
        };
        assert_eq!(play_lines(&hub, GzDecoder::new(output.as_slice()), "demo 1", &until_the_map, &stop).unwrap(), 0);
        assert_eq!(lock(&hub).map.as_deref(), Some("tc_hydro"));
    }

    #[test]
    fn the_snapshot_rate_reaches_the_decoder() {
        let snapshots = |hz| lines(&decode(1, &recorded(), hz).unwrap()).iter().filter(|l| matches!(l.line, Line::Snapshot(_))).count();
        assert_eq!(snapshots(0.0), 0);
        assert!(snapshots(5.0) < snapshots(20.0));
    }

    #[test]
    fn a_file_that_is_no_demo_is_an_error() {
        let err = decode(7, b"not a demo", 20.0).unwrap_err();
        assert!(format!("{err:#}").contains("decoding demo 7"), "{err:#}");
    }

    #[test]
    fn demos_tf_is_asked_where_the_demo_is() {
        let (base, asked) = web_server(vec![("/7", br#"{"url":"https://static.demos.tf/7.dem"}"#.to_vec())]);
        assert_eq!(demo_url_from(&base, 7).unwrap(), "https://static.demos.tf/7.dem");
        assert_eq!(*lock(&asked), ["/7"]);
        let err = demo_url_from(&base, 8).unwrap_err();
        assert!(format!("{err:#}").contains("demos.tf has no demo 8"), "{err:#}");
    }

    #[test]
    fn a_download_stops_at_the_limit() {
        let (base, _) = web_server(vec![("/fits", vec![1; 10]), ("/big", vec![1; 11])]);
        assert_eq!(download(&format!("{base}fits"), 7, 10).unwrap().len(), 10);
        assert!(download(&format!("{base}big"), 7, 10).unwrap_err().to_string().contains("demo 7 is over"));
        assert!(download(&format!("{base}gone"), 7, 10).is_err());
    }

    fn told(hub: &SharedHub) -> Vec<serde_json::Value> {
        crate::matches::hub::subscribe(hub, false).try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect()
    }

    #[test]
    fn pages_hear_how_a_demo_ended() {
        let stopped = AtomicBool::new(true);
        let running = AtomicBool::new(false);

        let hub = SharedHub::default();
        tell_pages(&hub, &Ok(()), &stopped);
        assert_eq!(told(&hub), Vec::<serde_json::Value>::new(), "nobody is left to tell");

        let hub = SharedHub::default();
        tell_pages(&hub, &Ok(()), &running);
        let said = told(&hub);
        assert_eq!((said[0]["state"].as_str(), said[0]["reason"].as_str()), (Some("disconnected"), Some("the demo ended")));

        let hub = SharedHub::default();
        tell_pages(&hub, &Err(anyhow::anyhow!("demos.tf has no demo 7")), &running);
        let said = told(&hub);
        assert_eq!((said[0]["state"].as_str(), said[0]["reason"].as_str()), (Some("gave_up"), Some("demos.tf has no demo 7")));
    }
}
