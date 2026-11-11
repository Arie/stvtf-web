//! A live match's voice from outside it: what the bot posts goes into a [`VoiceHold`], and what
//! the hold lets go is published to the match's hub as `voice` lines (pages that asked, and the
//! recording). The hold goes by the hub's stream: its tick, its pauses, its new starts.

use crate::lock;
use crate::matches::hub::{SharedHub, publish};
use crate::matches::voice_hold::{Said, Took, VoiceHold};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use stvtf::Line;
use stvtf::output::Voice;

/// How far behind its game a source's stream is unless it says: most leagues' `tv_delay`.
pub(crate) const DEFAULT_DELAY: f64 = 90.0;
/// The delays taken: `tv_delay`'s own range.
pub(crate) const MAX_DELAY: f64 = 300.0;
/// A delay under this is none: commentary on the stream itself.
const NO_DELAY: f64 = 0.05;
/// The rate the frames are labelled with: Opus decodes any packet at it; TF2 and pages play it.
const RATE: u16 = 24000;
/// With nothing held and nothing said this long, the feed needs no tending.
const IDLE: Duration = Duration::from_secs(60);

#[derive(Default)]
struct State {
    hold: Option<VoiceHold>,
    /// Seconds added to every delayed source's delay: a room's admins' nudge, by ear.
    nudge: f64,
    /// The stream's count of new starts, and whether it was paused, when last looked at.
    epoch: u64,
    paused: bool,
    /// The tick the game stood still at, for what was said meanwhile.
    paused_at: u32,
    said: Option<Instant>,
}

/// A match's voice from outside it.
#[derive(Default)]
pub(crate) struct VoiceFeed(Mutex<State>);

impl VoiceFeed {
    /// The nudge: seconds added to delayed sources' delays.
    #[cfg(test)]
    pub(crate) fn nudge(&self) -> f64 {
        lock(&self.0).nudge
    }

    /// Sets the nudge, for what is said from now on.
    pub(crate) fn set_nudge(&self, nudge: f64) {
        lock(&self.0).nudge = nudge.clamp(-MAX_NUDGE, MAX_NUDGE);
    }

    /// `who` (a `SteamID`) said `frames` just now, in a source whose stream is `delay` seconds
    /// behind its game (the usual, unless given). With no delay it is commentary on the stream
    /// itself: straight out, paused or not.
    pub(crate) fn said(&self, hub: &SharedHub, who: &str, frames: Vec<String>, delay: Option<f64>, now: Instant) -> Took {
        let delay = delay.unwrap_or(DEFAULT_DELAY);
        if delay < NO_DELAY {
            let Some(tick) = lock(hub).voice_clock().0 else { return Took::NoStream };
            publish(hub, &line(hub, Said { who: who.to_string(), frames }, tick, false));
            return Took::Live;
        }
        self.step(hub, now);
        let interval = lock(hub).tick_interval();
        let mut st = lock(&self.0);
        st.said = Some(now);
        let delay = (delay + st.nudge).clamp(0.0, MAX_DELAY);
        st.hold.get_or_insert_with(|| VoiceHold::new(interval)).said(Said { who: who.to_string(), frames }, delay, now)
    }

    /// Whether nothing is held and nothing was said for a while: no need to tend it.
    pub(crate) fn idle(&self, now: Instant) -> bool {
        let st = lock(&self.0);
        st.hold.as_ref().is_none_or(|h| h.held() == 0) && st.said.is_none_or(|s| now.duration_since(s) > IDLE)
    }

    /// One look at the stream: the hold follows its tick, pauses and new starts, and what is due
    /// goes to the hub; pause talk, once the game goes on, as lines only kept.
    pub(crate) fn step(&self, hub: &SharedHub, now: Instant) {
        let (tick, paused, epoch) = lock(hub).voice_clock();
        let (out, pause_talk, at) = {
            let mut st = lock(&self.0);
            let interval = lock(hub).tick_interval();
            let (was_paused, new_start) = (st.paused, st.epoch != epoch);
            (st.epoch, st.paused) = (epoch, paused);
            if paused && !was_paused {
                st.paused_at = tick.unwrap_or(0);
            }
            let at = st.paused_at;
            let hold = st.hold.get_or_insert_with(|| VoiceHold::new(interval));
            if new_start {
                hold.reset();
            }
            hold.on_pause(paused, now);
            let talk = if was_paused && !paused { hold.pause_talk() } else { Vec::new() };
            (hold.due(tick, now), talk, at)
        };
        for said in pause_talk {
            publish(hub, &line(hub, said, at, true));
        }
        for said in out {
            publish(hub, &line(hub, said, tick.unwrap_or(0), false));
        }
    }
}

/// The most a nudge is, either way.
pub(crate) const MAX_NUDGE: f64 = 30.0;

/// What `said` is as a line at `tick`: its speaker the player with that `SteamID` now, if any.
fn line(hub: &SharedHub, said: Said, tick: u32, pause: bool) -> Line {
    let player = lock(hub).player_with(&said.who).map(|(id, _)| id);
    Line::Voice(Voice { tick, player, steamid: said.who, rate: RATE, frames: said.frames, injected: true, pause, team: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matches::hub::tests::{player, snapshot};
    use crate::matches::hub::{subscribe_all, subscribe_to};
    use serde_json::Value;
    use std::sync::mpsc::Receiver;
    use stvtf::output::{Pause, Status, Team};

    const STEAMID: &str = "[U:1:5]";

    /// A hub whose stream is at `tick`, with player 7 of [`STEAMID`] in it.
    fn at(hub: &SharedHub, tick: u32) {
        let p = stvtf::output::Player { steamid: STEAMID.into(), ..player(7, "a", Team::Red) };
        publish(hub, &snapshot(tick, None, vec![p]));
    }

    fn voices(rx: &Receiver<std::sync::Arc<str>>) -> Vec<Value> {
        rx.try_iter().map(|l| serde_json::from_str::<Value>(&l).unwrap()).filter(|v| v["type"] == "voice").collect()
    }

    fn frames(n: usize) -> Vec<String> {
        vec!["aA==".to_string(); n]
    }

    #[test]
    fn voice_said_to_a_live_match_comes_out_when_its_tick_does() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        assert_eq!(feed.said(&hub, STEAMID, frames(1), None, now), Took::NoStream, "no stream yet");
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        assert_eq!(feed.said(&hub, STEAMID, frames(1), None, now), Took::Held { due: 7000 });
        feed.step(&hub, now);
        at(&hub, 6999);
        feed.step(&hub, now);
        assert_eq!(voices(&page).len(), 0, "not before its tick");
        at(&hub, 7000);
        feed.step(&hub, now);
        let out = voices(&page);
        assert_eq!(out.len(), 1);
        let v = &out[0];
        assert_eq!((v["player"].as_u64(), v["steamid"].as_str(), v["rate"].as_u64()), (Some(7), Some(STEAMID), Some(24000)));
        assert_eq!((v["injected"].as_bool(), v["tick"].as_u64(), v["frames"].as_array().map(Vec::len)), (Some(true), Some(7000), Some(1)));
    }

    #[test]
    fn a_speaker_who_is_no_player_then_goes_out_by_steamid_alone() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        feed.said(&hub, "[U:1:99]", frames(1), None, now);
        at(&hub, 7000);
        feed.step(&hub, now);
        let out = voices(&page);
        assert!(out[0]["player"].is_null() && out[0]["steamid"] == "[U:1:99]", "{out:?}");
    }

    #[test]
    fn pause_talk_is_kept_for_the_recording_and_reaches_no_page() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let (page, recorder) = (subscribe_to(&hub, true, true), subscribe_all(&hub));
        feed.step(&hub, now);
        publish(&hub, &Line::Pause(Pause { tick: 1000, paused: true }));
        feed.step(&hub, now);
        assert_eq!(feed.said(&hub, STEAMID, frames(3), None, now), Took::PauseTalk);
        publish(&hub, &Line::Pause(Pause { tick: 1000, paused: false }));
        feed.step(&hub, now);
        assert_eq!(voices(&page).len(), 0, "no page hears it");
        let kept = voices(&recorder);
        assert_eq!(kept.len(), 1);
        assert_eq!(
            (kept[0]["pause"].as_bool(), kept[0]["tick"].as_u64(), kept[0]["injected"].as_bool()),
            (Some(true), Some(1000), Some(true))
        );
    }

    #[test]
    fn a_new_map_drops_what_is_held() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        feed.said(&hub, STEAMID, frames(1), None, now);
        feed.step(&hub, now);
        publish(&hub, &Line::Status(Status::MapChange));
        let page = subscribe_to(&hub, false, true);
        at(&hub, 9000);
        feed.step(&hub, now);
        assert_eq!(voices(&page).len(), 0);
    }

    #[test]
    fn each_post_says_its_own_delay_else_it_is_the_usual() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        assert_eq!(feed.said(&hub, STEAMID, frames(1), Some(30.0), now), Took::Held { due: 3000 });
        assert_eq!(feed.said(&hub, "[U:1:6]", frames(1), None, now), Took::Held { due: 7000 }, "a team's, 90 s");
    }

    #[test]
    fn voice_with_no_delay_is_live_commentary_straight_out_paused_or_not() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        assert_eq!(feed.said(&hub, "[U:1:99]", frames(2), Some(0.0), now), Took::Live);
        let out = voices(&page);
        assert_eq!(out.len(), 1, "at once");
        assert!(out[0]["player"].is_null() && out[0]["tick"] == 1000 && out[0]["injected"] == true, "{out:?}");
        publish(&hub, &Line::Pause(Pause { tick: 1000, paused: true }));
        assert_eq!(feed.said(&hub, "[U:1:99]", frames(1), Some(0.0), now), Took::Live, "casters see the pause too");
        assert_eq!(voices(&page).len(), 1);
    }
}
