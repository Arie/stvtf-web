//! Voice coming in, its invite, and who is who.

use super::*;

fn post_voice(app: &App, id: &str, key: Option<&str>, body: &str) -> String {
    post_voice_as_is(app, id, key, &invited(app, id, body))
}

fn post_voice_as_is(app: &App, id: &str, key: Option<&str>, body: &str) -> String {
    let auth = key.map_or_default(|k| format!("Authorization: Bearer {k}\r\n"));
    let send = format!("POST /w/{id}/voice HTTP/1.1\r\n{auth}Content-Length: {}\r\n\r\n{body}", body.len());
    request_to(app, send.as_bytes()).1
}

const VOICE: &str = r#"{"steamid":"[U:1:5]","frames":["aA=="]}"#;

#[test]
fn a_voice_post_needs_the_key_and_a_live_match() {
    let (app, id) = voice_app();
    assert!(post_voice(&app, &id, None, VOICE).starts_with("HTTP/1.1 401"));
    assert!(post_voice(&app, &id, Some("wrong"), VOICE).starts_with("HTTP/1.1 401"));
    assert!(post_voice_as_is(&app, "nope", Some("k"), VOICE).starts_with("HTTP/1.1 404"));
    let no_stream = post_voice(&app, &id, Some("k"), VOICE);
    assert!(no_stream.starts_with("HTTP/1.1 409"), "nothing to hold it against yet: {no_stream}");
    let mut keyless = voice_app();
    keyless.0.voice.key = None;
    assert!(post_voice(&keyless.0, &keyless.1, Some("k"), VOICE).starts_with("HTTP/1.1 404"), "off without a key");
}

#[test]
fn voice_posted_to_a_live_match_is_held_to_its_tick() {
    let (app, id) = voice_app();
    let Matches::Many(sessions) = &app.matches else { panic!() };
    let m = sessions.get(&id).unwrap();
    publish(&m.hub, &crate::matches::hub::tests::snapshot(1000, None, Vec::new()));
    let held = post_voice(&app, &id, Some("k"), VOICE);
    assert!(held.starts_with("HTTP/1.1 200") && held.ends_with("held until tick 7000"), "{held}");
    let sooner = post_voice(&app, &id, Some("k"), r#"{"steamid":"[U:1:5]","frames":["aA=="],"delay":30}"#);
    assert!(sooner.ends_with("held until tick 3000"), "{sooner}");
    for bad in [
        r#"{"steamid":"[U:1:5]","frames":[]}"#,
        r#"{"steamid":"someone","frames":["aA=="]}"#,
        r#"{"steamid":"[U:1:5]","frames":["not base64!"]}"#,
        r#"{"steamid":"[U:1:5]","frames":["aA=="],"delay":900}"#,
    ] {
        assert!(post_voice(&app, &id, Some("k"), bad).starts_with("HTTP/1.1 400"), "{bad}");
    }
    let many = format!(r#"{{"steamid":"[U:1:5]","frames":[{}]}}"#, vec![r#""aA==""#; 33].join(","));
    assert!(post_voice(&app, &id, Some("k"), &many).starts_with("HTTP/1.1 413"));
}

#[test]
fn a_room_takes_voice_for_its_live_match_and_its_admins_match_who_is_who() {
    let (app, id) = voice_app();
    let Matches::Many(sessions) = &app.matches else { panic!() };
    let m = sessions.get(&id).unwrap();
    publish(&m.hub, &server("cp_process_f12"));
    let scout = output::Player { steamid: "[U:1:5]".into(), ..crate::matches::hub::tests::player(7, "scout", output::Team::Red) };
    publish(&m.hub, &crate::matches::hub::tests::snapshot(1000, None, vec![scout]));
    let opened = post(&app, &format!("/w/{id}/room"), "", "");
    let (cookie, room) = (cookie_of(&opened), page_of(&opened));
    // The bot posts to the room's link as to the match's.
    let said = invited(&app, &id, r#"{"discord":"77","name":"coach","channel":"9","frames":["aA=="]}"#);
    let send = format!("POST {room}voice HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: {}\r\n\r\n{said}", said.len());
    let held = request_to(&app, send.as_bytes()).1;
    assert!(held.starts_with("HTTP/1.1 200") && held.ends_with("held until tick 7000"), "{held}");
    let keyless = format!("POST {room}voice HTTP/1.1\r\nContent-Length: {}\r\n\r\n{said}", said.len());
    assert!(request_to(&app, keyless.as_bytes()).1.starts_with("HTTP/1.1 401"));
    // Its admins see who is not matched, and match them to a player.
    let voices = get(&app, &format!("{room}voices"), &cookie);
    assert!(voices.contains(r#"{"confirmed":false,"heard":true,"id":"77","linked":null,"name":"coach","steamid":null}"#), "{voices}");
    let matched = post(&app, &format!("{room}voice-match"), &cookie, r#"{"discord":"77","player":7}"#);
    assert!(matched.starts_with("HTTP/1.1 204"), "{matched}");
    assert!(get(&app, &format!("{room}voices"), &cookie).contains(r#""steamid":"[U:1:5]""#));
    assert!(
        post(&app, &format!("{room}voice-match"), &cookie, r#"{"discord":"77","player":99}"#).starts_with("HTTP/1.1 400"),
        "no such player"
    );
    assert!(
        post(&app, &format!("{room}voice-match"), &cookie, r#"{"discord":"78","player":7}"#).starts_with("HTTP/1.1 400"),
        "no such voice"
    );
    assert!(post(&app, &format!("{room}voice-match"), &cookie, r#"{"discord":"77","player":null}"#).starts_with("HTTP/1.1 204"));
    assert!(get(&app, &format!("{room}voices"), &cookie).contains(r#""steamid":null"#), "unmatched again");
    // The delay is the bot's to say (whoever invited it): a room moves no voice.
    let nudged = post(&app, &format!("{room}voice-nudge"), &cookie, r#"{"nudge":-30}"#);
    assert!(!nudged.starts_with("HTTP/1.1 2"), "{nudged}");
    assert!(post_voice(&app, &id, Some("k"), VOICE).ends_with("held until tick 7000"), "90 s, as said");
}

/// An app that links through a made-up Discord, whose user 77 has these connections.
fn linking_app(connections: &str) -> App {
    let (discord, _) = web_server(vec![
        ("/oauth2/token", br#"{"access_token":"t","token_type":"Bearer"}"#.to_vec()),
        ("/users/@me", br#"{"id":"77","username":"gummy"}"#.to_vec()),
        ("/users/@me/connections", connections.as_bytes().to_vec()),
    ]);
    let mut app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    app.voice.key = Some("k".into());
    app.voice.links =
        crate::voice::links::Links::with_discord(discord.trim_end_matches('/'), "id1", "secret", "https://stv.example/voice/linked");
    app
}

fn who(app: &App, discord: &str, key: &str) -> String {
    request_to(app, format!("GET /voice/who?discord={discord} HTTP/1.1\r\nAuthorization: Bearer {key}\r\n\r\n").as_bytes()).1
}

#[test]
fn a_discord_user_links_the_steam_account_discord_verified_and_the_bot_asks_who_they_are() {
    // 76561197960265728 + 5.
    let app = linking_app(r#"[{"type":"twitch","id":"1","verified":true},{"type":"steam","id":"76561197960265733","verified":true}]"#);
    assert!(who(&app, "77", "k").ends_with(r#"{"steamid":null}"#), "not linked yet");
    let sent = get(&app, "/voice/link", "");
    assert!(sent.starts_with("HTTP/1.1 302"), "{sent}");
    let location = sent.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap();
    assert!(location.contains("/oauth2/authorize?response_type=code&client_id=id1&scope=identify+connections"), "{location}");
    assert!(location.contains("&redirect_uri=https%3A//stv.example/voice/linked"), "{location}");
    let cookie = cookie_of(&sent);
    let state = cookie.strip_prefix("voice_link=").unwrap();
    assert!(location.ends_with(&format!("&state={state}")), "{location}");
    // Back from Discord without the browser's own state: refused.
    assert!(get(&app, "/voice/linked?code=c&state=other", &cookie).starts_with("HTTP/1.1 400"));
    assert!(get(&app, &format!("/voice/linked?code=c&state={state}"), "").starts_with("HTTP/1.1 400"));
    let linked = get(&app, &format!("/voice/linked?code=c&state={state}"), &cookie);
    assert!(linked.starts_with("HTTP/1.1 200") && linked.contains("[U:1:5]"), "{linked}");
    assert!(who(&app, "77", "k").ends_with(r#"{"steamid":"[U:1:5]"}"#));
    assert!(who(&app, "77", "wrong").starts_with("HTTP/1.1 401"));
    assert!(who(&app, "x", "k").starts_with("HTTP/1.1 400"));
}

#[test]
fn a_discord_user_without_a_verified_steam_connection_is_told_to_add_one() {
    let app = linking_app(r#"[{"type":"steam","id":"76561197960265733","verified":false}]"#);
    let sent = get(&app, "/voice/link", "");
    let cookie = cookie_of(&sent);
    let state = cookie.strip_prefix("voice_link=").unwrap();
    let linked = get(&app, &format!("/voice/linked?code=c&state={state}"), &cookie);
    assert!(linked.starts_with("HTTP/1.1 409") && linked.contains("Connections"), "{linked}");
    assert!(who(&app, "77", "k").ends_with(r#"{"steamid":null}"#));
}

#[test]
fn without_a_discord_application_there_is_no_linking() {
    let (app, _) = voice_app();
    assert!(get(&app, "/voice/link", "").starts_with("HTTP/1.1 404"));
    assert!(who(&app, "77", "k").ends_with(r#"{"steamid":null}"#), "the bot may still ask");
}

#[test]
fn where_people_sign_in_only_streamers_among_a_rooms_admins_tune_its_matchs_voice() {
    let (app, key, _dir) = site_app();
    let owner = signed_in(&app, &key, r#"{"uid":"7656","name":"Arie","streamer":false}"#);
    let page = page_of(&post(&app, "/w/demo/room", &owner, ""));
    assert!(get(&app, &format!("{page}me"), &owner).contains(r#""admin":true"#));
    // Anyone opens a room and is its admin: a match's voice is not theirs to move.
    assert!(get(&app, &format!("{page}voices"), &owner).starts_with("HTTP/1.1 204"));
    assert!(post(&app, &format!("{page}voice-match"), &owner, r#"{"discord":"77","player":7}"#).starts_with("HTTP/1.1 409"), "a demo");
    let streamer = signed_in(&app, &key, r#"{"uid":"9999","name":"caster","streamer":true}"#);
    assert!(post(&app, &format!("{page}claim"), &streamer, "").starts_with("HTTP/1.1 204"));
    assert!(get(&app, &format!("{page}voices"), &streamer).starts_with("HTTP/1.1 204"), "a demo has no voice coming in");
}

#[test]
fn voice_comes_in_only_with_the_matchs_invite() {
    let (app, id) = voice_app();
    let Matches::Many(sessions) = &app.matches else { panic!() };
    let m = sessions.get(&id).unwrap();
    publish(&m.hub, &server("cp_process_f12"));
    publish(&m.hub, &crate::matches::hub::tests::snapshot(1000, None, Vec::new()));
    // The bot's key alone lets no channel in: anyone can have the bot in their Discord.
    assert!(post_voice_as_is(&app, &id, Some("k"), VOICE).starts_with("HTTP/1.1 403"));
    let wrong = r#"{"invite":"0000000000000000","steamid":"[U:1:5]","frames":["aA=="]}"#;
    assert!(post_voice_as_is(&app, &id, Some("k"), wrong).starts_with("HTTP/1.1 403"));
    assert!(post_voice(&app, &id, Some("k"), VOICE).starts_with("HTTP/1.1 200"));
    // Where nobody signs in, whoever has the page has the invite.
    let got = get(&app, &format!("/w/{id}/voice-invite"), "");
    assert!(got.starts_with("HTTP/1.1 200") && got.contains(&format!(r#""invite":"{}""#, invite_of(&app, &id))), "{got}");
    assert_eq!(invite_of(&app, &id).len(), 16);
}

#[test]
fn behind_a_site_the_invite_is_for_who_started_the_match_and_for_streamers() {
    let (mut app, key, _dir) = site_app();
    app.voice.key = Some("k".into());
    let exp = unix_now() + 60;
    let start = format!(r#"{{"connect":"connect 192.0.2.9:27020; password \"tv\"","owner":"7656","exp":{exp}}}"#);
    let opened = get(&app, &format!("/start?t={}", token::seal(&key, &start, [3; 12])), "");
    let page = opened.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap().to_string();
    let id = page.trim_matches('/').trim_start_matches("w/").to_string();
    let starter = signed_in(&app, &key, r#"{"uid":"7656","name":"Arie","streamer":false}"#);
    let other = signed_in(&app, &key, r#"{"uid":"1111","name":"someone","streamer":false}"#);
    let caster = signed_in(&app, &key, r#"{"uid":"9999","name":"caster","streamer":true}"#);
    let invite = format!(r#""invite":"{}""#, invite_of(&app, &id));
    assert!(get(&app, &format!("{page}voice-invite"), &starter).contains(&invite));
    assert!(get(&app, &format!("{page}voice-invite"), &caster).contains(&invite));
    assert!(get(&app, &format!("{page}voice-invite"), &other).starts_with("HTTP/1.1 204"));
    assert!(get(&app, &format!("{page}voice-invite"), "").starts_with("HTTP/1.1 204"), "none, and no error on anyone's page");
    // In a room on it too; and there the starter, as the casters, says whose voice is whose.
    let room = page_of(&post(&app, &format!("{page}room"), &other, ""));
    assert!(get(&app, &format!("{room}voice-invite"), &starter).contains(&invite));
    assert!(get(&app, &format!("{room}voice-invite"), &other).starts_with("HTTP/1.1 204"), "its admin, but nobody to the match");
    assert!(get(&app, &format!("{room}voices"), &other).starts_with("HTTP/1.1 204"));
    assert!(get(&app, &format!("{room}voices"), &starter).starts_with("HTTP/1.1 200"), "no admin of the room, but the match is theirs");
}

#[test]
fn who_is_who_is_told_to_and_asked_of_the_stv_tf_that_holds_it_with_the_bots_key() {
    let (app, _id) = voice_app();
    let known = |key: &str, method: &str, query: &str, body: &str| {
        let send =
            format!("{method} /voice/known{query} HTTP/1.1\r\nAuthorization: Bearer {key}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        request_to(&app, send.as_bytes()).1
    };
    let said = r#"{"id":"mumble:cert:abc","steamid":"76561197960265737","name":"arie"}"#;
    assert!(known("wrong", "POST", "", said).starts_with("HTTP/1.1 401") && known("wrong", "GET", "", "").starts_with("HTTP/1.1 401"));
    assert!(known("k", "POST", "", said).starts_with("HTTP/1.1 204"));
    let all = known("k", "GET", "", "");
    assert!(
        all.contains(r#""version":1"#)
            && all.contains(r#""by":"","id":"mumble:cert:abc","name":"arie","src":"","steamid":"[U:1:9]","v":1"#),
        "{all}"
    );
    // Asked what changed since: nothing, until something does.
    assert!(known("k", "GET", "?since=1", "").contains(r#""known":[]"#));
    assert!(known("k", "POST", "", r#"{"id":"mumble:cert:abc","steamid":null,"name":"arie"}"#).starts_with("HTTP/1.1 204"));
    assert!(known("k", "GET", "?since=1", "").contains(r#""steamid":null,"v":2"#));
    for bad in [r#"{"id":"a b","steamid":"[U:1:9]"}"#, r#"{"id":"77","steamid":"gaben"}"#, "{}"] {
        assert!(known("k", "POST", "", bad).starts_with("HTTP/1.1 400"), "{bad}");
    }
}

#[test]
fn someone_in_mumble_is_known_by_who_they_are_on_their_server() {
    let (app, id) = voice_app();
    let Matches::Many(sessions) = &app.matches else { panic!() };
    let m = sessions.get(&id).unwrap();
    publish(&m.hub, &server("cp_process_f12"));
    publish(&m.hub, &crate::matches::hub::tests::snapshot(1000, None, Vec::new()));
    let said = r#"{"discord":"mumble:mumble.example:user:97","name":"Arie","channel":"mumble:mumble.example:12","frames":["aA=="]}"#;
    assert!(post_voice(&app, &id, Some("k"), said).starts_with("HTTP/1.1 200"));
    assert_eq!(m.comms.feed.voices(&m.hub)[0].voice_id, "mumble:mumble.example:user:97");
    let odd = r#"{"discord":"mumble:<script>","frames":["aA=="]}"#;
    assert!(post_voice(&app, &id, Some("k"), odd).starts_with("HTTP/1.1 400"));
}

#[test]
fn who_is_who_is_said_before_anyone_speaks_by_player_or_by_steam_id() {
    let (app, id) = voice_app();
    let Matches::Many(sessions) = &app.matches else { panic!() };
    let m = sessions.get(&id).unwrap();
    publish(&m.hub, &server("cp_process_f12"));
    let scout = output::Player { steamid: "[U:1:5]".into(), ..crate::matches::hub::tests::player(7, "scout", output::Team::Red) };
    publish(&m.hub, &crate::matches::hub::tests::snapshot(1000, None, vec![scout]));
    let page = format!("/w/{id}/");
    // The bot is in a Discord channel, and says who is there: nobody has said a word.
    let there = r#"{"kind":"discord","status":"In General with 2 people","people":[{"id":"77","name":"coach"},{"id":"78","name":"sub"}]}"#;
    assert!(status(&app, &id, "k", there).starts_with("HTTP/1.1 204"));
    assert!(
        get(&app, &format!("{page}voice-comms"), "")
            .ends_with(r#"{"discord":{"state":"in","status":"In General with 2 people"},"mumble":[]}"#)
    );
    let voices = get(&app, &format!("{page}voices"), "");
    assert!(voices.contains(r#"{"confirmed":false,"heard":false,"id":"77","linked":null,"name":"coach","steamid":null}"#), "{voices}");
    // One on a player in the match, one on a Steam id as people write them.
    assert!(post(&app, &format!("{page}voice-match"), "", r#"{"discord":"77","player":7}"#).starts_with("HTTP/1.1 204"));
    assert!(post(&app, &format!("{page}voice-match"), "", r#"{"discord":"78","steamid":"STEAM_0:1:4"}"#).starts_with("HTTP/1.1 204"));
    let voices = get(&app, &format!("{page}voices"), "");
    assert!(voices.contains(r#""name":"coach","steamid":"[U:1:5]""#) && voices.contains(r#""name":"sub","steamid":"[U:1:9]""#), "{voices}");
    assert!(post(&app, &format!("{page}voice-match"), "", r#"{"discord":"78","steamid":"gaben"}"#).starts_with("HTTP/1.1 400"));
    assert!(post(&app, &format!("{page}voice-match"), "", r#"{"discord":"99","player":7}"#).starts_with("HTTP/1.1 400"), "nobody there");
    // What they then say is that player's.
    let said = r#"{"discord":"77","name":"coach","frames":["aA=="],"delay":0}"#;
    let page_stream = crate::matches::hub::subscribe_to(&m.hub, false, true);
    assert!(post_voice(&app, &id, Some("k"), said).ends_with("live"));
    assert!(page_stream.try_iter().any(|l| l.contains(r#""player":7"#) && l.contains(r#""type":"voice""#)));
    // The bot gone: nobody is in a channel, and those who never spoke are no longer listed.
    assert!(status(&app, &id, "k", r#"{"kind":"discord","left":true}"#).starts_with("HTTP/1.1 204"));
    assert!(get(&app, &format!("{page}voice-comms"), "").contains(r#""discord":null"#));
    let voices = get(&app, &format!("{page}voices"), "");
    assert!(voices.contains("coach") && !voices.contains("sub"), "{voices}");
    // Not without the invite.
    let uninvited =
        format!("POST /w/{id}/voice-status HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: {}\r\n\r\n{there}", there.len());
    assert!(request_to(&app, uninvited.as_bytes()).1.starts_with("HTTP/1.1 403"));
}
