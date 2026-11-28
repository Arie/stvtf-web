use std::sync::mpsc::Receiver;

use serde_json::Value;
use stvtf::output::{Pause, Status, Team};

use super::*;
use crate::matches::hub::tests::{player, snapshot};
use crate::matches::hub::{subscribe_all, subscribe_to};

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
    assert_eq!((kept[0]["pause"].as_bool(), kept[0]["tick"].as_u64(), kept[0]["injected"].as_bool()), (Some(true), Some(1000), Some(true)));
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
    let coach = Spoken { tick: 1000, by: By::Voice { who: "discord:77".into(), name: Some("coach".into()) }, frames: frames(1) };
    assert_eq!(kept, [Spoken { tick: 1000, by: By::Player { slot: 6, account: 5 }, frames: frames(2) }, coach]);
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
        &snapshot(1000, None, vec![playing(7, "^_^ gummy@lan", STEAMID), playing(8, "Aoki", "[U:1:6]"), playing(9, "aoki2", "[U:1:7]")]),
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

/// A hub on `map`, its stream at `tick`, as a process just connected has it.
fn on(map: &str, tick: u32) -> SharedHub {
    let hub = SharedHub::default();
    publish(&hub, &crate::testing::server(map));
    at(&hub, tick);
    hub
}

/// What `feed` saves, as the next process reads it.
fn handed_on(feed: &VoiceFeed, hub: &SharedHub, last: bool) -> SavedVoice {
    let Save::Held(saved) = feed.save(hub, last) else { panic!("nothing to save") };
    serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap()
}

#[test]
fn what_a_feed_held_as_its_process_stopped_comes_out_of_the_next_at_its_tick() {
    let (old_hub, old, now) = (on("cp_process_f12", 1000), VoiceFeed::default(), Instant::now());
    assert_eq!(old.take_in(&old_hub, &unlinked("77", "coach"), frames(2), None, now), Took::Held { due: 7000 });
    let saved = handed_on(&old, &old_hub, true);
    // The next process is on the stream a little later.
    let (hub, new) = (on("cp_process_f12", 1400), VoiceFeed::default());
    let page = subscribe_to(&hub, false, true);
    assert_eq!(new.adopt(&hub, "old", saved, now), Adopt::Taken);
    assert!(!new.idle(now), "it needs tending");
    at(&hub, 6999);
    new.step(&hub, now);
    assert_eq!(voices(&page).len(), 0, "not before its tick");
    at(&hub, 7000);
    new.step(&hub, now);
    let out = voices(&page);
    assert_eq!((out.len(), out[0]["tick"].as_u64(), out[0]["name"].as_str()), (1, Some(7000), Some("coach")), "{out:?}");
}

#[test]
fn a_feed_saved_as_its_process_stops_takes_no_more_and_lets_nothing_go() {
    let (hub, feed, now) = (on("cp_process_f12", 1000), VoiceFeed::default(), Instant::now());
    feed.take_in(&hub, &STEAMID.into(), frames(1), None, now);
    let page = subscribe_to(&hub, false, true);
    handed_on(&feed, &hub, true);
    assert_eq!(feed.take_in(&hub, &STEAMID.into(), frames(1), None, now), Took::Closed, "the next process has it");
    at(&hub, 7000);
    feed.step(&hub, now);
    assert_eq!(voices(&page).len(), 0, "the next process plays it");
}

#[test]
fn a_feed_is_saved_again_only_when_what_it_holds_changed() {
    let (hub, feed, now) = (on("cp_process_f12", 1000), VoiceFeed::default(), Instant::now());
    assert!(matches!(feed.save(&hub, false), Save::Same), "nothing held, nothing saved");
    feed.take_in(&hub, &STEAMID.into(), frames(1), None, now);
    assert!(matches!(feed.save(&hub, false), Save::Held(_)));
    assert!(matches!(feed.save(&hub, false), Save::Same));
    at(&hub, 7000);
    feed.step(&hub, now);
    assert!(matches!(feed.save(&hub, false), Save::Nothing), "it went out: what was saved is not held now");
    assert!(matches!(feed.save(&hub, false), Save::Same));
}

#[test]
fn held_voice_is_not_taken_on_another_map_or_a_stream_that_started_over() {
    let (old_hub, old, now) = (on("cp_process_f12", 9000), VoiceFeed::default(), Instant::now());
    old.take_in(&old_hub, &STEAMID.into(), frames(1), None, now);
    let other_map = on("cp_gullywash_f9", 9100);
    assert_eq!(VoiceFeed::default().adopt(&other_map, "old", handed_on(&old, &old_hub, false), now), Adopt::Not);
    let started_over = on("cp_process_f12", 500);
    assert_eq!(VoiceFeed::default().adopt(&started_over, "old", handed_on(&old, &old_hub, true), now), Adopt::Not);
}

#[test]
fn held_voice_waits_for_the_stream_to_be_taken() {
    let (old_hub, old, now) = (on("cp_process_f12", 1000), VoiceFeed::default(), Instant::now());
    old.take_in(&old_hub, &STEAMID.into(), frames(1), None, now);
    let (hub, new) = (SharedHub::default(), VoiceFeed::default());
    assert_eq!(new.adopt(&hub, "old", handed_on(&old, &old_hub, true), now), Adopt::NotYet);
}

#[test]
fn pause_talk_handed_on_is_kept_at_the_tick_the_game_stood_still_at() {
    let (old_hub, old, now) = (on("cp_process_f12", 1000), VoiceFeed::default(), Instant::now());
    old.step(&old_hub, now);
    publish(&old_hub, &Line::Pause(Pause { tick: 1000, paused: true }));
    old.step(&old_hub, now);
    assert_eq!(old.take_in(&old_hub, &STEAMID.into(), frames(3), None, now), Took::PauseTalk);
    let saved = handed_on(&old, &old_hub, true);
    // The game went on before the next process was there.
    let (hub, new) = (on("cp_process_f12", 1200), VoiceFeed::default());
    let (page, recorder) = (subscribe_to(&hub, true, true), subscribe_all(&hub));
    assert_eq!(new.adopt(&hub, "old", saved, now), Adopt::Taken);
    assert_eq!(voices(&page).len(), 0, "no page hears it");
    let kept = voices(&recorder);
    assert_eq!((kept.len(), kept[0]["pause"].as_bool(), kept[0]["tick"].as_u64()), (1, Some(true), Some(1000)), "{kept:?}");
}
