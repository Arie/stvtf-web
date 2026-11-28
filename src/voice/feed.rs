//! A live match's voice from outside it: what the bot posts goes into a [`VoiceHold`], and what
//! the hold lets go is published to the match's hub as `voice` lines (pages that asked, and the
//! recording). The hold goes by the hub's stream: its tick, its pauses, its new starts.
//!
//! What is held outlives the process: it is saved ([`VoiceFeed::save`]) for the next one on the
//! same stream to take ([`VoiceFeed::adopt`]).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use stvtf::Line;
use stvtf::output::{Team, Voice};

use crate::matches::hub::{Hub, NamedPlayer, SharedHub, VoiceClock, publish};
use crate::matches::live_demos::{By, Spoken};
use crate::sync::lock;
use crate::voice::hold::{Held, Said, Took, VoiceHold};
use crate::voice::ids::account_of;

/// How far behind its game a source's stream is unless it says: most leagues' `tv_delay`.
pub(crate) const DEFAULT_DELAY: f64 = 90.0;
/// The longest delay taken: as far as `tv_delay` itself goes.
pub(crate) const MAX_DELAY: f64 = 300.0;
/// A delay under this is none: commentary on the stream itself.
const NO_DELAY: f64 = 0.05;

/// What is left of `delay` (the usual, unless given) for what was said `late` seconds ago: held
/// voice stays held, however late; commentary has none to take from.
pub(crate) fn delay_left(delay: Option<f64>, late: f64) -> f64 {
    let delay = delay.unwrap_or(DEFAULT_DELAY);
    if delay < NO_DELAY { delay } else { (delay - late).max(NO_DELAY) }
}
/// The rate the frames are labelled with: Opus decodes any packet at it; TF2 and pages play it.
const RATE: u16 = 24000;
/// A tick that has not moved for this long means the game is paused.
const STILL: Duration = Duration::from_millis(700);
/// With nothing held and nothing said this long, the feed needs no tending.
const IDLE: Duration = Duration::from_secs(60);
/// Another process on the same stream may be this far behind the one that saved.
const BEHIND: f64 = 5.0;

/// Who said something: their `SteamID` when the bot knows it (their link), who they are where
/// they talk and their name there, and the voice channel they are in.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Speaker {
    pub(crate) steamid: Option<String>,
    pub(crate) voice_id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) channel: Option<String>,
}

/// What a feed holds, as saved for the next process.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct SavedVoice {
    /// The stream's map and tick as it was saved.
    map: Option<String>,
    tick: u32,
    /// Saved as its process stopped: that let nothing go after.
    #[serde(default)]
    last: bool,
    /// The tick the game stood still at, for the pause talk.
    #[serde(default)]
    paused_at: u32,
    /// Those whose voice is held, as they were heard.
    #[serde(default)]
    known: BTreeMap<String, Speaker>,
    held: Held,
}

/// What there is to save of a feed.
pub(crate) enum Save {
    /// As last saved.
    Same,
    /// Nothing is held now, though something was when last saved.
    Nothing,
    Held(Box<SavedVoice>),
}

/// What became of held voice given to a feed.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Adopt {
    Taken,
    /// No stream to hold it against yet.
    NotYet,
    /// Not this stream's: another map, or ticks that started over.
    Not,
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

/// How a speaker with no `SteamID` is named in the hold and on their lines.
pub(crate) const UNLINKED: &str = "discord:";

/// Someone in a channel brought into the match (Discord, Mumble), heard yet or not: for whoever
/// has a say to put on a player (one with no link, or playing on another account).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
    /// What was said, as it went out, for the demo being recorded: until taken.
    spoken: Vec<Spoken>,
    /// The hold's mark when last saved.
    saved: Option<(u64, usize, usize)>,
    /// Saved for the next process as this one stops: nothing comes in or goes out any more.
    closed: bool,
}

impl State {
    /// Those in any channel now: who they are there, and their name.
    fn roster(&self) -> impl Iterator<Item = (&String, &String)> {
        self.rosters.values().flatten()
    }

    /// Those nobody placed here are put on whom they were said to be before, while that one
    /// plays here; said to be nobody before, unless they have a link of their own.
    fn apply_known_before(&mut self, players: &[NamedPlayer]) {
        let playing = |steamid: &str| players.iter().any(|p| p.steamid == steamid);
        let linked = |id: &str| self.known.values().any(|k| k.voice_id.as_deref() == Some(id) && k.steamid.is_some());
        let holding: Vec<(String, Put)> = self
            .known_before
            .iter()
            .filter(|(id, before)| {
                let placed = matches!(self.matched.get(*id), Some(Put::Nobody | Put::Player { confirmed: true, .. }));
                let holds = match before {
                    Put::Player { steamid, .. } => playing(steamid),
                    Put::Nobody => !linked(id),
                };
                !placed && holds
            })
            .map(|(id, before)| (id.clone(), before.clone()))
            .collect();
        self.matched.extend(holding);
    }

    /// Those still not placed are put on the one player whose name looks like theirs, when there
    /// is just one and nobody else has that player: a guess, shown as one until someone says so.
    fn guess_by_name(&mut self, players: &[NamedPlayer]) {
        let playing = |steamid: &str| players.iter().any(|p| p.steamid == steamid);
        let mut people: BTreeMap<String, (String, Option<String>)> =
            self.roster().map(|(id, name)| (id.clone(), (name.clone(), None))).collect();
        for k in self.known.values() {
            if let Some(id) = &k.voice_id {
                let name = k.name.clone().or_else(|| people.get(id).map(|p| p.0.clone())).unwrap_or_default();
                people.insert(id.clone(), (name, k.steamid.clone()));
            }
        }
        let mut taken: Vec<String> =
            self.matched.values().filter_map(|p| if let Put::Player { steamid, .. } = p { Some(steamid.clone()) } else { None }).collect();
        taken.extend(people.values().filter_map(|(_, own)| own.clone()).filter(|s| playing(s)));
        for (id, (name, own)) in people {
            if self.matched.contains_key(&id) || own.is_some_and(|s| playing(&s)) {
                continue;
            }
            let mut like = players.iter().filter(|p| alike(&p.name, &name) && !taken.contains(&p.steamid));
            if let (Some(NamedPlayer { steamid, .. }), None) = (like.next(), like.next()) {
                taken.push(steamid.clone());
                self.matched.insert(id, Put::Player { steamid: steamid.clone(), confirmed: false });
            }
        }
    }

    /// The `SteamID` the speaker known by `key` is put on (by someone, or by their name), else
    /// their own; none when said to be no player, not even the one they linked.
    fn steamid_put_on(&self, key: &str) -> Option<String> {
        let voice_id = key.strip_prefix(UNLINKED).or_else(|| self.known.get(key)?.voice_id.as_deref());
        let own = (!key.starts_with(UNLINKED)).then(|| key.to_string());
        match voice_id.and_then(|d| self.matched.get(d)) {
            Some(Put::Player { steamid, .. }) => Some(steamid.clone()),
            Some(Put::Nobody) => None,
            None => own.or_else(|| match self.known_before.get(voice_id?) {
                Some(Put::Player { steamid, .. }) => Some(steamid.clone()),
                _ => None,
            }),
        }
    }

    /// The side most of the players in voice channel `channel` play on now, if one has more.
    fn channel_team(&self, hub: &Hub, channel: Option<&str>) -> Option<Team> {
        let channel = channel?;
        let (mut red, mut blue) = (0, 0);
        let mates = self.known.iter().filter(|(_, k)| k.channel.as_deref() == Some(channel));
        for (_, team) in mates.filter_map(|(key, _)| hub.player_with(&self.steamid_put_on(key)?)) {
            match team {
                Team::Red => red += 1,
                Team::Blue => blue += 1,
                _ => {}
            }
        }
        (red != blue).then_some(if red > blue { Team::Red } else { Team::Blue })
    }

    /// What a player said, kept for the demo being recorded, while there is room.
    fn record_spoken(&mut self, spoken: Spoken) {
        if self.spoken.len() < MAX_SPOKEN {
            self.spoken.push(spoken);
        }
    }
}

/// The most kept for the demo, when nothing takes it (no demo is recorded).
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
            if st.closed {
                return Took::Closed;
            }
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
    /// one plays here; else by their name.
    fn guess(&self, hub: &SharedHub) {
        let mut st = lock(&self.0);
        let players = lock(hub).players();
        st.apply_known_before(&players);
        st.guess_by_name(&players);
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

    /// Whom `voice_id` was said to be before: they are put on that player here while that player
    /// plays here and nobody placed them. When that player does not play here, a guess by their
    /// name comes first.
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
        let steamid = st.steamid_put_on(&said.who);
        let hub = lock(hub);
        let player = steamid.as_deref().and_then(|s| hub.player_with(s));
        // Not a player: by name, on the side most of their channel plays on.
        let known = st.known.get(&said.who).filter(|_| player.is_none());
        let team = known.and_then(|k| st.channel_team(&hub, k.channel.as_deref()));
        let name = known.and_then(|k| k.name.clone());
        // Said in the running game: what a demo with the comms in it is made of. A player's, by
        // their slot.
        let slot = steamid.as_deref().filter(|_| player.is_some() && !pause).and_then(|s| Some((hub.slot_of(s)?, account_of(s)?)));
        drop(hub);
        // No player's (yet): kept as whose voice it is, for whoever says so later.
        let by = match slot {
            Some((slot, account)) => Some(By::Player { slot, account }),
            None => (!pause).then(|| {
                // By who they are where they talk, a linked one too: that is whom one is put on.
                let voice_id = known.and_then(|k| k.voice_id.as_ref());
                By::Voice { who: voice_id.map_or_else(|| said.who.clone(), |v| format!("{UNLINKED}{v}")), name: name.clone() }
            }),
        };
        if let Some(by) = by {
            st.record_spoken(Spoken { tick, by, frames: said.frames.clone() });
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

    /// What was said since last asked, as it went out.
    pub(crate) fn take_spoken(&self) -> Vec<Spoken> {
        std::mem::take(&mut lock(&self.0).spoken)
    }

    /// Frames held.
    pub(crate) fn held(&self) -> usize {
        lock(&self.0).hold.as_ref().map_or(0, VoiceHold::held)
    }

    /// Whether nothing is held and nothing was said for a while: no need to tend it.
    pub(crate) fn idle(&self, now: Instant) -> bool {
        self.held() == 0 && lock(&self.0).said.is_none_or(|s| now.duration_since(s) > IDLE)
    }

    /// What is held, for the next process, if it changed since last asked; `last`, as this
    /// process stops: always, and from then on nothing comes in or goes out here.
    pub(crate) fn save(&self, hub: &SharedHub, last: bool) -> Save {
        let mut st = lock(&self.0);
        let mark = st.hold.as_ref().map_or_default(VoiceHold::mark);
        if !last && st.saved.unwrap_or_default() == mark {
            return Save::Same;
        }
        st.saved = Some(mark);
        st.closed |= last;
        let held = st.hold.as_ref().map_or_default(VoiceHold::saved);
        if held.is_empty() {
            return Save::Nothing;
        }
        let (map, tick) = {
            let hub = lock(hub);
            (hub.map.clone(), hub.voice_clock().tick.unwrap_or(0))
        };
        let known = st.known.iter().filter(|(key, _)| held.has(key)).map(|(key, who)| (key.clone(), who.clone())).collect();
        Save::Held(Box::new(SavedVoice { map, tick, last, paused_at: st.paused_at, known, held }))
    }

    /// Takes what the feed of the process named `from` saved of this match (see
    /// `VoiceHold::adopt`), once there is a stream; its pause talk at once when the game goes on.
    pub(crate) fn adopt(&self, hub: &SharedHub, from: &str, saved: SavedVoice, now: Instant) -> Adopt {
        // New starts first: the first look at a stream is one.
        self.step(hub, now);
        let pause_talk = {
            let mut st = lock(&self.0);
            let (tick, map, interval) = {
                let hub = lock(hub);
                (hub.voice_clock().tick, hub.map.clone(), hub.tick_interval())
            };
            let Some(tick) = tick else { return Adopt::NotYet };
            let behind = (BEHIND / interval.max(1e-6)) as u32;
            if st.closed || map != saved.map || tick.saturating_add(behind) < saved.tick {
                return Adopt::Not;
            }
            for (key, who) in saved.known {
                if st.known.len() < MAX_KNOWN {
                    st.known.entry(key).or_insert(who);
                }
            }
            st.said = Some(now);
            let paused = st.paused;
            let hold = st.hold.get_or_insert_with(|| VoiceHold::new(interval));
            hold.adopt(from, saved.held, saved.tick, saved.last);
            if paused { Vec::new() } else { hold.pause_talk() }
        };
        for said in pause_talk {
            let line = self.line(hub, said, saved.paused_at, true);
            publish(hub, &line);
        }
        Adopt::Taken
    }

    /// One look at the stream: the hold follows its tick, pauses and new starts, and what is due
    /// goes to the hub; pause talk, once the game goes on, as lines that are kept and not heard.
    pub(crate) fn step(&self, hub: &SharedHub, now: Instant) {
        let (out, pause_talk, at, tick) = {
            // The feed first, then the stream's clock: two looking at once see it in turn.
            let mut st = lock(&self.0);
            if st.closed {
                return;
            }
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
mod tests;
