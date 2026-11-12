//! The player: a room's recording played in time, paused, sped up and sought.

use super::{Room, SPEEDS};
use crate::lock;
use crate::matches::hub::{announce, publish, restore};
use crate::matches::recording::Recording;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::MutexGuard;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use stvtf::Line;

/// How often pages hear where playback is while it plays.
pub(super) const ANNOUNCE_EVERY: Duration = Duration::from_secs(1);
/// How often a room nobody watches looks again, and one at a live match's end for more of it.
pub(super) const IDLE_POLL: Duration = Duration::from_millis(250);
pub(super) const LIVE_POLL: Duration = Duration::from_millis(20);

/// Where playback is in the recording: the chunk, how many of its lines are played, and the lines
/// after, as far as the recording had them when last looked at (`looked`: how many it had then).
#[derive(Default)]
struct Cursor {
    chunk: usize,
    done: usize,
    looked: Option<usize>,
    lines: VecDeque<(u64, Line)>,
}

impl Cursor {
    /// More of this chunk, if a live one grew; else on to the next.
    fn refill(&mut self, rec: &Recording) {
        if self.lines.is_empty() && self.looked != Some(rec.count()) {
            self.looked = Some(rec.count());
            self.lines = rec.chunk(self.chunk).unwrap_or_default().into_iter().skip(self.done).collect();
        }
        while self.lines.is_empty() && self.chunk + 1 < rec.chunks() {
            (self.chunk, self.done) = (self.chunk + 1, 0);
            self.lines = rec.chunk(self.chunk).unwrap_or_default().into();
        }
    }
}

pub(super) struct State {
    pub(super) paused: bool,
    pub(super) speed: f64,
    /// Play time, in milliseconds.
    pub(super) at: u64,
    pub(super) seek: Option<u64>,
    pub(super) stop: bool,
    /// The server tick of the last snapshot played, since the last seek.
    pub(super) tick: Option<u32>,
    /// The map of the match it plays, by number.
    pub(super) part: usize,
    /// It goes on to the next map by itself: it was at live when its map ended, and nobody did
    /// anything in it since.
    pub(super) follows: bool,
}

/// What an owner changes; unset fields stay.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Control {
    pub(crate) paused: Option<bool>,
    pub(crate) speed: Option<f64>,
    pub(crate) seek: Option<u64>,
}

/// Where playback is as time passes, from where it was last set down: at `at` milliseconds into
/// the match, at `speed`. Each wait measured by itself would lose what passes between waits (a
/// long sleep, the lines sent in between), and a room would fall behind the match it plays.
#[derive(Debug, Clone, Copy)]
pub(super) struct Clock {
    from: Instant,
    at: u64,
    speed: f64,
}

impl Clock {
    pub(super) fn new(from: Instant, at: u64, speed: f64) -> Self {
        Clock { from, at, speed }
    }

    /// When playback is at `t`.
    pub(super) fn due(&self, t: u64) -> Instant {
        self.from + wait(t.saturating_sub(self.at), self.speed)
    }

    /// Where playback is at `now`.
    pub(super) fn at(&self, now: Instant) -> u64 {
        self.at + (now.saturating_duration_since(self.from).as_secs_f64() * 1000.0 * self.speed).round() as u64
    }
}

/// Real time to wait out `gap` milliseconds of play at `speed`.
pub(super) fn wait(gap: u64, speed: f64) -> Duration {
    Duration::from_secs_f64(gap as f64 / 1000.0 / speed)
}

impl Room {
    pub(crate) fn control(&self, c: &Control) -> Result<()> {
        if let Some(speed) = c.speed
            && !SPEEDS.contains(&speed)
        {
            bail!("speed {speed} is not one of {SPEEDS:?}");
        }
        let mut st = lock(&self.state);
        st.follows = false;
        if let Some(speed) = c.speed {
            st.speed = speed;
        }
        if let Some(paused) = c.paused {
            st.paused = paused;
        }
        if let Some(at) = c.seek {
            st.seek = Some(self.held(&st, at));
        }
        self.announce(&st);
        self.wake.notify_all();
        Ok(())
    }

    pub(super) fn announce(&self, st: &State) {
        let (part, over) = (self.part(st), self.part_over(st));
        let maps: Vec<_> = self.recording.parts().iter().map(|p| serde_json::json!({ "n": p.n, "map": p.map, "ready": p.ready })).collect();
        let line = serde_json::json!({
            "type": "playback",
            "paused": st.paused,
            "speed": st.speed,
            "at": st.at,
            "length": part.end.unwrap_or_else(|| self.recording.length()),
            // What is kept of its map (from `start`), and where the map began.
            "start": part.start,
            "origin": part.origin,
            "rounds": self.recording.rounds_in(part.n),
            "live": self.recording.is_live() && !over,
            // Its map's number, whether the match went on to another, and the maps kept.
            "map": part.n,
            "over": over,
            "maps": maps,
            // Marks so far: a page fetches the ones it lacks.
            "marks": self.recording.marks(usize::MAX).1,
        });
        announce(&self.hub, "playback", &line.to_string().into());
        self.follow(st);
    }

    /// The player: until stopped, plays the lines in time, seeking and pausing when told. At the
    /// end of a live match's recording it waits for more; at the end of any other it pauses.
    pub(super) fn play(&self) {
        let rec = &self.recording;
        let mut cur = Cursor::default();
        let mut st = lock(&self.state);
        self.announce(&st);
        let mut announced = Instant::now();
        // The clock playing goes by, while it plays on undisturbed: a seek, a pause, the end of
        // what is recorded or another speed sets it down anew.
        let mut clock: Option<Clock> = None;
        loop {
            if st.stop {
                return;
            }
            // A live match goes on, paused or not: pages hear how long it is now.
            if rec.is_live() && announced.elapsed() >= ANNOUNCE_EVERY {
                self.announce_now(&st, &mut announced);
            }
            self.rehome(&mut st);
            let (start, _) = self.reach(&st);
            if st.seek.is_none() && st.at < start {
                st.seek = Some(start);
            }
            if let Some(target) = st.seek.take() {
                self.seek_to(&mut st, &mut cur, target);
                clock = None;
                continue;
            }
            cur.refill(rec);
            // No line is played at or past the end of its map; looked at after the lines, so a
            // line seen has its part.
            let (_, end) = self.reach(&st);
            let Some(t) = cur.lines.front().map(|l| l.0).filter(|t| end.is_none_or(|end| *t < end)) else {
                st = self.at_end(st, end);
                clock = None;
                continue;
            };
            if st.paused || (self.viewers.load(Ordering::Relaxed) == 0 && self.relay_clients() == 0) {
                st = self.sleep(st, IDLE_POLL);
                clock = None;
                continue;
            }
            if t > st.at {
                st = self.wait_until(st, t, &mut announced, &mut clock);
                continue;
            }
            let (_, line) = cur.lines.pop_front().expect("a front line");
            cur.done += 1;
            publish(&self.hub, &line);
            self.relay_line(&line);
            if let Line::Snapshot(snap) = &line {
                st.tick = Some(snap.tick);
                self.follow(&st);
            }
        }
    }

    /// Pages and the relay hear where playback is, now.
    fn announce_now(&self, st: &State, announced: &mut Instant) {
        self.announce(st);
        self.tell_relay();
        *announced = Instant::now();
    }

    /// Playback goes to `target`: the hub as it was there, and the lines after.
    fn seek_to(&self, st: &mut State, cur: &mut Cursor, target: u64) {
        let rec = &self.recording;
        let (hub, chunk, done) = rec.seek(target);
        // Where it went, paused or not: the relay follows at once.
        st.tick = hub.tick();
        restore(&self.hub, hub);
        *cur = Cursor { chunk, done, looked: None, lines: VecDeque::new() };
        st.at = self.held(st, target);
        self.announce(st);
        // Every move between maps ends here: the marker goes with the room.
        self.show_marker(self.part(st).n);
    }

    /// At the end of what is played: of a map that the match went on from (`over`, its end), it
    /// waits there, unless it follows live and goes on; of what is recorded, a live match's goes on
    /// (at 1×, as it comes in, following it unless paused), any other pauses there.
    fn at_end<'a>(&self, mut st: MutexGuard<'a, State>, over: Option<u64>) -> MutexGuard<'a, State> {
        let rec = &self.recording;
        if let Some(end) = over {
            if self.move_on(&mut st) {
                return st;
            }
            if !st.paused && st.at < end {
                st.at = end;
                self.announce(&st);
            }
            return self.sleep(st, IDLE_POLL);
        }
        if rec.is_live() {
            if !st.paused {
                (st.at, st.follows) = (st.at.max(rec.length()), true);
            }
            if (st.speed - 1.0).abs() > f64::EPSILON {
                st.speed = 1.0;
                self.announce(&st);
            }
            return self.sleep(st, LIVE_POLL);
        }
        if !st.paused {
            (st.paused, st.at) = (true, rec.length());
            self.announce(&st);
        }
        self.sleep(st, IDLE_POLL)
    }

    /// Waits toward the next line, at `t`, for at most [`ANNOUNCE_EVERY`], by `clock` (set down
    /// here when there is none, or the speed changed): playback is then where the clock says.
    fn wait_until<'a>(
        &self,
        st: MutexGuard<'a, State>,
        t: u64,
        announced: &mut Instant,
        clock: &mut Option<Clock>,
    ) -> MutexGuard<'a, State> {
        let now = Instant::now();
        let by = *clock.insert(match *clock {
            Some(c) if (c.speed - st.speed).abs() < f64::EPSILON => c,
            _ => Clock::new(now, st.at, st.speed),
        });
        let mut st = self.sleep(st, by.due(t).saturating_duration_since(now).min(ANNOUNCE_EVERY));
        // Woken to something else (a seek, a pause): the loop sees to it, and sets the clock anew.
        st.at = by.at(Instant::now()).clamp(st.at, t);
        if announced.elapsed() >= ANNOUNCE_EVERY {
            self.announce_now(&st, announced);
        }
        st
    }

    pub(super) fn sleep<'a>(&self, st: MutexGuard<'a, State>, d: Duration) -> MutexGuard<'a, State> {
        self.wake.wait_timeout(st, d).unwrap_or_else(std::sync::PoisonError::into_inner).0
    }
}
