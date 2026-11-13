//! A demos.tf demo played as a match: fetched and decoded, then played through once at game
//! speed from its gzipped stvtf output. With a store, the output is kept there for the next to
//! play the demo (and the demo file too, where relays play it), so that neither is fetched nor
//! decoded again; the store's sweep keeps them within a budget. The file comes in to disk, many
//! at a time; decoding holds a whole file in memory, one at a time.

use crate::matches::hub::{SharedHub, publish};
use crate::matches::replay::play_lines;
use crate::matches::sessions::{Store, demo_dem, demo_output};
use crate::{is_name, lock};
use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Deserialize;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use stvtf::Line;
use stvtf::output::{Stamped, Status};

const API: &str = "https://api.demos.tf/demos/";
/// A whole evening on one demo is a few hundred MiB.
const MAX_DEMO: u64 = 1 << 30;

/// One demo is decoded at a time: each holds the whole file in memory meanwhile.
pub(crate) static DECODING: Mutex<()> = Mutex::new(());
/// Held by a test while no demo is to reach demos.tf.
#[cfg(test)]
pub(crate) static HELD: Mutex<()> = Mutex::new(());
/// How often the pages hear how far a download is, at most.
const SAID_EVERY: Duration = Duration::from_millis(500);
/// How long demos.tf gets to connect, and to send the next of an answer, before it is given up on.
const CONNECT_FOR: Duration = Duration::from_secs(30);
const STALLED_FOR: Duration = Duration::from_secs(60);

/// Where a demo's files are kept from one play to the next.
pub(crate) struct DemoFiles {
    pub(crate) store: Store,
    /// Whether the demo file is kept too (relays play it), not only what it decodes to.
    pub(crate) dem: bool,
}

/// A download: what it writes to, and what it tells of how far it is. Whether it came to its end
/// (not when told to stop).
type Download<'a> = &'a dyn Fn(&mut dyn Write, &mut dyn FnMut(Option<u8>)) -> Result<bool>;

/// What demos.tf says about a demo; only where its file is.
#[derive(Deserialize)]
struct Info {
    url: String,
}

/// What asks demos.tf: given up on once an answer stalls for `stalled`.
fn agent(stalled: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(CONNECT_FOR).timeout_read(stalled).build()
}

fn demo_url(agent: &ureq::Agent, id: u64) -> Result<String> {
    demo_url_from(agent, API, id)
}

fn demo_url_from(agent: &ureq::Agent, api: &str, id: u64) -> Result<String> {
    let body = agent.get(&format!("{api}{id}")).call().with_context(|| format!("demos.tf has no demo {id}"))?.into_string()?;
    file_url(id, &body)
}

/// Where demos.tf's answer `body` says demo `id` is: on one of its own hosts, so a token cannot
/// make us fetch anything else.
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
/// `stop` meanwhile. From what is `kept` of it, else downloaded and decoded (and then kept): the
/// pages on `hub` hear how far that is.
fn fetch(hub: &SharedHub, id: u64, snapshot_hz: f32, stop: &AtomicBool, kept: Option<&DemoFiles>) -> Result<Option<Vec<u8>>> {
    #[cfg(test)]
    let _held = lock(&HELD);
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let agent = agent(STALLED_FOR);
    let download =
        |out: &mut dyn Write, said: &mut dyn FnMut(Option<u8>)| download(&agent, &demo_url(&agent, id)?, id, MAX_DEMO, out, stop, said);
    fetch_with(hub, id, snapshot_hz, stop, kept, &download)
}

fn fetch_with(
    hub: &SharedHub,
    id: u64,
    snapshot_hz: f32,
    stop: &AtomicBool,
    kept: Option<&DemoFiles>,
    download: Download,
) -> Result<Option<Vec<u8>>> {
    if let Some(output) = kept.and_then(|k| kept_output(k, id)) {
        return Ok(Some(output));
    }
    let started = Instant::now();
    let mut said = |percent| publish(hub, &Line::Status(Status::Fetching { percent }));
    let file = kept.map(|k| k.store.dir.join(demo_dem(id)));
    let mut in_memory = Vec::new();
    let whole = match &file {
        Some(file) => file.exists() || download_to_file(file, download, &mut said)?,
        None => download(&mut in_memory, &mut said)?,
    };
    if !whole {
        return Ok(None);
    }
    let fetched = started.elapsed();
    let _one_at_a_time = lock(&DECODING);
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    publish(hub, &Line::Status(Status::Decoding));
    let waited = started.elapsed();
    let demo = match &file {
        Some(file) => std::fs::read(file)?,
        None => in_memory,
    };
    let output = decode_and_keep(id, &demo, snapshot_hz, kept.zip(file.as_deref()))?;
    log!(
        "demo {id}: {} MiB fetched in {:.1} s, decoded to {} MiB in {:.1} s",
        demo.len() >> 20,
        fetched.as_secs_f64(),
        output.len() >> 20,
        started.elapsed().saturating_sub(waited).as_secs_f64()
    );
    Ok(Some(output))
}

/// What demo `id` decoded to, as kept: with its file too where relays play it.
fn kept_output(k: &DemoFiles, id: u64) -> Option<Vec<u8>> {
    let output = k.store.take(&demo_output(id)).ok()?;
    if k.dem && !k.store.dir.join(demo_dem(id)).exists() {
        return None;
    }
    k.store.touch(&demo_output(id));
    k.store.touch(&demo_dem(id));
    log!("demo {id}: as kept, {} MiB", output.len() >> 20);
    Some(output)
}

/// Downloads to `file`, beside it until it is whole (a file by that name is one; another fetch
/// of the same demo has a part file of its own). Whether all of it came.
fn download_to_file(file: &Path, download: Download, said: &mut dyn FnMut(Option<u8>)) -> Result<bool> {
    let part = file.with_extension(format!("dem.{}.part", crate::matches::sessions::random_id()));
    std::fs::create_dir_all(file.parent().unwrap_or(file))?;
    let whole = download(&mut BufWriter::new(std::fs::File::create(&part)?), said);
    if !matches!(whole, Ok(true)) {
        let _ = std::fs::remove_file(&part);
        return whole;
    }
    std::fs::rename(&part, file)?;
    Ok(true)
}

/// `demo` decoded, and kept in `kept`'s store, its `file` too where relays play it: a file that
/// will not decode is not kept to fail again.
fn decode_and_keep(id: u64, demo: &[u8], snapshot_hz: f32, kept: Option<(&DemoFiles, &Path)>) -> Result<Vec<u8>> {
    let output = decode(id, demo, snapshot_hz);
    if let Some((k, file)) = kept {
        if output.is_err() || !k.dem {
            let _ = std::fs::remove_file(file);
        }
        if let Ok(output) = &output
            && let Err(e) = k.store.put(&demo_output(id), output)
        {
            log!("keeping demo {id}: {e:#}");
        }
    }
    output
}

/// What is at `url` (demo `id`, of `limit` bytes at most) written to `out`, telling `said` how
/// far it is where the server says how much there is. Whether all of it came: not when told to
/// `stop`.
fn download(
    agent: &ureq::Agent,
    url: &str,
    id: u64,
    limit: u64,
    out: &mut dyn Write,
    stop: &AtomicBool,
    said: &mut dyn FnMut(Option<u8>),
) -> Result<bool> {
    let answer = agent.get(url).call()?;
    let all = answer.header("Content-Length").and_then(|l| l.parse::<u64>().ok()).filter(|l| *l > 0);
    let (mut from, mut chunk, mut got, mut told) = (answer.into_reader(), vec![0; 64 << 10], 0u64, None::<Instant>);
    said(all.map(|_| 0));
    loop {
        let n = from.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        got += n as u64;
        // One byte over the limit tells a demo that is too big from one that just fits.
        if got > limit {
            bail!("demo {id} is over {} MiB", limit >> 20);
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        out.write_all(&chunk[..n])?;
        if told.is_none_or(|t| t.elapsed() >= SAID_EVERY) {
            told = Some(Instant::now());
            said(all.map(|all| (got.min(all) * 100 / all) as u8));
        }
    }
    out.flush()?;
    Ok(true)
}

/// A demo file as gzipped stvtf output.
fn decode(id: u64, demo: &[u8], snapshot_hz: f32) -> Result<Vec<u8>> {
    // Kept for the whole match: half the size of a fast level (a 24-minute demo is under 6 MiB).
    // Buffered: flate2 runs the compressor on every write, and serde writes a line in many small
    // pieces.
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
/// until it ends or `stop`; its files are `kept` from one play to the next, and `keep` gets the
/// output once it is there.
pub(crate) fn play(
    hub: &SharedHub,
    id: u64,
    snapshot_hz: f32,
    play: &dyn Fn() -> bool,
    stop: &AtomicBool,
    kept: Option<&DemoFiles>,
    keep: &dyn Fn(Arc<Vec<u8>>),
) -> Result<()> {
    let played = fetch(hub, id, snapshot_hz, stop, kept).and_then(|output| match output {
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
        // Kept already, so nothing is asked of demos.tf: told to stop, it plays none of it.
        let dir = crate::testing::TempDir::new("demostf-stopped");
        let kept = DemoFiles { store: Store { dir: dir.join("store"), keys: None }, dem: false };
        kept.store.put(&demo_output(1), &decode(1, &recorded(), 20.0).unwrap()).unwrap();
        play(&hub, 1, 20.0, &|| true, &AtomicBool::new(true), Some(&kept), &drop).unwrap();
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
        assert_eq!(demo_url_from(&agent(STALLED_FOR), &base, 7).unwrap(), "https://static.demos.tf/7.dem");
        assert_eq!(*lock(&asked), ["/7"]);
        let err = demo_url_from(&agent(STALLED_FOR), &base, 8).unwrap_err();
        assert!(format!("{err:#}").contains("demos.tf has no demo 8"), "{err:#}");
    }

    #[test]
    fn a_download_stops_at_the_limit_or_when_told_to_and_says_how_far_it_is() {
        let (base, _) = web_server(vec![("/fits", vec![1; 10]), ("/big", vec![1; 11])]);
        let running = AtomicBool::new(false);
        let (mut out, mut said) = (Vec::new(), Vec::new());
        assert!(download(&agent(STALLED_FOR), &format!("{base}fits"), 7, 10, &mut out, &running, &mut |p| said.push(p)).unwrap());
        assert_eq!((out.len(), said.first(), said.last()), (10, Some(&Some(0)), Some(&Some(100))));
        let err = download(&agent(STALLED_FOR), &format!("{base}big"), 7, 10, &mut Vec::new(), &running, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("demo 7 is over"));
        assert!(download(&agent(STALLED_FOR), &format!("{base}gone"), 7, 10, &mut Vec::new(), &running, &mut |_| {}).is_err());
        let mut out = Vec::new();
        assert!(!download(&agent(STALLED_FOR), &format!("{base}fits"), 7, 10, &mut out, &AtomicBool::new(true), &mut |_| {}).unwrap());
        assert_eq!(out, Vec::<u8>::new());
    }

    #[test]
    fn a_download_that_stalls_is_given_up_on() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/7.dem", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (_quiet, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(5));
        });
        let started = Instant::now();
        let running = AtomicBool::new(false);
        assert!(download(&agent(Duration::from_millis(200)), &url, 7, 10, &mut Vec::new(), &running, &mut |_| {}).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A store, and whether relays play its demos; a download of the recorded demo that counts.
    fn kept(dir: &crate::testing::TempDir, dem: bool) -> DemoFiles {
        DemoFiles { store: Store { dir: dir.join("store"), keys: None }, dem }
    }

    #[test]
    fn a_demo_played_before_is_neither_fetched_nor_decoded_again() {
        let dir = crate::testing::TempDir::new("demostf-kept");
        let (hub, running, fetched) = (SharedHub::default(), AtomicBool::new(false), Mutex::new(0));
        let download = |out: &mut dyn Write, said: &mut dyn FnMut(Option<u8>)| {
            *lock(&fetched) += 1;
            said(Some(50));
            out.write_all(&recorded())?;
            Ok(true)
        };
        // No relay plays it: only what it decodes to is kept.
        let k = kept(&dir, false);
        let first = fetch_with(&hub, 5, 20.0, &running, Some(&k), &download).unwrap().unwrap();
        assert!(k.store.has(&demo_output(5)) && !k.store.has(&demo_dem(5)));
        let states: Vec<String> = told(&hub).iter().map(|l| l["state"].as_str().unwrap_or_default().to_string()).collect();
        assert_eq!(states.last().map(String::as_str), Some("decoding"), "the pages hear how far it is: {states:?}");
        assert_eq!(fetch_with(&hub, 5, 20.0, &running, Some(&k), &download).unwrap().unwrap(), first);
        assert_eq!(*lock(&fetched), 1);
        // A relay plays it: the demo file is wanted too, fetched once more and then kept.
        let k = kept(&dir, true);
        assert_eq!(fetch_with(&hub, 5, 20.0, &running, Some(&k), &download).unwrap().unwrap(), first);
        assert!(k.store.has(&demo_dem(5)));
        fetch_with(&hub, 5, 20.0, &running, Some(&k), &download).unwrap();
        assert_eq!(*lock(&fetched), 2);
    }

    #[test]
    fn a_download_that_fails_or_a_file_that_is_no_demo_leaves_nothing_kept() {
        let dir = crate::testing::TempDir::new("demostf-bad");
        let (hub, running, k) = (SharedHub::default(), AtomicBool::new(false), kept(&dir, true));
        let fails = |_: &mut dyn Write, _: &mut dyn FnMut(Option<u8>)| anyhow::bail!("down");
        assert!(fetch_with(&hub, 6, 20.0, &running, Some(&k), &fails).is_err());
        let junk = |out: &mut dyn Write, _: &mut dyn FnMut(Option<u8>)| {
            out.write_all(b"not a demo")?;
            Ok(true)
        };
        assert!(fetch_with(&hub, 6, 20.0, &running, Some(&k), &junk).is_err());
        let left: Vec<_> = std::fs::read_dir(&k.store.dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_demo_fetched_twice_at_once_comes_in_whole_though_the_first_fetch_stops() {
        let dir = crate::testing::TempDir::new("demostf-twice");
        let (k, running) = (kept(&dir, true), AtomicBool::new(false));
        let (both, first_done) = (std::sync::Barrier::new(2), AtomicBool::new(false));
        let stops = |out: &mut dyn Write, _: &mut dyn FnMut(Option<u8>)| {
            out.write_all(&recorded()[..1000])?;
            out.flush()?;
            both.wait();
            both.wait();
            Ok(false)
        };
        let whole = |out: &mut dyn Write, _: &mut dyn FnMut(Option<u8>)| {
            both.wait();
            out.write_all(&recorded())?;
            both.wait();
            while !first_done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(true)
        };
        std::thread::scope(|scope| {
            scope.spawn(|| {
                assert!(fetch_with(&SharedHub::default(), 4, 20.0, &running, Some(&k), &stops).unwrap().is_none());
                first_done.store(true, Ordering::Relaxed);
            });
            assert!(fetch_with(&SharedHub::default(), 4, 20.0, &running, Some(&k), &whole).unwrap().is_some());
        });
        assert_eq!(std::fs::read(k.store.dir.join(demo_dem(4))).unwrap(), recorded());
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
