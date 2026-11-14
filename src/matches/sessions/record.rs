//! A live match's recorder: its lines into its recording, and its demos' loads.
//!
//! The recording's file has one writer at a time. A process that picks a match up reads the file
//! and writes nothing (its writer, the store another's now, closes its open chunk into it every
//! [`BEAT`] for [`READ_ON_FOR`]): once its own stream has a tick it asks for the file
//! (`live-<id>.take`, its mark and that tick). The process that writes it writes on until its own
//! stream is at that tick (a new connection's first frame is ahead of the stream), for
//! [`GIVE_BY`] at most, saying every [`RECORD_POLL`] that it does (`live-<id>.beat`) and closing
//! its open chunk into the file every [`BEAT`]; then it closes the last, writes no more and says
//! so (`live-<id>.given`). It does the same as it stops, asked or not. The new one reads the file
//! on as it grows, goes on with its own lines from the tick after the file's last, and writes
//! from there. A file whose writer says nothing for [`GIVE_WAIT`] after the asking (it is gone,
//! or of a version that gives at once) is taken as it is, as is any [`TAKE_BY`] after. A process
//! that gave its file and still runs when the one that took it vanishes writes it again, whole.

use super::session::Session;
use super::store::{LIVE_PREFIX, Store, live_file, to_file};
use crate::lock;
use crate::matches::hub::subscribe_all;
use crate::matches::recording::Recording;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};
use stvtf::Line;
use stvtf::output::Stamped;

/// How long the recorder waits for a line before it looks whether to stop, and how often it looks
/// whether its file is asked for.
pub(super) const RECORD_POLL: Duration = Duration::from_millis(if cfg!(test) { 20 } else { 250 });
/// How long a process that asked for a file waits for its writer's word: that it is given, or
/// that it writes it still.
pub(super) const GIVE_WAIT: Duration = Duration::from_millis(if cfg!(test) { 300 } else { 3000 });
/// A file asked for longer ago than this, its writer not having said since that it writes it, is
/// not written to, even to close it: the one that asked may have taken it.
const GIVE_WITHIN: Duration = Duration::from_millis(GIVE_WAIT.as_millis() as u64 / 2);
/// How long a file asked for is written on at most, its stream not yet at the tick asked at: a new
/// connection's first frame was seen some 11 s ahead of the stream.
pub(super) const GIVE_BY: Duration = Duration::from_millis(if cfg!(test) { 1500 } else { 20_000 });
/// How long after asking a file is taken, whatever its writer says.
const TAKE_BY: Duration = Duration::from_millis((GIVE_BY.as_millis() + GIVE_WAIT.as_millis()) as u64);
/// How often the process that writes a file says it still does, and how long after it last said
/// so it is gone: another that has the recording writes it again.
const BEAT: Duration = Duration::from_millis(if cfg!(test) { 40 } else { 1000 });
const GONE_AFTER: Duration = Duration::from_millis(if cfg!(test) { 400 } else { 10_000 });
/// How long after another process took the store a writer closes a chunk into its file at every
/// beat, for that one to read on: longer than a connection takes to have a stream (22 s seen).
const READ_ON_FOR: Duration = Duration::from_secs(60);
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

/// Touched by the process that writes match `id`'s file, every [`BEAT`].
pub(super) fn beat_file(id: &str) -> String {
    format!("{}.beat", live_file(id).trim_end_matches(".gz"))
}

/// What a `.take` says after its mark once the file is taken: no tick to write on to.
const TAKEN: &str = "taken";

/// Another process's word in a `.take`.
struct Asked {
    /// When it was written, and how long ago.
    at: SystemTime,
    ago: Duration,
    /// The tick its stream began at; none from a version before.
    tick: Option<u32>,
    /// It has the file already.
    taken: bool,
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

    /// The file is this process's, to write: asked for with its stream at `tick`, or taken.
    fn ask(&self, tick: Option<u32>) {
        let word = tick.map_or_else(|| TAKEN.to_string(), |t| t.to_string());
        if let Err(e) = self.store.replace(&take_file(&self.id), format!("{} {word}", self.mark).as_bytes()) {
            log!("asking for match {}'s recording: {e:#}", self.id);
        }
    }

    /// What another process said in the `.take`, if one did.
    fn asked(&self) -> Option<Asked> {
        let name = take_file(&self.id);
        let said = String::from_utf8(self.store.read(&name).ok()?).ok()?;
        let mut words = said.split_whitespace();
        if words.next()? == self.mark {
            return None;
        }
        let word = words.next();
        let at = self.store.written(&name)?;
        let ago = SystemTime::now().duration_since(at).unwrap_or_default();
        Some(Asked { at, ago, tick: word.and_then(|w| w.parse().ok()), taken: word == Some(TAKEN) })
    }

    /// When the process that writes the file last said so, if not before `since`.
    fn beat_since(&self, since: SystemTime) -> Option<SystemTime> {
        self.store.written(&beat_file(&self.id)).filter(|at| *at >= since)
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

    /// Says this process writes the file still.
    fn beat(&self) {
        let _ = self.store.replace(&beat_file(&self.id), self.mark.as_bytes());
    }

    /// Whether the process that took the file is gone without a word: it is named as its writer,
    /// and has not said so for [`GONE_AFTER`].
    fn writer_gone(&self) -> bool {
        let theirs = self.asked().is_some();
        let said = self.store.written(&beat_file(&self.id)).or_else(|| self.store.written(&take_file(&self.id)));
        theirs && said.is_some_and(|at| SystemTime::now().duration_since(at).unwrap_or_default() > GONE_AFTER)
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
        for name in [live_file(&self.id), take_file(&self.id), given_file(&self.id), beat_file(&self.id)] {
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
    /// When it asked for the file, and when its `.take` says it was written.
    asked: Option<(Instant, SystemTime)>,
    /// When it last looked whether the file was given.
    looked: Option<Instant>,
    /// Its writer gave it: closed, and current as far as that one had the stream.
    given: bool,
}

/// How much of a file's start is kept, to tell whether it was written anew.
const HEAD: usize = 64;

impl Taking {
    /// Reads the file as it is now into a recording, which the lines to come go on from.
    fn begin(file: &LiveFile) -> (Recording, Taking) {
        let data = file.store.read(&live_file(&file.id)).unwrap_or_default();
        let recording = Recording::live();
        match recording.read_on(&data) {
            Ok(read) => {
                recording.set_catching_up(true);
                (recording, Taking { read: Some((read, data[..read.min(HEAD)].to_vec())), ..Taking::default() })
            }
            Err(e) => {
                log!("match {}'s recording does not read, and starts anew: {e:#}", file.id);
                (Recording::live(), Taking::default())
            }
        }
    }

    /// A line of this process's own stream: held, and the first with a tick asks for the file.
    fn hold(&mut self, file: &LiveFile, stamped: Stamped, json: Arc<str>) {
        if let (None, Some(tick)) = (self.asked, stamped.line.tick().filter(|t| *t > 0)) {
            file.ask(Some(tick));
            let at = file.store.written(&take_file(&file.id)).unwrap_or_else(SystemTime::now);
            self.asked = Some((Instant::now(), at));
        }
        self.held.push((stamped, json));
    }

    /// Takes in what was added to the file since it was last read. False if it does not read to
    /// its end.
    fn read_on(&mut self, file: &LiveFile, recording: &Recording) -> bool {
        let Some((read, head)) = &mut self.read else { return false };
        let Ok((from, data)) = file.store.read_on(&live_file(&file.id), *read, head) else { return false };
        // Written anew meanwhile: read from its start, what is known skipped.
        if from == 0 {
            *head = data[..data.len().min(HEAD)].to_vec();
        }
        let more = if data.is_empty() { Ok(0) } else { recording.read_on(&data) };
        *read = from + more.as_ref().map_or(0, |more| *more);
        more.is_ok_and(|more| more == data.len())
    }

    /// Whether the file can be taken: given, or asked for and its writer silent for long enough
    /// since, or asked for too long ago.
    fn may_take(&mut self, file: &LiveFile) -> bool {
        if file.take_given() {
            self.given = true;
            return true;
        }
        // Until it asked, the file is given only as its writer stops.
        let Some((asked, at)) = self.asked else { return false };
        let silent = file.beat_since(at).map_or_else(|| asked.elapsed(), |beat| SystemTime::now().duration_since(beat).unwrap_or_default());
        silent >= GIVE_WAIT || asked.elapsed() >= TAKE_BY
    }

    /// Takes the file: what was added to it since it was read, then this process's own lines,
    /// which `recording` writes from now on. Of a file written since this process's stream began,
    /// the lines held up to its last tick are in it; the tick a line is then to be past before
    /// those behind it count again. The statuses of its own connecting are not the match's: a
    /// recording that goes on has none of them.
    fn take(mut self, file: &LiveFile, recording: &Recording, interval: f64) -> Option<u32> {
        let name = live_file(&file.id);
        // Said first: its writer, late, writes no more to it.
        file.ask(None);
        // Given, or written since this process asked: the file is of the stream held. (Given with
        // nothing more written: a paused game.)
        let written = file.store.written(&name);
        let since = self.given || self.asked.is_some_and(|(_, asked)| written.is_some_and(|at| at >= asked));
        // Torn, or not read: this process's is the file.
        if !self.read_on(file, recording) {
            recording.rewrite();
        }
        recording.write_to(to_file(&file.store, &name));
        if recording.count() > 0 {
            let ticked = self.held.iter().position(|(s, _)| s.line.tick().is_some_and(|t| t > 0));
            let stream = self.held.split_off(ticked.unwrap_or(self.held.len()));
            self.held.retain(|(s, _)| !matches!(s.line, Line::Status(_)));
            self.held.extend(stream);
        }
        // A stream's ticks do not only go up (a snapshot is of a few ticks back): every line
        // held up to the file's last tick is in it, wherever it stands.
        let last =
            recording.last_tick().filter(|last| since && of_the_stream(self.held.iter().map(|(s, _)| s.line.tick()), *last, interval));
        if let Some(last) = last {
            self.held.retain(|(s, _)| !s.line.tick().is_some_and(|t| t > 0 && t <= last));
        }
        for (stamped, json) in self.held.drain(..) {
            let _ = recording.push(&stamped, &json);
        }
        recording.set_catching_up(false);
        last
    }
}

/// Whether a file whose last tick is `last` is of the stream held, by its `ticks`: within
/// [`AHEAD_S`] (of `interval` seconds a tick) of them. Not of a stream that began well after it (a
/// hole), or whose ticks started over (a map loaded again).
fn of_the_stream(ticks: impl Iterator<Item = Option<u32>>, last: u32, interval: f64) -> bool {
    let ticks: Vec<u32> = ticks.flatten().filter(|t| *t > 0).collect();
    let (Some(lowest), Some(latest)) = (ticks.iter().min(), ticks.iter().max()) else { return false };
    let ahead = (AHEAD_S / interval.max(1e-6)).round() as u32;
    *lowest <= last.saturating_add(ahead) && last <= latest.saturating_add(ahead)
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
    std::thread::spawn(move || {
        let now = Instant::now();
        Pump { file, taking, giving: None, behind: None, fresh, looked: now, beat: now, taken_over: None }.run(&s, &lines, &recording);
    });
}

/// Ticks a file is written on past the tick asked at: 3 s of a 66.67-tick server's.
const PAST: u32 = if cfg!(test) { 0 } else { 200 };

/// A file asked for, written on until its stream is at the tick asked at (none: at once).
struct Giving {
    tick: Option<u32>,
    /// When the `.take` that asked was written: another since is the file taken.
    asked: SystemTime,
    since: Instant,
    /// When it last said it writes the file still, and when it last closed a chunk into it.
    beat: Instant,
    flushed: Option<Instant>,
}

impl Giving {
    /// Writes the file on: says so, and closes the open chunk into it every [`BEAT`]. False once
    /// it does no more: given, its stream at the tick or [`GIVE_BY`] up, or let go, taken
    /// meanwhile (or it may be).
    fn writes_on(&mut self, file: &LiveFile, recording: &Recording, asked: Option<&Asked>) -> bool {
        if asked.is_none_or(|a| a.at != self.asked) || self.beat.elapsed() >= GIVE_WITHIN {
            recording.let_go();
            return false;
        }
        // Some past it: a new connection's first snapshots are sparse.
        let there = self.tick.is_none_or(|tick| recording.last_tick().is_none_or(|last| last >= tick.saturating_add(PAST)));
        if there || self.since.elapsed() >= GIVE_BY {
            file.give(recording);
            return false;
        }
        self.beat = Instant::now();
        file.beat();
        if self.flushed.is_none_or(|t| t.elapsed() >= BEAT) {
            self.flushed = Some(Instant::now());
            recording.flush();
        }
        true
    }
}

/// What takes a match's lines into its recording.
struct Pump {
    file: Option<LiveFile>,
    /// While the file is another process's.
    taking: Option<Taking>,
    /// While the file is asked for and not given yet.
    giving: Option<Giving>,
    /// The file's last tick when it was taken, and when that was, until this process's stream
    /// is past it: lines before it are in the file.
    behind: Option<(u32, u32, Instant)>,
    /// A fresh recording's first part is the load the match is at.
    fresh: bool,
    /// When it last looked whether its file is asked for.
    looked: Instant,
    /// When it last said it writes its file.
    beat: Instant,
    /// When it first saw the store another process's.
    taken_over: Option<Instant>,
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
        s.giving.store(false, Ordering::Relaxed);
        recording.end();
    }

    /// Takes the file once it may, gives it once another process asks for it and its stream is at
    /// the tick asked at, or takes it back once that one stops.
    fn hand_over(&mut self, s: &Session, recording: &Recording) {
        let Some(file) = &self.file else { return };
        if let Some(taking) = &mut self.taking {
            if taking.looked.is_some_and(|t| t.elapsed() < RECORD_POLL) {
                return;
            }
            taking.looked = Some(Instant::now());
            // What its writer added meanwhile plays here too.
            if taking.may_take(file) {
                let interval = lock(&s.hub).tick_interval();
                let last = self.taking.take().and_then(|taking| taking.take(file, recording, interval));
                let ahead = (AHEAD_S / interval.max(1e-6)).round() as u32;
                self.behind = last.map(|tick| (tick.saturating_sub(ahead), tick, Instant::now()));
            } else {
                taking.read_on(file, recording);
            }
            return;
        }
        if self.looked.elapsed() < RECORD_POLL {
            return;
        }
        self.looked = Instant::now();
        if !recording.is_written() {
            // The process that took the file stopped, or is gone: this one has it all, and writes
            // it again.
            if file.take_given() || file.writer_gone() {
                file.ask(None);
                recording.rewrite();
                recording.write_to(to_file(&file.store, &live_file(&file.id)));
            }
            return;
        }
        let asked = file.asked();
        if let Some(giving) = &mut self.giving {
            if !giving.writes_on(file, recording, asked.as_ref()) {
                self.giving = None;
                s.giving.store(false, Ordering::Relaxed);
            }
            return;
        }
        match asked {
            Some(Asked { at, ago, tick, taken: false }) if ago < GIVE_WITHIN => {
                let now = Instant::now();
                let mut giving = Giving { tick, asked: at, since: now, beat: now, flushed: None };
                if giving.writes_on(file, recording, asked.as_ref()) {
                    self.giving = Some(giving);
                    s.giving.store(true, Ordering::Relaxed);
                }
            }
            Some(_) => recording.let_go(),
            None if self.beat.elapsed() >= BEAT => {
                self.beat = Instant::now();
                file.beat();
                // The process the store is now reads the file on until it asks for it.
                let taken_over = file.store.taken_over(&file.mark).then(|| *self.taken_over.get_or_insert_with(Instant::now));
                if taken_over.is_some_and(|since| since.elapsed() < READ_ON_FOR) {
                    recording.flush();
                }
            }
            None => {}
        }
    }

    /// Whether a line at `tick` is one the file taken has: up to its last tick and not far before
    /// it (lower ticks still are a map loaded again), for [`AHEAD_S`] after the take.
    fn is_behind(&mut self, tick: Option<u32>) -> bool {
        let (Some((from, last, since)), Some(tick)) = (self.behind, tick.filter(|t| *t > 0)) else { return false };
        if since.elapsed() > Duration::from_secs_f64(AHEAD_S) {
            self.behind = None;
            return false;
        }
        (from..=last).contains(&tick)
    }
}

#[cfg(test)]
mod tests {
    use super::of_the_stream;

    fn held(ticks: &[u32]) -> impl Iterator<Item = Option<u32>> + '_ {
        ticks.iter().map(|t| Some(*t).filter(|t| *t > 0))
    }

    #[test]
    fn a_file_is_of_the_stream_held_when_its_last_tick_is_near_the_stream_s() {
        // A status and a server line, then ticks: a 66.67-tick server's, 15 ms each.
        let stream = [0, 0, 500, 500, 533, 566, 566, 600];
        assert!(of_the_stream(held(&stream), 566, 0.015));
        assert!(of_the_stream(held(&stream), 700, 0.015), "the file is ahead: what comes is looked at too");
        assert!(of_the_stream(held(&[0, 620, 566, 600]), 580, 0.015), "a snapshot of some ticks back after an event");
        assert!(!of_the_stream(held(&stream), 100, 0.015), "a stream begun well after it: a hole");
        assert!(!of_the_stream(held(&stream), 90_000, 0.015), "ticks that started over: a new load");
        assert!(!of_the_stream(held(&[0, 0]), 566, 0.015), "no stream yet");
    }
}
