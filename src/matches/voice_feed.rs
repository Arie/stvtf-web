//! A live match's voice from outside it: what the bot posts goes into a [`VoiceHold`], and what
//! the hold lets go is published to the match's hub as `voice` lines (pages that asked, and the
//! recording). The hold goes by the hub's stream: its tick, its pauses, its new starts.

use crate::lock;
use crate::matches::hub::{SharedHub, publish};
use crate::matches::voice_hold::{Said, Took, VoiceHold};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use stvtf::Line;
use stvtf::output::{Team, Voice};

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

/// Who said something: their `SteamID` when the bot knows it (their link), else their Discord
/// id and name; and the voice channel they are in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Speaker {
    pub(crate) steamid: Option<String>,
    pub(crate) discord: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) channel: Option<String>,
}

impl From<&str> for Speaker {
    /// A speaker known by `SteamID` alone.
    fn from(steamid: &str) -> Self {
        Speaker { steamid: Some(steamid.to_string()), ..Speaker::default() }
    }
}

impl Speaker {
    /// What the hold knows them by: their `SteamID`, else `discord:<id>`.
    fn key(&self) -> String {
        self.steamid.clone().unwrap_or_else(|| format!("{DISCORD}{}", self.discord.as_deref().unwrap_or_default()))
    }
}

/// How a speaker with no `SteamID` is named in the hold and on their lines.
const DISCORD: &str = "discord:";

/// Someone heard with no link of their own: for a room's admins to match to a player.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Heard {
    pub(crate) discord: String,
    pub(crate) name: String,
    /// The `SteamID` an admin matched them to.
    pub(crate) steamid: Option<String>,
}

/// The most speakers remembered (names, channels).
const MAX_KNOWN: usize = 256;

#[derive(Default)]
struct State {
    hold: Option<VoiceHold>,
    /// The speakers heard, by key: their name and channel as last given.
    known: BTreeMap<String, Speaker>,
    /// Discord ids matched by hand to a `SteamID`.
    matched: BTreeMap<String, String>,
    /// Per channel, how many lines went out as a RED and as a BLU player's.
    teams: BTreeMap<String, (u32, u32)>,
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

    /// `who` said `frames` just now, in a source whose stream is `delay` seconds behind its game
    /// (the usual, unless given). With no delay it is commentary on the stream itself: straight
    /// out, paused or not.
    pub(crate) fn said(&self, hub: &SharedHub, who: &Speaker, frames: Vec<String>, delay: Option<f64>, now: Instant) -> Took {
        let key = who.key();
        {
            let mut st = lock(&self.0);
            if st.known.len() < MAX_KNOWN || st.known.contains_key(&key) {
                st.known.insert(key.clone(), who.clone());
            }
        }
        let delay = delay.unwrap_or(DEFAULT_DELAY);
        if delay < NO_DELAY {
            let Some(tick) = lock(hub).voice_clock().0 else { return Took::NoStream };
            let line = self.line(hub, Said { who: key, frames }, tick, false);
            publish(hub, &line);
            return Took::Live;
        }
        self.step(hub, now);
        let interval = lock(hub).tick_interval();
        let mut st = lock(&self.0);
        st.said = Some(now);
        let delay = (delay + st.nudge).clamp(0.0, MAX_DELAY);
        st.hold.get_or_insert_with(|| VoiceHold::new(interval)).said(Said { who: key, frames }, delay, now)
    }

    /// Those heard with no link of their own, and whom an admin matched them to.
    pub(crate) fn voices(&self) -> Vec<Heard> {
        let st = lock(&self.0);
        st.known
            .values()
            .filter(|s| s.steamid.is_none())
            .filter_map(|s| {
                let discord = s.discord.clone()?;
                let steamid = st.matched.get(&discord).cloned();
                Some(Heard { name: s.name.clone().unwrap_or_else(|| discord.clone()), discord, steamid })
            })
            .collect()
    }

    /// Matches Discord user `discord` to `steamid` by hand, or with none, no more.
    pub(crate) fn set_match(&self, discord: &str, steamid: Option<String>) {
        let mut st = lock(&self.0);
        match steamid {
            Some(s) => st.matched.insert(discord.to_string(), s),
            None => st.matched.remove(discord),
        };
    }

    /// What `said` is as a line at `tick`: its speaker the player with their `SteamID` now (their
    /// own, or the one they were matched to); else by name, on the team most of their channel's
    /// players are on.
    fn line(&self, hub: &SharedHub, said: Said, tick: u32, pause: bool) -> Line {
        let mut st = lock(&self.0);
        let known = st.known.get(&said.who).cloned().unwrap_or_default();
        let steamid = match said.who.strip_prefix(DISCORD) {
            Some(discord) => st.matched.get(discord).cloned(),
            None => Some(said.who.clone()),
        };
        let player = steamid.as_deref().and_then(|s| lock(hub).player_with(s));
        let channel = known.channel.clone().unwrap_or_default();
        let (team, name) = if let Some((_, team)) = player {
            let tally = st.teams.entry(channel).or_default();
            match team {
                Team::Red => tally.0 += 1,
                Team::Blue => tally.1 += 1,
                _ => {}
            }
            (None, None)
        } else {
            let (red, blue) = st.teams.get(&channel).copied().unwrap_or_default();
            let team = (red != blue).then_some(if red > blue { Team::Red } else { Team::Blue });
            (team, known.name.clone())
        };
        Line::Voice(Voice {
            tick,
            player: player.map(|(id, _)| id),
            steamid: steamid.unwrap_or(said.who),
            rate: RATE,
            frames: said.frames,
            injected: true,
            pause,
            team,
            name,
        })
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
            let line = self.line(hub, said, at, true);
            publish(hub, &line);
        }
        for said in out {
            let line = self.line(hub, said, tick.unwrap_or(0), false);
            publish(hub, &line);
        }
    }
}

/// The most a nudge is, either way.
pub(crate) const MAX_NUDGE: f64 = 30.0;

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
        assert_eq!(feed.said(&hub, &STEAMID.into(), frames(1), None, now), Took::NoStream, "no stream yet");
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        assert_eq!(feed.said(&hub, &STEAMID.into(), frames(1), None, now), Took::Held { due: 7000 });
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
        feed.said(&hub, &"[U:1:99]".into(), frames(1), None, now);
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
        assert_eq!(feed.said(&hub, &STEAMID.into(), frames(3), None, now), Took::PauseTalk);
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
        feed.said(&hub, &STEAMID.into(), frames(1), None, now);
        feed.step(&hub, now);
        publish(&hub, &Line::Status(Status::MapChange));
        let page = subscribe_to(&hub, false, true);
        at(&hub, 9000);
        feed.step(&hub, now);
        assert_eq!(voices(&page).len(), 0);
    }

    /// Someone in Discord channel 9 with no Steam link.
    fn unlinked(discord: &str, name: &str) -> Speaker {
        Speaker { steamid: None, discord: Some(discord.into()), name: Some(name.into()), channel: Some("9".into()) }
    }

    #[test]
    fn someone_with_no_link_is_heard_by_name_on_the_team_most_of_their_channel_is_on() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        let mate = Speaker { channel: Some("9".into()), ..STEAMID.into() };
        feed.said(&hub, &mate, frames(1), Some(0.0), now);
        feed.said(&hub, &unlinked("77", "coach"), frames(1), Some(0.0), now);
        let out = voices(&page);
        let v = &out[1];
        assert!(v["player"].is_null(), "{v}");
        assert_eq!((v["steamid"].as_str(), v["name"].as_str(), v["team"].as_str()), (Some("discord:77"), Some("coach"), Some("red")));
        assert_eq!(feed.voices(), [Heard { discord: "77".into(), name: "coach".into(), steamid: None }], "for a room's admins to place");
    }

    #[test]
    fn someone_matched_by_hand_is_heard_as_that_player_from_then_on() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        feed.said(&hub, &unlinked("77", "arie"), frames(1), None, now);
        feed.set_match("77", Some(STEAMID.into()));
        at(&hub, 7000);
        feed.step(&hub, now);
        let out = voices(&page);
        assert_eq!((out[0]["player"].as_u64(), out[0]["steamid"].as_str()), (Some(7), Some(STEAMID)), "what was held too");
        assert_eq!(feed.voices()[0].steamid.as_deref(), Some(STEAMID));
        feed.set_match("77", None);
        feed.said(&hub, &unlinked("77", "arie"), frames(1), Some(0.0), now);
        assert!(voices(&page)[0]["player"].is_null(), "and no more once unmatched");
    }

    #[test]
    fn each_post_says_its_own_delay_else_it_is_the_usual() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        assert_eq!(feed.said(&hub, &STEAMID.into(), frames(1), Some(30.0), now), Took::Held { due: 3000 });
        assert_eq!(feed.said(&hub, &"[U:1:6]".into(), frames(1), None, now), Took::Held { due: 7000 }, "a team's, 90 s");
    }

    #[test]
    fn voice_with_no_delay_is_live_commentary_straight_out_paused_or_not() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        assert_eq!(feed.said(&hub, &"[U:1:99]".into(), frames(2), Some(0.0), now), Took::Live);
        let out = voices(&page);
        assert_eq!(out.len(), 1, "at once");
        assert!(out[0]["player"].is_null() && out[0]["tick"] == 1000 && out[0]["injected"] == true, "{out:?}");
        publish(&hub, &Line::Pause(Pause { tick: 1000, paused: true }));
        assert_eq!(feed.said(&hub, &"[U:1:99]".into(), frames(1), Some(0.0), now), Took::Live, "casters see the pause too");
        assert_eq!(voices(&page).len(), 1);
    }
}
