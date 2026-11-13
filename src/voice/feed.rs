//! A live match's voice from outside it: what the bot posts goes into a [`VoiceHold`], and what
//! the hold lets go is published to the match's hub as `voice` lines (pages that asked, and the
//! recording). The hold goes by the hub's stream: its tick, its pauses, its new starts.

use crate::lock;
use crate::matches::hub::{NamedPlayer, SharedHub, VoiceClock, publish};
use crate::matches::live_demos::Spoken;
use crate::voice::hold::{Said, Took, VoiceHold};
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
/// A tick that has not moved this long is a paused game's.
const STILL: Duration = Duration::from_millis(700);
/// With nothing held and nothing said this long, the feed needs no tending.
const IDLE: Duration = Duration::from_secs(60);

/// Who said something: their `SteamID` when the bot knows it (their link), else who they are/// id and name; and the voice channel they are in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Speaker {
    pub(crate) steamid: Option<String>,
    pub(crate) voice_id: Option<String>,
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
        self.steamid.clone().unwrap_or_else(|| format!("{UNLINKED}{}", self.voice_id.as_deref().unwrap_or_default()))
    }
}

/// The account of `[U:1:<account>]`.
fn account(steamid: &str) -> Option<u32> {
    steamid.strip_prefix("[U:1:")?.strip_suffix(']')?.parse().ok()
}

/// How a speaker with no `SteamID` is named in the hold and on their lines.
const UNLINKED: &str = "discord:";

/// Someone in a channel brought into the match (Discord, Mumble), heard yet or not: for whoever
/// has a say to put on a player (one with no link, or playing on another account).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Person {
    /// Who they are where they talk.
    #[serde(rename = "id")]
    pub(crate) voice_id: String,
    pub(crate) name: String,
    /// The `SteamID` they are put on: by someone, or guessed from their name.
    pub(crate) steamid: Option<String>,
    /// The `SteamID` they linked themselves.
    pub(crate) linked: Option<String>,
    /// Whether they have said anything.
    pub(crate) heard: bool,
    /// Whether whom they are put on (a player, or nobody) was said by someone, not guessed.
    pub(crate) confirmed: bool,
}

/// Whom someone is put on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Put {
    /// The player with this `SteamID`: said so by someone (`confirmed`), or guessed from names.
    Player { steamid: String, confirmed: bool },
    /// Nobody: they are no player, said so by someone.
    Nobody,
}

/// Whether two names look like one: the same letters and digits, or one within the other.
pub(crate) fn alike(a: &str, b: &str) -> bool {
    let plain = |n: &str| n.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let (a, b) = (plain(a), plain(b));
    a.chars().count() > 2 && b.chars().count() > 2 && (a.contains(&b) || b.contains(&a))
}

/// The most speakers remembered (names, channels), and the most channels with people in them.
const MAX_KNOWN: usize = 256;
const MAX_CHANNELS: usize = 8;

impl State {
    /// Those in any channel now: who they are there, and their name.
    fn roster(&self) -> impl Iterator<Item = (&String, &String)> {
        self.rosters.values().flatten()
    }
}

#[derive(Default)]
struct State {
    hold: Option<VoiceHold>,
    /// The speakers heard, by key: their name and channel as last given.
    known: BTreeMap<String, Speaker>,
    /// Whom each is put on, by who they are where they talk.
    matched: BTreeMap<String, Put>,
    /// Those in each channel now, as the bot says: who they are there, and their name.
    rosters: BTreeMap<String, BTreeMap<String, String>>,
    /// Whom each was said to be before, in other matches.
    known_before: BTreeMap<String, Put>,
    /// The stream's count of new starts, and whether it was paused, when last looked at.
    epoch: u64,
    paused: bool,
    /// The tick the game stood still at, for what was said meanwhile.
    paused_at: u32,
    /// The stream's tick, and since when it is that.
    seen: Option<(u32, Instant)>,
    said: Option<Instant>,
    /// What players said, as it went out, for the demo being recorded: until taken.
    spoken: Vec<Spoken>,
}

/// The most kept so, when nothing takes it (no demo is recorded).
const MAX_SPOKEN: usize = 4096;

/// A match's voice from outside it.
#[derive(Default)]
pub(crate) struct VoiceFeed(Mutex<State>);

impl VoiceFeed {
    /// `who` said `frames` just now, in a source whose stream is `delay` seconds behind its game
    /// (the usual, unless given). With no delay it is commentary on the stream itself: straight
    /// out, paused or not.
    pub(crate) fn take_in(&self, hub: &SharedHub, who: &Speaker, frames: Vec<String>, delay: Option<f64>, now: Instant) -> Took {
        let key = who.key();
        {
            let mut st = lock(&self.0);
            if st.known.len() < MAX_KNOWN || st.known.contains_key(&key) {
                st.known.insert(key.clone(), who.clone());
            }
        }
        self.guess(hub);
        let delay = delay.unwrap_or(DEFAULT_DELAY);
        if delay < NO_DELAY {
            let Some(tick) = lock(hub).voice_clock().tick else { return Took::NoStream };
            let line = self.line(hub, Said { who: key, frames }, tick, false);
            publish(hub, &line);
            return Took::Live;
        }
        self.step(hub, now);
        let interval = lock(hub).tick_interval();
        let mut st = lock(&self.0);
        st.said = Some(now);
        let delay = delay.clamp(0.0, MAX_DELAY);
        st.hold.get_or_insert_with(|| VoiceHold::new(interval)).take_in(Said { who: key, frames }, delay, now)
    }

    /// Those in the channel are these now (the bot says so, before anyone speaks): each by who
    /// they are there, with their name.
    pub(crate) fn set_roster(&self, hub: &SharedHub, channel: &str, people: impl IntoIterator<Item = (String, String)>) {
        let people: BTreeMap<String, String> = people.into_iter().take(MAX_KNOWN).collect();
        {
            let mut st = lock(&self.0);
            if people.is_empty() {
                st.rosters.remove(channel);
            } else if st.rosters.len() < MAX_CHANNELS || st.rosters.contains_key(channel) {
                st.rosters.insert(channel.to_string(), people);
            }
        }
        self.guess(hub);
    }

    /// Puts those nobody placed here on a player: the one they were said to be before, if that
    /// one plays here; else the one player whose name looks like theirs, when there is just one
    /// and nobody else has that player: a guess, shown as one until someone says so.
    fn guess(&self, hub: &SharedHub) {
        let mut st = lock(&self.0);
        let players = lock(hub).players();
        let playing = |steamid: &str| players.iter().any(|p| p.steamid == steamid);
        for (id, before) in st.known_before.clone() {
            let placed = matches!(st.matched.get(&id), Some(Put::Nobody | Put::Player { confirmed: true, .. }));
            // Said to be nobody before: not for someone with a link of their own.
            let linked = || st.known.values().any(|k| k.voice_id.as_deref() == Some(&id) && k.steamid.is_some());
            let holds = match &before {
                Put::Player { steamid, .. } => playing(steamid),
                Put::Nobody => !linked(),
            };
            if !placed && holds {
                st.matched.insert(id, before);
            }
        }
        let mut people: BTreeMap<String, (String, Option<String>)> =
            st.roster().map(|(id, name)| (id.clone(), (name.clone(), None))).collect();
        for k in st.known.values() {
            if let Some(id) = &k.voice_id {
                let name = k.name.clone().or_else(|| people.get(id).map(|p| p.0.clone())).unwrap_or_default();
                people.insert(id.clone(), (name, k.steamid.clone()));
            }
        }
        let mut taken: Vec<String> =
            st.matched.values().filter_map(|p| if let Put::Player { steamid, .. } = p { Some(steamid.clone()) } else { None }).collect();
        taken.extend(people.values().filter_map(|(_, own)| own.clone()).filter(|s| playing(s)));
        for (id, (name, own)) in people {
            if st.matched.contains_key(&id) || own.is_some_and(|s| playing(&s)) {
                continue;
            }
            let mut like = players.iter().filter(|p| alike(&p.name, &name) && !taken.contains(&p.steamid));
            if let (Some(NamedPlayer { steamid, .. }), None) = (like.next(), like.next()) {
                taken.push(steamid.clone());
                st.matched.insert(id, Put::Player { steamid: steamid.clone(), confirmed: false });
            }
        }
    }

    /// Those in the channel and those heard, their own link, and whom each is put on.
    pub(crate) fn voices(&self, hub: &SharedHub) -> Vec<Person> {
        self.guess(hub);
        let st = lock(&self.0);
        let put = |id: &str| match st.matched.get(id) {
            Some(Put::Player { steamid, confirmed }) => (Some(steamid.clone()), *confirmed),
            Some(Put::Nobody) => (None, true),
            // Known from before, and no player here: as known.
            None => match st.known_before.get(id) {
                Some(Put::Player { steamid, confirmed }) => (Some(steamid.clone()), *confirmed),
                _ => (None, false),
            },
        };
        let mut all: BTreeMap<String, Person> = BTreeMap::new();
        for (id, name) in st.roster() {
            let (steamid, confirmed) = put(id);
            all.insert(id.clone(), Person { voice_id: id.clone(), name: name.clone(), steamid, linked: None, heard: false, confirmed });
        }
        // Once each: someone who linked meanwhile is known both ways, the linked one last.
        for s in st.known.values() {
            let Some(voice_id) = s.voice_id.clone() else { continue };
            let before = all.remove(&voice_id);
            let name = s.name.clone().or_else(|| before.as_ref().map(|h| h.name.clone())).unwrap_or_else(|| voice_id.clone());
            let linked = s.steamid.clone().or_else(|| before.and_then(|h| h.linked));
            let (steamid, confirmed) = put(&voice_id);
            all.insert(voice_id.clone(), Person { voice_id, name, steamid, linked, heard: true, confirmed });
        }
        all.into_values().collect()
    }

    /// Who is in the channels now, each as who they are where they talk.
    pub(crate) fn roster_ids(&self) -> Vec<String> {
        lock(&self.0).roster().map(|(id, _)| id.clone()).collect()
    }

    /// The name `voice_id` talks under, in the channel or as heard.
    pub(crate) fn name_of(&self, voice_id: &str) -> Option<String> {
        let st = lock(&self.0);
        let heard = || st.known.values().find(|s| s.voice_id.as_deref() == Some(voice_id)).and_then(|s| s.name.clone());
        st.roster().find(|(id, _)| *id == voice_id).map(|(_, name)| name.clone()).or_else(heard)
    }

    /// Someone says whom `voice_id` is: a player's `SteamID`, or nobody. Whether there is such a
    /// person, in the channel or heard.
    pub(crate) fn put_on(&self, voice_id: &str, steamid: Option<String>) -> bool {
        let mut st = lock(&self.0);
        if !st.roster().any(|(id, _)| id == voice_id) && !st.known.values().any(|s| s.voice_id.as_deref() == Some(voice_id)) {
            return false;
        }
        let put = steamid.map_or(Put::Nobody, |steamid| Put::Player { steamid, confirmed: true });
        st.matched.insert(voice_id.to_string(), put);
        true
    }

    /// Whom `voice_id` was said to be before: theirs here while that one plays here and nobody
    /// placed them; with that one not playing, their name is gone by first.
    pub(crate) fn set_known_before(&self, voice_id: &str, before: Put) {
        let mut st = lock(&self.0);
        if st.known_before.len() < MAX_KNOWN || st.known_before.contains_key(voice_id) {
            st.known_before.insert(voice_id.to_string(), before);
        }
    }

    /// What `said` is as a line at `tick`: its speaker the player with their `SteamID` now (their
    /// own, or the one they were matched to); else by name, on the team most of their channel's
    /// players are on.
    fn line(&self, hub: &SharedHub, said: Said, tick: u32, pause: bool) -> Line {
        let mut st = lock(&self.0);
        let known = st.known.get(&said.who).cloned().unwrap_or_default();
        // The one they are put on (by someone, or by their name), else their own.
        let put_on_steamid = |key: &str| {
            let voice_id = key.strip_prefix(UNLINKED).or_else(|| st.known.get(key)?.voice_id.as_deref());
            let own = (!key.starts_with(UNLINKED)).then(|| key.to_string());
            match voice_id.and_then(|d| st.matched.get(d)) {
                Some(Put::Player { steamid, .. }) => Some(steamid.clone()),
                // Said to be no player: not even the one they linked.
                Some(Put::Nobody) => None,
                None => own.or_else(|| match st.known_before.get(voice_id?) {
                    Some(Put::Player { steamid, .. }) => Some(steamid.clone()),
                    _ => None,
                }),
            }
        };
        let steamid = put_on_steamid(&said.who);
        let hub = lock(hub);
        let player = steamid.as_deref().and_then(|s| hub.player_with(s));
        let (team, name) = if player.is_some() {
            (None, None)
        } else {
            // The sides those in their channel play on now.
            let (mut red, mut blue) = (0, 0);
            let mates = st.known.iter().filter(|(_, k)| known.channel.is_some() && k.channel == known.channel);
            for (_, team) in mates.filter_map(|(key, _)| hub.player_with(&put_on_steamid(key)?)) {
                match team {
                    Team::Red => red += 1,
                    Team::Blue => blue += 1,
                    _ => {}
                }
            }
            let team = (red != blue).then_some(if red > blue { Team::Red } else { Team::Blue });
            (team, known.name.clone())
        };
        // A player's, said in the running game: what a demo with the comms in it is made of.
        let slot = steamid.as_deref().filter(|_| player.is_some() && !pause).and_then(|s| Some((hub.slot_of(s)?, account(s)?)));
        drop(hub);
        if let Some((slot, account)) = slot
            && st.spoken.len() < MAX_SPOKEN
        {
            st.spoken.push(Spoken { tick, slot, account, frames: said.frames.clone() });
        }
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

    /// What players said since last asked, as it went out.
    pub(crate) fn take_spoken(&self) -> Vec<Spoken> {
        std::mem::take(&mut lock(&self.0).spoken)
    }

    /// Whether nothing is held and nothing was said for a while: no need to tend it.
    pub(crate) fn idle(&self, now: Instant) -> bool {
        let st = lock(&self.0);
        st.hold.as_ref().is_none_or(|h| h.held() == 0) && st.said.is_none_or(|s| now.duration_since(s) > IDLE)
    }

    /// One look at the stream: the hold follows its tick, pauses and new starts, and what is due
    /// goes to the hub; pause talk, once the game goes on, as lines only kept.
    pub(crate) fn step(&self, hub: &SharedHub, now: Instant) {
        let (out, pause_talk, at, tick) = {
            // The feed first, then the stream's clock: two looking at once see it in turn.
            let mut st = lock(&self.0);
            let (VoiceClock { tick, paused, epoch }, interval) = {
                let hub = lock(hub);
                (hub.voice_clock(), hub.tick_interval())
            };
            // SourceTV says a pause only as late as the rest, or not at all: its tick standing
            // still is one too, from when it last moved.
            let (still, since) = match st.seen {
                Some((t, since)) if Some(t) == tick => (now.saturating_duration_since(since) > STILL, since),
                _ => {
                    st.seen = tick.map(|t| (t, now));
                    (false, now)
                }
            };
            let since = if still && !paused { since } else { now };
            let paused = paused || still;
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
            hold.on_pause(paused, now, since);
            let talk = if was_paused && !paused { hold.pause_talk() } else { Vec::new() };
            (hold.due(tick, now), talk, at, tick)
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
        assert_eq!(feed.take_in(&hub, &STEAMID.into(), frames(1), None, now), Took::NoStream, "no stream yet");
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        assert_eq!(feed.take_in(&hub, &STEAMID.into(), frames(1), None, now), Took::Held { due: 7000 });
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
        feed.take_in(&hub, &"[U:1:99]".into(), frames(1), None, now);
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
        assert_eq!(feed.take_in(&hub, &STEAMID.into(), frames(3), None, now), Took::PauseTalk);
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
        feed.take_in(&hub, &STEAMID.into(), frames(1), None, now);
        feed.step(&hub, now);
        publish(&hub, &Line::Status(Status::MapChange));
        let page = subscribe_to(&hub, false, true);
        at(&hub, 9000);
        feed.step(&hub, now);
        assert_eq!(voices(&page).len(), 0);
    }

    /// Someone in Discord channel 9 with no Steam link.
    fn unlinked(voice_id: &str, name: &str) -> Speaker {
        Speaker { steamid: None, voice_id: Some(voice_id.into()), name: Some(name.into()), channel: Some("9".into()) }
    }

    #[test]
    fn someone_with_no_link_is_heard_by_name_on_the_team_most_of_their_channel_is_on() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        let mate = Speaker { channel: Some("9".into()), ..STEAMID.into() };
        feed.take_in(&hub, &mate, frames(1), Some(0.0), now);
        feed.take_in(&hub, &unlinked("77", "coach"), frames(1), Some(0.0), now);
        let out = voices(&page);
        let v = &out[1];
        assert!(v["player"].is_null(), "{v}");
        assert_eq!((v["steamid"].as_str(), v["name"].as_str(), v["team"].as_str()), (Some("discord:77"), Some("coach"), Some("red")));
        assert_eq!(
            feed.voices(&hub),
            [Person { voice_id: "77".into(), name: "coach".into(), steamid: None, linked: None, heard: true, confirmed: false }],
            "for a room's admins to place"
        );
    }

    #[test]
    fn someone_with_no_link_follows_their_channel_when_the_teams_swap_sides() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        let mate = Speaker { channel: Some("9".into()), ..STEAMID.into() };
        for _ in 0..5 {
            feed.take_in(&hub, &mate, frames(1), Some(0.0), now);
        }
        let swapped = stvtf::output::Player { steamid: STEAMID.into(), ..player(7, "a", Team::Blue) };
        publish(&hub, &snapshot(2000, None, vec![swapped]));
        feed.take_in(&hub, &unlinked("77", "coach"), frames(1), Some(0.0), now);
        assert_eq!(voices(&page).last().unwrap()["team"], "blue");
        let alone = Speaker { channel: None, ..unlinked("78", "caster") };
        feed.take_in(&hub, &alone, frames(1), Some(0.0), now);
        assert!(voices(&page)[0]["team"].is_null(), "in no channel, on no team");
    }

    #[test]
    fn a_tick_standing_still_is_a_pause_though_the_stream_says_none() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let (page, recorder) = (subscribe_to(&hub, true, true), subscribe_all(&hub));
        feed.step(&hub, now);
        assert_eq!(feed.take_in(&hub, &STEAMID.into(), frames(1), None, now + Duration::from_millis(500)), Took::Held { due: 7000 });
        let later = now + Duration::from_secs(2);
        assert_eq!(feed.take_in(&hub, &STEAMID.into(), frames(3), None, later), Took::PauseTalk);
        at(&hub, 1001);
        feed.step(&hub, later);
        assert_eq!(voices(&page).len(), 0, "no page hears it");
        assert_eq!(voices(&recorder)[0]["pause"], true);
        assert_eq!(feed.take_in(&hub, &STEAMID.into(), frames(1), None, later), Took::Held { due: 7001 }, "and on it goes");
    }

    #[test]
    fn someone_linked_who_is_no_player_is_put_on_one_by_hand_too() {
        // On another account than the one they linked, say.
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        let alt = Speaker { steamid: Some("[U:1:99]".into()), ..unlinked("77", "arie") };
        feed.take_in(&hub, &alt, frames(1), Some(0.0), now);
        assert!(voices(&page)[0]["player"].is_null());
        let heard = Person {
            voice_id: "77".into(),
            name: "arie".into(),
            steamid: None,
            linked: Some("[U:1:99]".into()),
            heard: true,
            confirmed: false,
        };
        assert_eq!(feed.voices(&hub), [heard]);
        assert!(feed.put_on("77", Some(STEAMID.into())));
        feed.take_in(&hub, &alt, frames(1), Some(0.0), now);
        let out = voices(&page);
        assert_eq!((out[0]["player"].as_u64(), out[0]["steamid"].as_str()), (Some(7), Some(STEAMID)));
        assert!(feed.put_on("77", None));
        feed.take_in(&hub, &alt, frames(1), Some(0.0), now);
        assert_eq!(voices(&page)[0]["steamid"], "[U:1:99]", "their own again");
    }

    #[test]
    fn what_players_say_is_handed_on_to_be_kept_with_the_demo() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        // A player (entity 7, so slot 6), someone who is no player, and pause talk.
        feed.take_in(&hub, &STEAMID.into(), frames(2), Some(0.0), now);
        feed.take_in(&hub, &unlinked("77", "coach"), frames(1), Some(0.0), now);
        let kept = feed.take_spoken();
        assert_eq!(kept, [Spoken { tick: 1000, slot: 6, account: 5, frames: frames(2) }]);
        assert_eq!(feed.take_spoken(), [], "once");
        publish(&hub, &Line::Pause(Pause { tick: 1000, paused: true }));
        feed.step(&hub, now);
        feed.take_in(&hub, &STEAMID.into(), frames(1), None, now);
        publish(&hub, &Line::Pause(Pause { tick: 1000, paused: false }));
        feed.step(&hub, now);
        assert_eq!(feed.take_spoken(), [], "not what was said in a pause");
    }

    #[test]
    fn those_in_the_channel_are_put_on_players_before_they_speak_by_their_names_or_by_someone() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        let playing = |id, name: &str, steamid: &str| stvtf::output::Player { steamid: steamid.into(), ..player(id, name, Team::Red) };
        publish(
            &hub,
            &snapshot(
                1000,
                None,
                vec![playing(7, "^_^ gummy@lan", STEAMID), playing(8, "Aoki", "[U:1:6]"), playing(9, "aoki2", "[U:1:7]")],
            ),
        );
        let page = subscribe_to(&hub, false, true);
        feed.set_roster(
            &hub,
            "c",
            [("77".to_string(), "Gummy".to_string()), ("78".to_string(), "aoki".to_string()), ("79".to_string(), "dave".to_string())],
        );
        let who: Vec<_> = feed.voices(&hub).into_iter().map(|h| (h.name, h.steamid, h.heard, h.confirmed)).collect();
        assert_eq!(
            who,
            [
                ("Gummy".to_string(), Some(STEAMID.to_string()), false, false),
                ("aoki".to_string(), None, false, false),
                ("dave".to_string(), None, false, false),
            ],
            "one name like theirs is a guess; two are none"
        );
        // A guess counts until someone says otherwise: what they say is that player's.
        feed.take_in(&hub, &unlinked("77", "Gummy"), frames(1), Some(0.0), now);
        assert_eq!(voices(&page)[0]["player"], 7);
        // Someone says who is who, a player or nobody: confirmed, and no guess comes in its place.
        assert!(feed.put_on("78", Some("[U:1:6]".into())) && feed.put_on("77", None));
        let who: Vec<_> = feed.voices(&hub).into_iter().map(|h| (h.name, h.steamid, h.confirmed)).collect();
        assert_eq!(who[0], ("Gummy".to_string(), None, true));
        assert_eq!(who[1], ("aoki".to_string(), Some("[U:1:6]".to_string()), true));
        feed.take_in(&hub, &unlinked("77", "Gummy"), frames(1), Some(0.0), now);
        assert!(voices(&page)[0]["player"].is_null(), "said to be no player");
        // Known from before, for someone not placed here: as said by someone.
        feed.set_known_before("79", Put::Player { steamid: "[U:1:7]".into(), confirmed: true });
        feed.set_known_before("78", Put::Player { steamid: "[U:1:99]".into(), confirmed: true });
        let who = feed.voices(&hub);
        assert_eq!((who[2].steamid.as_deref(), who[2].confirmed), (Some("[U:1:7]"), true));
        assert_eq!(who[1].steamid.as_deref(), Some("[U:1:6]"), "what was said here stands");
    }

    #[test]
    fn someone_known_from_before_who_is_not_playing_here_goes_by_their_name_else_as_known() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        let playing = |id, name: &str, steamid: &str| stvtf::output::Player { steamid: steamid.into(), ..player(id, name, Team::Red) };
        publish(&hub, &snapshot(1000, None, vec![playing(7, "gummy", STEAMID)]));
        let page = subscribe_to(&hub, false, true);
        feed.set_roster(&hub, "c", [("77".to_string(), "Gummy".to_string()), ("79".to_string(), "dave".to_string())]);
        // Both known as accounts that are not in this match.
        feed.set_known_before("77", Put::Player { steamid: "[U:1:50]".into(), confirmed: true });
        feed.set_known_before("79", Put::Player { steamid: "[U:1:51]".into(), confirmed: true });
        let who = |feed: &VoiceFeed| feed.voices(&hub).into_iter().map(|h| (h.steamid, h.confirmed)).collect::<Vec<_>>();
        assert_eq!(
            who(&feed),
            [(Some(STEAMID.to_string()), false), (Some("[U:1:51]".to_string()), true)],
            "the player whose name is like theirs, as a guess; else the account known"
        );
        feed.take_in(&hub, &unlinked("77", "Gummy"), frames(1), Some(0.0), now);
        feed.take_in(&hub, &unlinked("79", "dave"), frames(1), Some(0.0), now);
        let heard = voices(&page);
        assert_eq!((&heard[0]["player"], &heard[1]["steamid"]), (&serde_json::json!(7), &serde_json::json!("[U:1:51]")));
        // The account known joins: theirs from then on, over the guess.
        publish(&hub, &snapshot(1001, None, vec![playing(7, "gummy", STEAMID), playing(8, "smurf", "[U:1:50]")]));
        assert_eq!(who(&feed)[0], (Some("[U:1:50]".to_string()), true));
    }

    #[test]
    fn a_suggestion_from_before_shows_as_one_and_nobody_from_before_gets_no_guess() {
        let (hub, feed) = (SharedHub::default(), VoiceFeed::default());
        let playing = |id, name: &str, steamid: &str| stvtf::output::Player { steamid: steamid.into(), ..player(id, name, Team::Red) };
        publish(&hub, &snapshot(1000, None, vec![playing(7, "gummy", STEAMID), playing(8, "coach", "[U:1:6]")]));
        feed.set_roster(&hub, "c", [("77".to_string(), "someone".to_string()), ("78".to_string(), "coach".to_string())]);
        feed.set_known_before("77", Put::Player { steamid: STEAMID.into(), confirmed: false });
        feed.set_known_before("78", Put::Nobody);
        let who: Vec<_> = feed.voices(&hub).into_iter().map(|h| (h.steamid, h.confirmed)).collect();
        assert_eq!(who, [(Some(STEAMID.to_string()), false), (None, true)]);
    }

    #[test]
    fn names_look_alike_when_one_is_in_the_other() {
        assert!(alike("Gummy", "^_^ gummy@lan") && alike("ÄRIE", "ärie"));
        assert!(!alike("gummy", "arie") && !alike("a", "arie") && !alike("", ""));
    }

    #[test]
    fn only_a_voice_heard_is_matched() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        assert!(!feed.put_on("77", Some(STEAMID.into())));
        feed.take_in(&hub, &unlinked("77", "arie"), frames(1), None, now);
        assert!(feed.put_on("77", Some(STEAMID.into())));
    }

    #[test]
    fn someone_matched_by_hand_is_heard_as_that_player_from_then_on() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        feed.take_in(&hub, &unlinked("77", "arie"), frames(1), None, now);
        feed.put_on("77", Some(STEAMID.into()));
        at(&hub, 7000);
        feed.step(&hub, now);
        let out = voices(&page);
        assert_eq!((out[0]["player"].as_u64(), out[0]["steamid"].as_str()), (Some(7), Some(STEAMID)), "what was held too");
        assert_eq!(feed.voices(&hub)[0].steamid.as_deref(), Some(STEAMID));
        feed.put_on("77", None);
        feed.take_in(&hub, &unlinked("77", "arie"), frames(1), Some(0.0), now);
        assert!(voices(&page)[0]["player"].is_null(), "and no more once unmatched");
    }

    #[test]
    fn each_post_says_its_own_delay_else_it_is_the_usual() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        assert_eq!(feed.take_in(&hub, &STEAMID.into(), frames(1), Some(30.0), now), Took::Held { due: 3000 });
        assert_eq!(feed.take_in(&hub, &"[U:1:6]".into(), frames(1), None, now), Took::Held { due: 7000 }, "a team's, 90 s");
    }

    #[test]
    fn voice_with_no_delay_is_live_commentary_straight_out_paused_or_not() {
        let (hub, feed, now) = (SharedHub::default(), VoiceFeed::default(), Instant::now());
        at(&hub, 1000);
        let page = subscribe_to(&hub, false, true);
        assert_eq!(feed.take_in(&hub, &"[U:1:99]".into(), frames(2), Some(0.0), now), Took::Live);
        let out = voices(&page);
        assert_eq!(out.len(), 1, "at once");
        assert!(out[0]["player"].is_null() && out[0]["tick"] == 1000 && out[0]["injected"] == true, "{out:?}");
        publish(&hub, &Line::Pause(Pause { tick: 1000, paused: true }));
        assert_eq!(feed.take_in(&hub, &"[U:1:99]".into(), frames(1), Some(0.0), now), Took::Live, "casters see the pause too");
        assert_eq!(voices(&page).len(), 1);
    }
}
