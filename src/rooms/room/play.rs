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
}

/// What an owner changes; unset fields stay.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Control {
    pub(crate) paused: Option<bool>,
    pub(crate) speed: Option<f64>,
    pub(crate) seek: Option<u64>,
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
        if let Some(speed) = c.speed {
            st.speed = speed;
        }
        if let Some(paused) = c.paused {
            st.paused = paused;
        }
        if let Some(at) = c.seek {
            st.seek = Some(at.clamp(self.recording.start(), self.recording.length()));
        }
        self.announce(&st);
        self.wake.notify_all();
        Ok(())
    }

    pub(super) fn announce(&self, st: &State) {
        let line = serde_json::json!({
            "type": "playback",
            "paused": st.paused,
            "speed": st.speed,
            "at": st.at,
            "length": self.recording.length(),
            // What is kept (from `start`), and where the map began.
            "start": self.recording.start(),
            "origin": self.recording.origin(),
            "rounds": self.recording.rounds(),
            "live": self.recording.is_live(),
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
        loop {
            if st.stop {
                return;
            }
            // A live match goes on, paused or not: pages hear how long it is now.
            if rec.is_live() && announced.elapsed() >= ANNOUNCE_EVERY {
                self.announce_now(&st, &mut announced);
            }
            // What is played was dropped (a new map): on from what is kept.
            if st.seek.is_none() && st.at < rec.start() {
                st.seek = Some(rec.start());
            }
            if let Some(target) = st.seek.take() {
                self.seek_to(&mut st, &mut cur, target);
                continue;
            }
            cur.refill(rec);
            let Some(&(t, _)) = cur.lines.front() else {
                st = self.at_end(st);
                continue;
            };
            if st.paused || (self.viewers.load(Ordering::Relaxed) == 0 && self.relay_clients() == 0) {
                st = self.sleep(st, IDLE_POLL);
                continue;
            }
            if t > st.at {
                st = self.wait_until(st, t, &mut announced);
                continue;
            }
            let (_, line) = cur.lines.pop_front().expect("a front line");
            cur.done += 1;
            publish(&self.hub, &line);
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
        st.at = target.max(rec.start());
        self.announce(st);
    }

    /// At the end of what is recorded: a live match's goes on (at 1×, as it comes in), any other
    /// pauses there.
    fn at_end<'a>(&self, mut st: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        let rec = &self.recording;
        if rec.is_live() {
            if !st.paused {
                st.at = st.at.max(rec.length());
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

    /// Waits toward the next line, at `t`, for at most [`ANNOUNCE_EVERY`]; whatever was waited
    /// counts at the speed it was waited at.
    fn wait_until<'a>(&self, st: MutexGuard<'a, State>, t: u64, announced: &mut Instant) -> MutexGuard<'a, State> {
        let (speed, started, gap) = (st.speed, Instant::now(), t - st.at);
        let mut st = self.sleep(st, wait(gap, speed).min(ANNOUNCE_EVERY));
        let played = (started.elapsed().as_secs_f64() * 1000.0 * speed) as u64;
        st.at = (st.at + played).min(t);
        if announced.elapsed() >= ANNOUNCE_EVERY {
            self.announce_now(&st, announced);
        }
        st
    }

    pub(super) fn sleep<'a>(&self, st: MutexGuard<'a, State>, d: Duration) -> MutexGuard<'a, State> {
        self.wake.wait_timeout(st, d).unwrap_or_else(std::sync::PoisonError::into_inner).0
    }
}
