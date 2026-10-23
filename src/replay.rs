//! Saved stvtf output played at game speed.

use crate::hub::{SharedHub, default_tick_interval, publish};
use anyhow::{Context, Result, bail};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::time::Duration;
use stvtf::Line;
use stvtf::output::{Stamped, Status};

/// A replay's pause between passes, how often a paused one checks for viewers, and the longest
/// gap it waits out between two lines (map loads and tick resets would stall it).
const REPLAY_PAUSE: Duration = Duration::from_secs(3);
const REPLAY_POLL: Duration = Duration::from_millis(250);
const MAX_REPLAY_GAP_MS: u64 = 1000;

/// Plays a saved stvtf output (gzipped if it ends in `.gz`) on a loop, each pass as a new match,
/// holding still whenever `play` says nobody is watching.
pub(crate) fn replay(hub: &SharedHub, path: &Path, play: &dyn Fn() -> bool) -> Result<()> {
    for pass in 0.. {
        if pass > 0 {
            // A pause between passes, which also keeps a file with (almost) no timing from
            // looping flat out.
            std::thread::sleep(REPLAY_PAUSE);
            publish(hub, &Line::Status(Status::MapChange));
        }
        let skipped = play_once(hub, path, play)?;
        if pass == 0 && skipped > 0 {
            eprintln!("{}: skipped {skipped} lines this stvtf-web cannot read", path.display());
        }
    }
    Ok(())
}

/// Plays the file once at its recorded speed. A line that does not read as a line of today's
/// stvtf (a recording from an older one, or a non-finite number written as null) is skipped;
/// returns how many were.
fn play_once(hub: &SharedHub, path: &Path, play: &dyn Fn() -> bool) -> Result<usize> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader: Box<dyn Read> =
        if path.extension().is_some_and(|e| e == "gz") { Box::new(flate2::read::GzDecoder::new(file)) } else { Box::new(file) };
    let (mut last, mut timed, mut skipped): (Option<u64>, bool, usize) = (None, false, 0);
    let mut interval = default_tick_interval();
    for (n, line) in BufReader::new(reader).lines().enumerate() {
        while !play() {
            std::thread::sleep(REPLAY_POLL);
        }
        let stamped: Stamped = match serde_json::from_str(&line?) {
            Ok(s) => s,
            Err(e) => {
                if skipped == 0 {
                    eprintln!("{}:{}: {e}", path.display(), n + 1);
                }
                skipped += 1;
                continue;
            }
        };
        if let Line::Server(server) = &stamped.line
            && server.tick_interval > 0.0
        {
            interval = server.tick_interval;
        }
        // Each line waits for the gap since the previous one, capped so map loads and
        // tick resets do not stall the replay.
        if let Some(t) = line_time(&stamped, interval) {
            if let Some(prev) = last {
                std::thread::sleep(Duration::from_millis(t.saturating_sub(prev).min(MAX_REPLAY_GAP_MS)));
            }
            last = Some(t);
            timed = true;
        }
        publish(hub, &stamped.line);
    }
    // Without any, the loop would replay the file as fast as it can, forever.
    if !timed {
        bail!("{}: no line has a time or tick", path.display());
    }
    Ok(skipped)
}

/// When a saved line happened, in milliseconds: live output has wall-clock `time`, offline output
/// (`stvtf replay`) only ticks, of `interval` seconds each. Tick 0 is a map load, not a time.
fn line_time(stamped: &Stamped, interval: f64) -> Option<u64> {
    const MS_PER_SECOND: f64 = 1000.0;
    stamped.time.or_else(|| stamped.line.tick().filter(|&t| t > 0).map(|t| (f64::from(t) * interval * MS_PER_SECOND).round() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::{SharedHub, default_tick_interval};
    use crate::testing::TempDir;
    use crate::testing::server;

    #[test]
    fn replay_without_times_stops() {
        let dir = TempDir::new("replay");
        let path = dir.join("replay.jsonl");
        for contents in ["", "{\"type\":\"status\",\"state\":\"new\"}\n"] {
            std::fs::write(&path, contents).unwrap();
            assert!(replay(&SharedHub::default(), &path, &|| true).is_err(), "{contents:?}");
        }
    }

    /// A recording from an older stvtf (a field it did not have yet, a line type since renamed)
    /// or with a non-finite number (written as null) still plays: such a line is skipped.
    #[test]
    fn replays_skip_lines_they_cannot_read() {
        let dir = TempDir::new("skip");
        let path = dir.join("skip.jsonl");
        let good = serde_json::to_string(&server("cp_badlands")).unwrap().replace(r#""tick":0"#, r#""tick":6"#);
        let lines = [
            r#"{"type":"status","state":"new"}"#,
            r#"{"type":"server","tick":5,"map":"cp_old"}"#,
            r#"{"type":"retired_line","tick":5}"#,
            r#"{"type":"mark","tick":5,"pos":[null,0.0,0.0]}"#,
            &good,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        let hub = SharedHub::default();
        assert_eq!(play_once(&hub, &path, &|| true).unwrap(), 3, "lines skipped");
        assert_eq!(hub.lock().unwrap().map.as_deref(), Some("cp_badlands"), "the lines after them still play");
    }

    #[test]
    fn replays_keep_the_recorded_timing() {
        let live: Stamped = serde_json::from_str(r#"{"type":"round_active","tick":500,"time":1700000000123}"#).unwrap();
        let tick = default_tick_interval();
        assert_eq!(line_time(&live, tick), Some(1_700_000_000_123), "live output: its receive time");
        let offline: Stamped = serde_json::from_str(r#"{"type":"round_active","tick":500}"#).unwrap();
        assert_eq!(line_time(&offline, tick), Some(7500), "offline output: its tick, 15 ms each");
        assert_eq!(line_time(&offline, 1.0 / 128.0), Some(3906), "on a 128-tick server");
        let status: Stamped = serde_json::from_str(r#"{"type":"status","state":"map_change"}"#).unwrap();
        assert_eq!(line_time(&status, tick), None);
        let loading: Stamped = serde_json::from_str(r#"{"type":"round_active","tick":0}"#).unwrap();
        assert_eq!(line_time(&loading, tick), None, "tick 0 is a map load, not a time");
    }
}
