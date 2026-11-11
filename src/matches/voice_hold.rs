//! Voice from outside a live match (a team's Discord), held until the match's stream shows the
//! moment it was said in.
//!
//! `SourceTV` is a fixed number of ticks behind the game (its delay in ticks, whatever the server's
//! pace), so what is said while the stream is at tick `T` is due at `T` plus that. Speech runs by
//! the clock and ticks need not, so it is anchored by utterance (a speaker's frames with no gap
//! over [`GAP`]): the first frame goes out at its due tick, the rest follow at 20 ms each, never
//! more than [`AHEAD`] before their own due tick. A pause stops everything where it is; what is
//! said during one is set apart.

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
/// The most frames held of one speaker (a minute): past it the oldest go.
pub(crate) const MAX_HELD: usize = 3000;
/// The most speakers held at once, and the most frames of pause talk kept.
const MAX_SPEAKERS: usize = 64;
const MAX_PAUSE_TALK: usize = 6000;

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
}

struct Frame {
    due: u32,
    opus: String,
    /// The first of an utterance: it waits for its tick, those after it for the clock.
    starts: bool,
    /// When it came in.
    came: Instant,
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
    pause_talk: Vec<Said>,
}

impl VoiceHold {
    /// A hold for a stream of ticks `interval` seconds long.
    pub(crate) fn new(interval: f64) -> Self {
        VoiceHold { interval, tick: None, paused: None, speakers: BTreeMap::new(), pause_talk: Vec::new() }
    }

    fn ticks(&self, time: f64) -> u32 {
        (time / self.interval.max(1e-6)).round() as u32
    }

    /// Frames held, all speakers'.
    pub(crate) fn held(&self) -> usize {
        self.speakers.values().map(|s| s.frames.len()).sum()
    }

    /// Takes what a speaker said just now, to come out when the stream is `delay` seconds of game
    /// further on: how far behind its game their source says the stream is.
    pub(crate) fn said(&mut self, said: Said, delay: f64, now: Instant) -> Took {
        let Some(tick) = self.tick else { return Took::NoStream };
        if self.paused.is_some() {
            if self.pause_talk.iter().map(|s| s.frames.len()).sum::<usize>() + said.frames.len() <= MAX_PAUSE_TALK {
                self.pause_talk.push(said);
            }
            return Took::PauseTalk;
        }
        if !self.speakers.contains_key(&said.who) && self.speakers.len() >= MAX_SPEAKERS {
            return Took::Full;
        }
        let due = tick + self.ticks(delay);
        let s = self.speakers.entry(said.who).or_default();
        let mut starts = s.heard.is_none_or(|h| now.duration_since(h) > GAP);
        s.heard = Some(now);
        for opus in said.frames {
            s.frames.push_back(Frame { due, opus, starts, came: now });
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
                for (who, s) in &mut self.speakers {
                    let kept = s.frames.iter().take_while(|f| f.came <= since + NOTICED).count();
                    let frames: Vec<String> = s.frames.drain(kept..).map(|f| f.opus).collect();
                    if !frames.is_empty() {
                        self.pause_talk.push(Said { who: who.clone(), frames });
                    }
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
        std::mem::take(&mut self.pause_talk)
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
        h.said(said("a", 0..5), 90.0, t0);
        // The tick stood still from 100 ms on; these came after that, before anyone knew.
        h.said(said("a", 5..10), 90.0, t0 + ms(300));
        h.said(said("a", 10..15), 90.0, t0 + ms(600));
        h.on_pause(true, t0 + ms(800), t0 + ms(100));
        assert_eq!(h.held(), 5, "only what was said while the game ran");
        assert_eq!(h.pause_talk(), [said("a", 5..15)], "the rest is pause talk, in order");
        // What is said once the game goes on starts at its own tick: nothing is in its way.
        h.on_pause(false, t0 + 10 * SEC, t0 + 10 * SEC);
        h.said(said("a", 20..21), 90.0, t0 + 10 * SEC);
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
        assert_eq!(h.said(said("a", 0..1), 90.0, t0), Took::Held { due: 7000 });
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
            h.said(said("a", i..i + 1), 90.0, t0 + FRAME * i);
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
            h.said(said("a", i..i + 1), 90.0, t0 + FRAME * i);
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
        h.said(said("a", 0..1), 90.0, t0);
        h.due(Some(1040), t0 + GAP + FRAME);
        assert_eq!(h.said(said("a", 1..2), 90.0, t0 + GAP + FRAME), Took::Held { due: 7040 });
        assert_eq!(count(&h.due(Some(7000), t0 + 90 * SEC)), 1);
        assert_eq!(h.due(Some(7039), t0 + 100 * SEC).len(), 0, "the second waits for its own tick");
        assert_eq!(count(&h.due(Some(7040), t0 + 100 * SEC)), 1);
    }

    #[test]
    fn speakers_are_held_apart() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        h.said(said("a", 0..1), 90.0, t0);
        h.said(said("b", 5..6), 90.0, t0);
        let out = h.due(Some(7000), t0 + 90 * SEC);
        assert_eq!(count(&out), 2);
        assert!(out.contains(&said("a", 0..1)) && out.contains(&said("b", 5..6)), "{out:?}");
    }

    #[test]
    fn a_pause_stops_an_utterance_and_it_goes_on_after() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        for i in 0..50u32 {
            h.said(said("a", i..i + 1), 90.0, t0 + FRAME * i);
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
        assert_eq!(h.said(said("a", 0..3), 90.0, t0), Took::PauseTalk);
        h.on_pause(false, t0 + SEC, t0 + SEC);
        assert_eq!(h.due(Some(9000), t0 + 200 * SEC).len(), 0, "never played");
        assert_eq!(h.pause_talk(), [said("a", 0..3)]);
        assert_eq!(h.pause_talk().len(), 0, "taken once");
    }

    #[test]
    fn before_any_tick_and_after_a_reset_it_is_refused() {
        let t0 = Instant::now();
        let mut h = VoiceHold::new(0.015);
        assert_eq!(h.said(said("a", 0..1), 90.0, t0), Took::NoStream);
        h.due(Some(1000), t0);
        h.said(said("a", 0..1), 90.0, t0);
        h.reset();
        assert_eq!(h.said(said("a", 1..2), 90.0, t0), Took::NoStream);
        assert_eq!(h.due(Some(9000), t0 + 200 * SEC).len(), 0, "what was held went with the reset");
    }

    #[test]
    fn over_the_cap_the_oldest_go() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        h.said(said("a", 0..MAX_HELD as u32 + 10), 90.0, t0);
        assert_eq!(h.held(), MAX_HELD);
        let out = h.due(Some(7000), t0 + 90 * SEC);
        assert_eq!(out[0].frames[0], "10", "the oldest ten went; what is left starts at its tick");
    }

    #[test]
    fn each_source_has_its_own_delay() {
        let t0 = Instant::now();
        let mut h = hold(t0);
        assert_eq!(h.said(said("team", 0..1), 90.0, t0), Took::Held { due: 7000 });
        assert_eq!(h.said(said("other", 0..1), 30.0, t0), Took::Held { due: 3000 });
        assert_eq!(h.due(Some(3000), t0 + 30 * SEC), [said("other", 0..1)]);
    }
}
