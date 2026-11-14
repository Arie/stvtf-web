//! Voice from outside a live match (a team's Discord), held until the match's stream shows the
//! moment it was said in.
//!
//! `SourceTV` is a fixed number of ticks behind the game (its delay in ticks, whatever the server's
//! pace), so what is said while the stream is at tick `T` is due at `T` plus that. Speech runs by
//! the clock and ticks need not, so it is anchored by utterance (a speaker's frames with no gap
//! over [`GAP`]): the first frame goes out at its due tick, the rest follow at 20 ms each, never
//! more than [`AHEAD`] before their own due tick. A pause stops everything where it is; what is
//! said during one is set apart.
//!
//! What is held can be saved for another process on the same stream ([`VoiceHold::saved`],
//! [`VoiceHold::adopt`]): due ticks stand, clocks do not.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// How long a frame of voice is.
pub(crate) const FRAME: Duration = Duration::from_millis(20);
/// A speaker silent this long starts a new utterance.
pub(crate) const GAP: Duration = Duration::from_millis(200);
/// The most a frame goes out before its own due tick.
const AHEAD: Duration = Duration::from_millis(500);
/// Frames that came this soon after the tick last moved were still said in the running game: the
/// stream's ticks come some apart.
const NOTICED: Duration = Duration::from_millis(100);
/// The most frames held of one speaker (speaking throughout the longest delay, and some):
/// past it the oldest go.
pub(crate) const MAX_HELD: usize = 16_000;
/// The most speakers held at once, and the most frames of pause talk kept.
const MAX_SPEAKERS: usize = 64;
const MAX_PAUSE_TALK: usize = 6000;
/// How often what is held is saved while it changes.
pub(crate) const SAVED_EVERY: Duration = Duration::from_secs(5);
/// A hold that saved and went on may have let go what was due this long after.
const MAYBE_PLAYED: Duration = Duration::from_secs(7);
/// The latest that what fell due with no process holding it still goes out.
const MAX_LATE: Duration = Duration::from_secs(30);
/// The most holds remembered as taken from.
const MAX_ADOPTED: usize = 8;

/// Frames (base64 Opus, in order) of one speaker: `who` is whatever names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Said {
    pub(crate) who: String,
    pub(crate) frames: Vec<String>,
}

/// What became of frames given to the hold.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Took {
    /// Held until the stream is at tick `due`.
    Held { due: u32 },
    /// Said while the game is paused: set apart.
    PauseTalk,
    /// No stream to hold it against (yet, or any more).
    NoStream,
    /// Too many speakers.
    Full,
    /// Not held: its source is not behind the stream (commentary), so it went straight out.
    Live,
    /// The process is stopping: what it held is saved for the next, which takes this too.
    Closed,
}

struct Frame {
    /// Its number in this hold: they count up.
    n: u64,
    due: u32,
    opus: String,
    /// The first of an utterance: it waits for its tick, those after it for the clock.
    starts: bool,
    /// When it came in; none for one another hold had.
    came: Option<Instant>,
}

/// A held frame as saved: its number, its due tick, whether it starts an utterance, the Opus.
#[derive(Debug, Serialize, Deserialize)]
struct SavedFrame(u64, u32, bool, String);

/// What a hold holds, as saved for another process.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Held {
    /// Each speaker's frames, in order.
    #[serde(default)]
    speakers: BTreeMap<String, Vec<SavedFrame>>,
    /// Pause talk nobody asked for yet: its number, who, their frames.
    #[serde(default)]
    pause_talk: Vec<(u64, String, Vec<String>)>,
}

impl Held {
    pub(crate) fn is_empty(&self) -> bool {
        self.speakers.is_empty() && self.pause_talk.is_empty()
    }

    /// Whether anything of `who` is in it.
    pub(crate) fn has(&self, who: &str) -> bool {
        self.speakers.contains_key(who) || self.pause_talk.iter().any(|(_, w, _)| w == who)
    }
}

#[derive(Default)]
struct Speaker {
    frames: VecDeque<Frame>,
    /// When their last frame came in.
    heard: Option<Instant>,
    /// The utterance going out: when its first frame went, and how many have.
    running: Option<(Instant, u32)>,
}

pub(crate) struct VoiceHold {
    interval: f64,
    /// The stream's tick, once it has one.
    tick: Option<u32>,
    /// Since when the game is paused.
    paused: Option<Instant>,
    speakers: BTreeMap<String, Speaker>,
    /// Each with its number, as frames have.
    pause_talk: Vec<(u64, Said)>,
    /// How many numbers were given out.
    count: u64,
    /// The highest number taken from each hold adopted from, by its name.
    adopted: BTreeMap<String, u64>,
}

impl VoiceHold {
    /// A hold for a stream of ticks `interval` seconds long.
    pub(crate) fn new(interval: f64) -> Self {
        VoiceHold {
            interval,
            tick: None,
            paused: None,
            speakers: BTreeMap::new(),
            pause_talk: Vec::new(),
            count: 0,
            adopted: BTreeMap::new(),
        }
    }

    fn ticks(&self, time: f64) -> u32 {
        (time / self.interval.max(1e-6)).round() as u32
    }

    fn number(&mut self) -> u64 {
        self.count += 1;
        self.count
    }

    fn pause_talk_frames(&self) -> usize {
        self.pause_talk.iter().map(|(_, s)| s.frames.len()).sum()
    }

    /// Set apart as pause talk, while there is room.
    fn set_apart(&mut self, said: Said) {
        if self.pause_talk_frames() + said.frames.len() <= MAX_PAUSE_TALK {
            let n = self.number();
            self.pause_talk.push((n, said));
        }
    }

    /// Different whenever what is held is: something came in, went out or was dropped.
    pub(crate) fn mark(&self) -> (u64, usize, usize) {
        (self.count, self.held(), self.pause_talk_frames())
    }

    /// What is held now, for another process.
    pub(crate) fn saved(&self) -> Held {
        let frames = |s: &Speaker| s.frames.iter().map(|f| SavedFrame(f.n, f.due, f.starts, f.opus.clone())).collect();
        Held {
            speakers: self.speakers.iter().filter(|(_, s)| !s.frames.is_empty()).map(|(who, s)| (who.clone(), frames(s))).collect(),
            pause_talk: self.pause_talk.iter().map(|(n, s)| (*n, s.who.clone(), s.frames.clone())).collect(),
        }
    }

    /// Takes what the hold named `from` saved with its stream at tick `at`, on the same stream:
    /// what was not taken from it before, each speaker's ahead of what they said here. `last`: it
    /// let nothing go after saving; else what it may have played since is dropped. What is late
    /// goes out at once, by the clock, unless over [`MAX_LATE`] late. Nothing without a tick.
    pub(crate) fn adopt(&mut self, from: &str, held: Held, at: u32, last: bool) {
        let Some(tick) = self.tick else { return };
        let had = self.adopted.get(from).copied().unwrap_or(0);
        let mut top = had;
        let (too_late, maybe_played) = (self.ticks(MAX_LATE.as_secs_f64()), at.saturating_add(self.ticks(MAYBE_PLAYED.as_secs_f64())));
        let stands = |due: u32| due.saturating_add(too_late) >= tick && (last || due > tick || due > maybe_played);
        for (who, frames) in held.speakers {
            top = frames.iter().map(|f| f.0).fold(top, u64::max);
            let new: Vec<SavedFrame> = frames.into_iter().filter(|f| f.0 > had && stands(f.1)).collect();
            if new.is_empty() || (!self.speakers.contains_key(&who) && self.speakers.len() >= MAX_SPEAKERS) {
                continue;
            }
            let new: Vec<Frame> =
                new.into_iter().map(|SavedFrame(_, due, starts, opus)| Frame { n: self.number(), due, opus, starts, came: None }).collect();
            let s = self.speakers.entry(who).or_default();
            let own = s.frames.iter().position(|f| f.came.is_some()).unwrap_or(s.frames.len());
            let said_here = s.frames.split_off(own);
            s.frames.extend(new);
            s.frames.extend(said_here);
            while s.frames.len() > MAX_HELD {
                s.frames.pop_front();
            }
        }
        for (n, who, frames) in held.pause_talk {
            top = top.max(n);
            if n > had {
                self.set_apart(Said { who, frames });
            }
        }
        if self.adopted.len() >= MAX_ADOPTED && !self.adopted.contains_key(from) {
            self.adopted.clear();
        }
        self.adopted.insert(from.to_string(), top);
    }

    /// Frames held, all speakers'.
    pub(crate) fn held(&self) -> usize {
        self.speakers.values().map(|s| s.frames.len()).sum()
    }

    /// Takes what a speaker said just now, to come out when the stream is `delay` seconds of game
    /// further on: how far behind its game their source says the stream is.
    pub(crate) fn take_in(&mut self, said: Said, delay: f64, now: Instant) -> Took {
        let Some(tick) = self.tick else { return Took::NoStream };
        if self.paused.is_some() {
            self.set_apart(said);
            return Took::PauseTalk;
        }
        if !self.speakers.contains_key(&said.who) && self.speakers.len() >= MAX_SPEAKERS {
            return Took::Full;
        }
        let due = tick + self.ticks(delay);
        let first = self.count + 1;
        self.count += said.frames.len() as u64;
        let s = self.speakers.entry(said.who).or_default();
        let mut starts = s.heard.is_none_or(|h| now.duration_since(h) > GAP);
        s.heard = Some(now);
        for (n, opus) in (first..).zip(said.frames) {
            s.frames.push_back(Frame { n, due, opus, starts, came: Some(now) });
            starts = false;
        }
        while s.frames.len() > MAX_HELD {
            s.frames.pop_front();
            // What is left of a cut utterance starts at its own tick.
            if let Some(f) = s.frames.front_mut() {
                f.starts = true;
            }
        }
        Took::Held { due }
    }

    /// What goes out now, the clock at `now`; with `tick`, the stream is there.
    pub(crate) fn due(&mut self, tick: Option<u32>, now: Instant) -> Vec<Said> {
        if tick.is_some() {
            self.tick = tick;
        }
        let (Some(tick), None) = (self.tick, self.paused) else { return Vec::new() };
        let ahead = tick + self.ticks(AHEAD.as_secs_f64());
        let mut out = Vec::new();
        for (who, s) in &mut self.speakers {
            let mut frames = Vec::new();
            while let Some(f) = s.frames.front() {
                let go = match s.running {
                    Some((since, n)) if !f.starts => now >= since + FRAME * n && f.due <= ahead,
                    _ => f.due <= tick,
                };
                if !go {
                    break;
                }
                s.running = Some(match s.running {
                    Some((since, n)) if !f.starts => (since, n + 1),
                    _ => (now, 1),
                });
                frames.extend(s.frames.pop_front().map(|f| f.opus));
            }
            if !frames.is_empty() {
                out.push(Said { who: who.clone(), frames });
            }
        }
        // Speakers long done with are forgotten.
        self.speakers.retain(|_, s| !s.frames.is_empty() || s.heard.is_some_and(|h| now.duration_since(h) < 2 * AHEAD + GAP));
        out
    }

    /// The game paused, or went on: utterances stop where they are, and go on from there. A
    /// pause may be noticed late (a stream that says none shows it only by its tick standing
    /// still): the game stood still from `since`, and what was said after that is pause talk.
    pub(crate) fn on_pause(&mut self, paused: bool, now: Instant, since: Instant) {
        match (paused, self.paused) {
            (true, None) => {
                self.paused = Some(now);
                let mut talk = Vec::new();
                for (who, s) in &mut self.speakers {
                    let kept = s.frames.iter().take_while(|f| f.came.is_none_or(|came| came <= since + NOTICED)).count();
                    let frames: Vec<String> = s.frames.drain(kept..).map(|f| f.opus).collect();
                    if !frames.is_empty() {
                        talk.push(Said { who: who.clone(), frames });
                    }
                }
                for said in talk {
                    let n = self.number();
                    self.pause_talk.push((n, said));
                }
            }
            (false, Some(since)) => {
                self.paused = None;
                let stood = now.duration_since(since);
                for s in self.speakers.values_mut() {
                    s.running = s.running.map(|(at, n)| (at + stood, n));
                }
            }
            _ => {}
        }
    }

    /// What was said during pauses since last asked.
    pub(crate) fn pause_talk(&mut self) -> Vec<Said> {
        std::mem::take(&mut self.pause_talk).into_iter().map(|(_, said)| said).collect()
    }

    /// The stream is gone, or on another map (its ticks start over): nothing held stands.
    pub(crate) fn reset(&mut self) {
        self.tick = None;
        self.paused = None;
        self.speakers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: Duration = Duration::from_secs(1);

    /// A hold at 66.67 ticks a second, its stream at tick 1000: 90 s is 6000 ticks.
    fn hold(t0: Instant) -> VoiceHold {
        let mut h = VoiceHold::new(0.015);
        assert_eq!(h.due(Some(1000), t0).len(), 0);
        h
    }

    fn said(who: &str, frames: std::ops::Range<u32>) -> Said {
        Said { who: who.into(), frames: frames.map(|i| i.to_string()).collect() }
    }

    fn count(out: &[Said]) -> usize {
        out.iter().map(|s| s.frames.len()).sum()
    }

    #[test]
    fn a_pause_noticed_late_takes_back_what_was_said_since_the_game_stood_still() {
        // A stream that says no pause: it shows only as its tick not moving, noticed after a while.
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut h = hold(t0);
        h.take_in(said("a", 0..5), 90.0, t0);
        // The tick stood still from 100 ms on; these came after that, before anyone knew.
        h.take_in(said("a", 5..10), 90.0, t0 + ms(300));
        h.take_in(said("a", 10..15), 90.0, t0 + ms(600));
        h.on_pause(true, t0 + ms(800), t0 + ms(100));
        assert_eq!(h.held(), 5, "only what was said while the game ran");
        assert_eq!(h.pause_talk(), [said("a", 5..15)], "the rest is pause talk, in order");
        // What is said once the game goes on starts at its own tick: nothing is in its way.
        h.on_pause(false, t0 + 10 * SEC, t0 + 10 * SEC);
        h.take_in(said("a", 20..21), 90.0, t0 + 10 * SEC);
        // The first goes at its tick, the rest by the clock; the new one right behind, not late.
        let mut out = h.due(Some(7001), t0 + 100 * SEC);
        out.extend(h.due(Some(7001), t0 + 100 * SEC + ms(80)));
        let frames: Vec<String> = out.into_iter().flat_map(|s| s.frames).collect();
        assert_eq!(frames, (0..5).chain(20..21).map(|i| i.to_string()).collect::<Vec<_>>());
    }

    #[test]
    fn an_utterance_starts_the_delay_in_ticks_after_it_came() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        assert_eq!(h.take_in(said("a", 0..1), 90.0, t0), Took::Held { due: 7000 });
        assert_eq!(h.due(Some(6999), t0 + 89 * SEC).len(), 0, "not yet");
        assert_eq!(h.due(Some(7000), t0 + 95 * SEC), [said("a", 0..1)], "by the tick, whatever the clock says");
    }

    #[test]
    fn the_rest_of_an_utterance_follows_by_the_clock_whatever_the_ticks_do() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        // A second of speech, said while the server made only 30 ticks of it.
        for i in 0..50u32 {
            h.due(Some(1000 + i * 30 / 50), t0 + FRAME * i);
            h.take_in(said("a", i..i + 1), 90.0, t0 + FRAME * i);
        }
        let start = t0 + 90 * SEC;
        assert_eq!(count(&h.due(Some(7000), start)), 1, "the first, at its tick");
        // Half a second on, the stream 34 ticks on: half the speech is out, at speech's pace.
        assert_eq!(count(&h.due(Some(7034), start + SEC / 2)), 25);
        assert_eq!(count(&h.due(Some(7067), start + SEC)), 24, "and the rest a second in");
    }

    #[test]
    fn a_frame_is_never_out_long_before_its_own_tick() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        // Two seconds of speech over two seconds of ticks; then the stream nearly stands still.
        for i in 0..100u32 {
            h.due(Some(1000 + i * 4 / 3), t0 + FRAME * i);
            h.take_in(said("a", i..i + 1), 90.0, t0 + FRAME * i);
        }
        let start = t0 + 90 * SEC;
        h.due(Some(7000), start);
        let out = count(&h.due(Some(7001), start + 2 * SEC));
        // Its tick is 7001: frames due up to 500 ms (33 ticks) past it may be out, no more.
        assert!((20..=27).contains(&out), "{out} frames out though the stream stood still");
    }

    #[test]
    fn a_new_utterance_anchors_anew() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        h.take_in(said("a", 0..1), 90.0, t0);
        h.due(Some(1040), t0 + GAP + FRAME);
        assert_eq!(h.take_in(said("a", 1..2), 90.0, t0 + GAP + FRAME), Took::Held { due: 7040 });
        assert_eq!(count(&h.due(Some(7000), t0 + 90 * SEC)), 1);
        assert_eq!(h.due(Some(7039), t0 + 100 * SEC).len(), 0, "the second waits for its own tick");
        assert_eq!(count(&h.due(Some(7040), t0 + 100 * SEC)), 1);
    }

    #[test]
    fn speakers_are_held_apart() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        h.take_in(said("a", 0..1), 90.0, t0);
        h.take_in(said("b", 5..6), 90.0, t0);
        let out = h.due(Some(7000), t0 + 90 * SEC);
        assert_eq!(count(&out), 2);
        assert!(out.contains(&said("a", 0..1)) && out.contains(&said("b", 5..6)), "{out:?}");
    }

    #[test]
    fn a_pause_stops_an_utterance_and_it_goes_on_after() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        for i in 0..50u32 {
            h.take_in(said("a", i..i + 1), 90.0, t0 + FRAME * i);
        }
        let start = t0 + 90 * SEC;
        h.due(Some(7067), start);
        assert_eq!(count(&h.due(Some(7067), start + FRAME * 9)), 9);
        h.on_pause(true, start + FRAME * 9, start + FRAME * 9);
        assert_eq!(h.due(Some(7067), start + 30 * SEC).len(), 0, "nothing while paused");
        h.on_pause(false, start + 30 * SEC, start + 30 * SEC);
        assert_eq!(count(&h.due(Some(7067), start + 30 * SEC + FRAME * 10)), 10, "on from where it stopped");
    }

    #[test]
    fn what_is_said_in_a_pause_is_pause_talk_not_held() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        h.on_pause(true, t0, t0);
        assert_eq!(h.take_in(said("a", 0..3), 90.0, t0), Took::PauseTalk);
        h.on_pause(false, t0 + SEC, t0 + SEC);
        assert_eq!(h.due(Some(9000), t0 + 200 * SEC).len(), 0, "never played");
        assert_eq!(h.pause_talk(), [said("a", 0..3)]);
        assert_eq!(h.pause_talk().len(), 0, "taken once");
    }

    #[test]
    fn before_any_tick_and_after_a_reset_it_is_refused() {
        let t0 = Instant::now();
        let mut h = VoiceHold::new(0.015);
        assert_eq!(h.take_in(said("a", 0..1), 90.0, t0), Took::NoStream);
        h.due(Some(1000), t0);
        h.take_in(said("a", 0..1), 90.0, t0);
        h.reset();
        assert_eq!(h.take_in(said("a", 1..2), 90.0, t0), Took::NoStream);
        assert_eq!(h.due(Some(9000), t0 + 200 * SEC).len(), 0, "what was held went with the reset");
    }

    #[test]
    fn over_the_cap_the_oldest_go() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        h.take_in(said("a", 0..MAX_HELD as u32 + 10), 90.0, t0);
        assert_eq!(h.held(), MAX_HELD);
        let out = h.due(Some(7000), t0 + 90 * SEC);
        assert_eq!(out[0].frames[0], "10", "the oldest ten went; what is left starts at its tick");
    }

    #[test]
    fn each_source_has_its_own_delay() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        assert_eq!(h.take_in(said("team", 0..1), 90.0, t0), Took::Held { due: 7000 });
        assert_eq!(h.take_in(said("other", 0..1), 30.0, t0), Took::Held { due: 3000 });
        assert_eq!(h.due(Some(3000), t0 + 30 * SEC), [said("other", 0..1)]);
    }

    /// What `h` holds, as another process would read it.
    fn handed_on(h: &VoiceHold) -> Held {
        serde_json::from_str(&serde_json::to_string(&h.saved()).unwrap()).unwrap()
    }

    #[test]
    fn what_a_hold_saved_comes_out_of_the_next_at_its_tick() {
        let t0 = Instant::now();
        let mut old = hold(t0);
        for i in 0..3u32 {
            old.take_in(said("a", i..i + 1), 90.0, t0 + FRAME * i);
        }
        // The next process: another clock, the same stream.
        let t1 = t0 + 10 * SEC;
        let mut new = VoiceHold::new(0.015);
        new.due(Some(1700), t1);
        new.adopt("old", handed_on(&old), 1650, true);
        assert_eq!(new.held(), 3);
        assert_eq!(new.due(Some(6999), t1 + 80 * SEC).len(), 0, "not before its tick");
        assert_eq!(new.due(Some(7000), t1 + 80 * SEC), [said("a", 0..1)], "the first at its tick");
        assert_eq!(new.due(Some(7003), t1 + 80 * SEC + 2 * FRAME), [said("a", 1..3)], "the rest by the clock");
    }

    #[test]
    fn a_hold_saved_again_gives_only_what_is_new_in_it() {
        let t0 = Instant::now();
        let mut old = hold(t0);
        old.take_in(said("a", 0..2), 90.0, t0);
        let mut new = VoiceHold::new(0.015);
        new.due(Some(1000), t0);
        new.adopt("old", handed_on(&old), 1000, false);
        // The old one goes on a while beside the new one, as at a deploy.
        old.take_in(said("a", 2..3), 90.0, t0 + FRAME);
        new.take_in(said("a", 7..8), 90.0, t0 + SEC);
        new.adopt("old", handed_on(&old), 1000, true);
        new.adopt("old", handed_on(&old), 1000, true);
        assert_eq!(new.held(), 4, "each frame once");
        let mut out = Vec::new();
        for i in 0..10 {
            out.extend(new.due(Some(7100), t0 + 100 * SEC + FRAME * i));
        }
        let frames: Vec<String> = out.into_iter().flat_map(|s| s.frames).collect();
        assert_eq!(frames, ["0", "1", "2", "7"], "the old hold's before what was said here since");
    }

    #[test]
    fn what_fell_due_with_nobody_holding_it_goes_out_at_once_unless_long_late() {
        let t0 = Instant::now();
        let mut old = hold(t0);
        old.take_in(said("late", 0..1), 10.0, t0);
        old.take_in(said("gone", 0..1), 1.0, t0);
        old.take_in(said("ahead", 0..1), 90.0, t0);
        // The stream is 39 s on: one is 29 s late, one 38 s.
        let mut new = VoiceHold::new(0.015);
        new.due(Some(3600), t0);
        new.adopt("old", handed_on(&old), 1000, true);
        assert_eq!(new.due(None, t0), [said("late", 0..1)]);
        assert_eq!(new.held(), 1, "what was too late is dropped, what is ahead waits");
    }

    #[test]
    fn what_a_hold_that_went_on_after_saving_may_have_played_is_not_played_again() {
        let t0 = Instant::now();
        let mut old = hold(t0);
        old.take_in(said("soon", 0..1), 3.0, t0);
        old.take_in(said("later", 0..1), 12.0, t0);
        // It saved at tick 1000 and stopped without a word (a crash); the stream is 15 s on.
        let mut new = VoiceHold::new(0.015);
        new.due(Some(2000), t0);
        new.adopt("old", handed_on(&old), 1000, false);
        assert_eq!(new.due(None, t0), [said("later", 0..1)], "due after it would have saved again: not played");
        assert_eq!(new.held(), 0, "due soon after it saved: played by it, as likely as not");
    }

    #[test]
    fn pause_talk_is_handed_on_once() {
        let t0 = Instant::now();
        let mut old = hold(t0);
        old.on_pause(true, t0, t0);
        old.take_in(said("a", 0..3), 90.0, t0);
        let mut new = VoiceHold::new(0.015);
        new.due(Some(1000), t0);
        new.adopt("old", handed_on(&old), 1000, false);
        new.adopt("old", handed_on(&old), 1000, true);
        assert_eq!(new.pause_talk(), [said("a", 0..3)]);
    }

    #[test]
    fn a_hold_says_when_what_it_holds_changed() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        let empty = h.mark();
        h.take_in(said("a", 0..1), 90.0, t0);
        let held = h.mark();
        assert_ne!(empty, held);
        h.due(Some(6000), t0 + 80 * SEC);
        assert_eq!(h.mark(), held, "nothing went out");
        h.due(Some(7000), t0 + 90 * SEC);
        assert_ne!(h.mark(), held, "something did");
        assert!(h.saved().is_empty());
    }
}
