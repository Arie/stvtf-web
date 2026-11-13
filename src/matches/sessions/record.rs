//! A live match's recorder: its lines into its recording, and its demos' loads.

use super::session::Session;
use super::store::{Store, live_file, to_file};
use crate::matches::hub::subscribe_all;
use crate::matches::recording::Recording;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// How long the recorder waits for a line before it looks whether to stop.
pub(super) const RECORD_POLL: Duration = Duration::from_millis(250);

/// Records live match `s` from now on, to its file when there is a store (`resume`: going on from
/// what the process before saved), until the match ends. The feed from before is not recorded
/// again as new.
pub(super) fn record(s: &Arc<Session>, store: Option<&Store>, resume: bool) {
    let name = live_file(&s.id);
    let resumed =
        store.filter(|_| resume).and_then(|store| Recording::resume(store.read(&name).ok()?.as_slice(), to_file(store, &name)).ok());
    let fresh = resumed.is_none();
    let recording = Arc::new(resumed.unwrap_or_else(|| match store {
        None => Recording::live(),
        Some(store) => {
            store.forget(&name);
            Recording::live_to(to_file(store, &name))
        }
    }));
    // The demos' loads go on from the recording's; a fresh one's from theirs.
    if let (Some(demos), false) = (s.demos.get(), fresh) {
        let first = recording.parts().first().map_or(0, |p| p.n);
        demos.go_on_from(recording.loads(), first);
        demos.keep_from(first);
    }
    let _ = s.live.set(recording.clone());
    // Everything: detail, voice, and what is only kept.
    let (lines, s) = (subscribe_all(&s.hub), s.clone());
    std::thread::spawn(move || pump(&s, &lines, &recording, fresh));
}

/// Takes the match's lines into its recording, and its demos' loads.
fn pump(s: &Session, lines: &std::sync::mpsc::Receiver<Arc<str>>, recording: &Recording, mut fresh: bool) {
    let mut chunks = recording.chunks();
    while s.is_live() && !s.stop.load(Ordering::Relaxed) {
        let json = match lines.recv_timeout(RECORD_POLL) {
            Ok(json) => json,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if json.contains(r#""history":true"#) {
            continue;
        }
        let Ok(mut stamped) = serde_json::from_str::<stvtf::output::Stamped>(&json) else { continue };
        stamped.time = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64));
        let demos = s.demos.get();
        // A fresh recording's first part is the load the match is at.
        if let Some(n) = demos.and_then(|d| d.line(&stamped.line))
            && std::mem::take(&mut fresh)
        {
            recording.first_load_is(n);
        }
        let _ = recording.push(&stamped, &json);
        // A chunk closed, maybe trimming parts: their demos go.
        if let Some(demos) = demos
            && recording.chunks() != chunks
        {
            chunks = recording.chunks();
            demos.keep_from(recording.parts().first().map_or(0, |p| p.n));
        }
    }
    recording.end();
}
