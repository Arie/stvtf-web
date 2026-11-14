//! A live match's recorder: its lines into its recording, and its demos' loads.
//!
//! The recording's file has one writer at a time. A process that picks a match up reads the file
//! and writes nothing: once its own stream has a tick it asks for the file (`live-<id>.take`,
//! its mark). The process that writes it then closes its open chunk into it, writes no more and
//! says so (`live-<id>.given`); it does the same as it stops, asked or not. The new one reads what
//! was added, goes on with its own lines from the tick after the file's last, and writes from
//! there. A file not given within [`GIVE_WAIT`] of the asking (its writer is gone) is taken as it
//! is. A process that gave its file and still runs when the one that took it stops (a deploy
//! that failed) writes it again, whole.

use super::session::Session;
use super::store::{LIVE_PREFIX, Store, live_file, to_file};
use crate::lock;
use crate::matches::hub::subscribe_all;
use crate::matches::recording::Recording;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};
use stvtf::output::Stamped;

/// How long the recorder waits for a line before it looks whether to stop, and how often it looks
/// whether its file is asked for.
pub(super) const RECORD_POLL: Duration = Duration::from_millis(if cfg!(test) { 20 } else { 250 });
/// How long a process that asked for a file waits for it to be given.
pub(super) const GIVE_WAIT: Duration = Duration::from_millis(if cfg!(test) { 300 } else { 3000 });
/// A file asked for longer ago than this is not written to, even to close it: the one that asked
/// may have taken it.
const GIVE_WITHIN: Duration = Duration::from_millis(GIVE_WAIT.as_millis() as u64 / 2);
/// Seconds of ticks the file's last tick may be past the stream held and still be of it.
const AHEAD_S: f64 = 5.0;

/// Where the process that wants match `id`'s file leaves its mark, and stays named while it
/// writes it.
pub(super) fn take_file(id: &str) -> String {
    format!("{LIVE_PREFIX}{id}.take")
}

/// There once match `id`'s file is closed and nobody writes it: the mark of the process that
/// closed it.
pub(super) fn given_file(id: &str) -> String {
    format!("{LIVE_PREFIX}{id}.given")
}

/// A live match's file in the store, for the process with `mark`.
pub(super) struct LiveFile {
    store: Store,
    id: String,
    mark: String,
}

impl LiveFile {
    pub(super) fn new(store: &Store, id: &str, mark: &str) -> LiveFile {
        LiveFile { store: store.clone(), id: id.to_string(), mark: mark.to_string() }
    }

    /// The file is this process's, to write.
    fn ask(&self) {
        if let Err(e) = self.store.replace(&take_file(&self.id), self.mark.as_bytes()) {
            log!("asking for match {}'s recording: {e:#}", self.id);
        }
    }

    /// How long ago another process asked for the file, if one did.
    fn asked(&self) -> Option<Duration> {
        let name = take_file(&self.id);
        let by = self.store.read(&name).ok()?;
        if by == self.mark.as_bytes() {
            return None;
        }
        let at = self.store.written(&name)?;
        Some(SystemTime::now().duration_since(at).unwrap_or_default())
    }

    /// Whether another process closed the file and this one is the one to have it: the first
    /// that takes its word away.
    fn take_given(&self) -> bool {
        let name = given_file(&self.id);
        if self.store.read(&name).map_or(true, |by| by == self.mark.as_bytes()) {
            return false;
        }
        let mine = format!("{name}.{}", self.mark);
        let got = std::fs::rename(self.store.path(&name), self.store.path(&mine)).is_ok();
        self.store.forget(&mine);
        got
    }

    /// Closes `recording` into the file, if this process writes it, and says nobody does now.
    pub(super) fn give(&self, recording: &Recording) {
        if recording.give()
            && let Err(e) = self.store.replace(&given_file(&self.id), self.mark.as_bytes())
        {
            log!("giving match {}'s recording: {e:#}", self.id);
        }
    }

    /// Forgets the file and what was said of it.
    pub(super) fn forget(&self) {
        for name in [live_file(&self.id), take_file(&self.id), given_file(&self.id)] {
            self.store.forget(&name);
        }
    }
}

/// A match picked up, until its file is this process's.
#[derive(Default)]
struct Taking {
    /// How much of the file is read, and how it began then; none of a file that did not read.
    read: Option<(usize, Vec<u8>)>,
    /// This process's own stream meanwhile.
    held: Vec<(Stamped, Arc<str>)>,
    /// When it asked for the file, by both clocks.
    asked: Option<(Instant, SystemTime)>,
    /// When it last looked whether the file was given.
    looked: Option<Instant>,
}

/// How much of a file's start is kept, to tell whether it was written anew.
const HEAD: usize = 64;

impl Taking {
    /// Reads the file as it is now into a recording, which the lines to come go on from.
    fn begin(file: &LiveFile) -> (Recording, Taking) {
        let data = file.store.read(&live_file(&file.id)).unwrap_or_default();
        let recording = Recording::live();
        match recording.read_on(&data) {
            Ok(read) => (recording, Taking { read: Some((read, data[..read.min(HEAD)].to_vec())), ..Taking::default() }),
            Err(e) => {
                log!("match {}'s recording does not read, and starts anew: {e:#}", file.id);
                (Recording::live(), Taking::default())
            }
        }
    }

    /// A line of this process's own stream: held, and the first with a tick asks for the file.
    fn hold(&mut self, file: &LiveFile, stamped: Stamped, json: Arc<str>) {
        if self.asked.is_none() && stamped.line.tick().is_some_and(|t| t > 0) {
            file.ask();
            self.asked = Some((Instant::now(), SystemTime::now()));
        }
        self.held.push((stamped, json));
    }

    /// Whether the file can be taken: given, or asked for long enough ago.
    fn may_take(&mut self, file: &LiveFile) -> bool {
        if self.asked.is_some_and(|(t, _)| t.elapsed() >= GIVE_WAIT) {
            return true;
        }
        // Until it asked, the file is given only as its writer stops.
        if self.asked.is_none() && self.looked.is_some_and(|t| t.elapsed() < RECORD_POLL) {
            return false;
        }
        self.looked = Some(Instant::now());
        file.take_given()
    }

    /// Takes the file: what was added to it since it was read, then this process's own lines,
    /// which `recording` writes from now on. Of a file written since this process's stream began,
    /// the lines held up to its last tick are in it; the tick a line is then to be past before
    /// those behind it count again.
    fn take(mut self, file: &LiveFile, recording: &Recording, interval: f64) -> Option<u32> {
        let name = live_file(&file.id);
        file.ask();
        let written = file.store.written(&name);
        let since = self.asked.is_some_and(|(_, asked)| written.is_some_and(|at| at > asked));
        let data = file.store.read(&name).unwrap_or_default();
        let whole = self.read.take().is_some_and(|(read, head)| {
            // Written anew meanwhile: read from its start, what is known skipped.
            let from = if data.len() >= read && data.starts_with(&head) { read } else { 0 };
            recording.read_on(&data[from..]).is_ok_and(|more| from + more == data.len())
        });
        // Torn, or not read: this process's is the file.
        if !whole {
            recording.rewrite();
        }
        recording.write_to(to_file(&file.store, &name));
        let last = recording.last_tick().filter(|_| since);
        let had = last.map_or(0, |last| in_file_already(self.held.iter().map(|(s, _)| s.line.tick()), last, interval));
        for (stamped, json) in self.held.drain(had..) {
            let _ = recording.push(&stamped, &json);
        }
        last.filter(|_| had > 0)
    }
}

/// How many of the lines held, by their `ticks`, a file whose last tick is `last` has already: up
/// to the latest at or before it, if `last` is a tick of the stream held, or within [`AHEAD_S`]
/// of its end (of `interval` seconds a tick). None of a stream that began after it, or whose
/// ticks started over.
fn in_file_already(ticks: impl Iterator<Item = Option<u32>>, last: u32, interval: f64) -> usize {
    let ticks: Vec<u32> = ticks.map(|t| t.unwrap_or(0)).collect();
    let mut ticked = ticks.iter().copied().filter(|t| *t > 0);
    let (Some(first), Some(latest)) = (ticked.next(), ticks.iter().copied().rfind(|t| *t > 0)) else { return 0 };
    let ahead = (AHEAD_S / interval.max(1e-6)).round() as u32;
    if first > last || last > latest.saturating_add(ahead) {
        return 0;
    }
    ticks.iter().rposition(|t| *t > 0 && *t <= last).map_or(0, |i| i + 1)
}

/// Records live match `s` from now on, to its file when there is a store, as the process with
/// `mark` (`resume`: going on from what the process before saved, see [`Taking`]), until the
/// match ends. The feed from before is not recorded again as new.
pub(super) fn record(s: &Arc<Session>, store: Option<&Store>, mark: &str, resume: bool) {
    let file = store.map(|store| LiveFile::new(store, &s.id, mark));
    let (recording, taking) = match &file {
        None => (Recording::live(), None),
        Some(file) if resume => {
            let (recording, taking) = Taking::begin(file);
            (recording, Some(taking))
        }
        Some(file) => {
            file.forget();
            (Recording::live_to(to_file(&file.store, &live_file(&s.id))), None)
        }
    };
    let fresh = recording.count() == 0;
    let recording = Arc::new(recording);
    // The demos' loads go on from the recording's; a fresh one's from theirs.
    if let (Some(demos), false) = (s.demos.get(), fresh) {
        let first = recording.parts().first().map_or(0, |p| p.load);
        demos.go_on_from(recording.loads());
        demos.keep_from(first);
    }
    let _ = s.live.set(recording.clone());
    // Everything: detail, voice, and what is only kept.
    let (lines, s) = (subscribe_all(&s.hub), s.clone());
    std::thread::spawn(move || Pump { file, taking, behind: None, fresh, looked: Instant::now() }.run(&s, &lines, &recording));
}

/// What takes a match's lines into its recording.
struct Pump {
    file: Option<LiveFile>,
    /// While the file is another process's.
    taking: Option<Taking>,
    /// The file's last tick when it was taken, and when that was, until this process's stream
    /// is past it: lines before it are in the file.
    behind: Option<(u32, Instant)>,
    /// A fresh recording's first part is the load the match is at.
    fresh: bool,
    /// When it last looked whether its file is asked for.
    looked: Instant,
}

impl Pump {
    /// Takes the match's lines into its recording, and its demos' loads.
    fn run(mut self, s: &Session, lines: &std::sync::mpsc::Receiver<Arc<str>>, recording: &Recording) {
        let mut chunks = recording.chunks();
        while s.is_live() && !s.stop.load(Ordering::Relaxed) {
            let line = lines.recv_timeout(RECORD_POLL);
            if matches!(line, Err(std::sync::mpsc::RecvTimeoutError::Disconnected)) {
                break;
            }
            self.hand_over(s, recording);
            let Ok(json) = line else { continue };
            if json.contains(r#""history":true"#) {
                continue;
            }
            let Ok(mut stamped) = serde_json::from_str::<Stamped>(&json) else { continue };
            stamped.time = Some(SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64));
            let demos = s.demos.get();
            if let Some(n) = demos.and_then(|d| d.note_line(&stamped.line))
                && std::mem::take(&mut self.fresh)
            {
                recording.first_load_is(n);
            }
            if let (Some(taking), Some(file)) = (&mut self.taking, &self.file) {
                taking.hold(file, stamped, json);
                continue;
            }
            if self.is_behind(stamped.line.tick()) {
                continue;
            }
            let _ = recording.push(&stamped, &json);
            // A chunk closed, maybe trimming parts: their demos go.
            if let Some(demos) = demos
                && recording.chunks() != chunks
            {
                chunks = recording.chunks();
                demos.keep_from(recording.parts().first().map_or(0, |p| p.load));
            }
        }
        recording.end();
    }

    /// Takes the file once it may, gives it once another process asks for it, or takes it back
    /// once that one stops.
    fn hand_over(&mut self, s: &Session, recording: &Recording) {
        let Some(file) = &self.file else { return };
        if let Some(taking) = &mut self.taking {
            if taking.may_take(file) {
                let interval = lock(&s.hub).tick_interval();
                let last = self.taking.take().and_then(|taking| taking.take(file, recording, interval));
                self.behind = last.map(|tick| (tick, Instant::now()));
            }
            return;
        }
        if self.looked.elapsed() < RECORD_POLL {
            return;
        }
        self.looked = Instant::now();
        if !recording.is_written() {
            // The process that took the file stopped: this one has it all, and writes it again.
            if file.take_given() {
                file.ask();
                recording.rewrite();
                recording.write_to(to_file(&file.store, &live_file(&file.id)));
            }
            return;
        }
        match file.asked() {
            Some(ago) if ago < GIVE_WITHIN => file.give(recording),
            Some(_) => recording.let_go(),
            None => {}
        }
    }

    /// Whether a line at `tick` is before the tick the file was taken at, its stream not yet past
    /// it: the file has it. Not for long: lower ticks that go on are a map loaded again.
    fn is_behind(&mut self, tick: Option<u32>) -> bool {
        let (Some((last, since)), Some(tick)) = (self.behind, tick.filter(|t| *t > 0)) else { return false };
        if tick > last || since.elapsed() > Duration::from_secs_f64(AHEAD_S) {
            self.behind = None;
        }
        self.behind.is_some() && tick < last
    }
}

#[cfg(test)]
mod tests {
    use super::in_file_already;

    fn held(ticks: &[u32]) -> impl Iterator<Item = Option<u32>> + '_ {
        ticks.iter().map(|t| Some(*t).filter(|t| *t > 0))
    }

    #[test]
    fn the_lines_held_up_to_the_file_s_last_tick_are_in_it_already() {
        // A status and a server line, then ticks: a 66.67-tick server's, 15 ms each.
        let stream = [0, 0, 500, 500, 533, 566, 566, 600];
        assert_eq!(in_file_already(held(&stream), 566, 0.015), 7);
        assert_eq!(in_file_already(held(&stream), 540, 0.015), 5, "a tick of the file's own between two held");
        assert_eq!(in_file_already(held(&stream), 700, 0.015), 8, "the file is ahead: what comes is looked at too");
        assert_eq!(in_file_already(held(&stream), 499, 0.015), 0, "a stream begun after it: a hole, all of it kept");
        assert_eq!(in_file_already(held(&stream), 90_000, 0.015), 0, "ticks that started over: a new load");
        assert_eq!(in_file_already(held(&[0, 0]), 566, 0.015), 0, "no stream yet");
    }
}
