//! Opening a room, who controls it, and signing in.

use super::*;

#[test]
fn signing_in_goes_by_the_site_and_back_only_to_our_own_pages() {
    let (app, key, _dir) = site_app();
    let out = get(&app, "/signin?return=/r/abc/", "");
    assert!(out.contains("Location: https://serveme.tf/stv/sign_in?return=/r/abc/"), "{out}");
    for elsewhere in ["https://evil.example/", "//evil.example/", "/\\evil.example"] {
        let out = get(&app, &format!("/signin?return={elsewhere}"), "");
        assert!(out.contains("sign_in?return=/\r\n"), "{elsewhere}: {out}");
    }
    signed_in(&app, &key, r#"{"uid":"7656","name":"Arie","streamer":false}"#);
    let stale = token::seal(&key, r#"{"signin":{"uid":"1","name":"x"},"exp":5}"#, [6; 12]);
    assert!(get(&app, &format!("/signed_in?t={stale}"), "").starts_with("HTTP/1.1 400"));
    let start = token::seal(&key, &format!(r#"{{"demo":1,"exp":{}}}"#, unix_now() + 60), [7; 12]);
    assert!(get(&app, &format!("/signed_in?t={start}"), "").starts_with("HTTP/1.1 400"), "a start token signs nobody in");
    assert!(get(&app, "/signout?return=/r/x/", "").contains("Set-Cookie: stvtf_user=; Path=/; Max-Age=0"));
}

#[test]
fn someone_signed_in_is_their_steam_id_in_a_room_and_a_streamer_makes_themselves_admin() {
    let (app, key, _dir) = site_app();
    let owner = signed_in(&app, &key, r#"{"uid":"7656","name":"Arie","streamer":false}"#);
    let opened = post(&app, "/w/demo/room", &owner, "");
    let page = page_of(&opened);
    let me = get(&app, &format!("{page}me"), &owner);
    assert!(me.ends_with(r#"{"admin":true,"id":1,"signed":true,"signin":true,"streamer":false}"#), "{me}");
    // The same user from another browser (no guest cookie) is still the owner.
    let elsewhere = format!("{owner}; stvtf_guest={}", "3".repeat(32));
    assert!(get(&app, &format!("{page}me"), &elsewhere).contains(r#""admin":true"#));
    let streamer = signed_in(&app, &key, r#"{"uid":"9999","name":"caster","streamer":true}"#);
    let viewer = format!("stvtf_guest={}", "1".repeat(32));
    assert!(post(&app, &format!("{page}claim"), &viewer, "").starts_with("HTTP/1.1 403"));
    assert!(post(&app, &format!("{page}claim"), &streamer, "").starts_with("HTTP/1.1 204"));
    assert!(get(&app, &format!("{page}me"), &streamer).contains(r#""admin":true"#));
    // A cookie sealed with another key is nobody.
    let forged = format!(
        "stvtf_user={}",
        token::Keys::from_hex(&"ee".repeat(32))
            .unwrap()
            .keep(&token::User { uid: "9999".into(), name: "caster".into(), streamer: true }, unix_now(),)
    );
    assert!(post(&app, &format!("{page}claim"), &forged, "").starts_with("HTTP/1.1 403"));
}

#[test]
fn opening_a_room_again_is_back_to_ones_own() {
    let (app, _dir) = demo_app();
    let (page, cookie) = open_room(&app);
    let again = post(&app, "/w/demo/room", &cookie, "");
    assert!(again.contains(&format!(r#""page":"{page}""#)), "{again}");
    let (other, _) = open_room(&app);
    assert_ne!(other, page, "someone else's is their own");
}

#[test]
fn a_demo_opens_a_room_for_its_owner() {
    let (app, _dir) = demo_app();
    assert!(get(&app, "/w/demo/room", "").starts_with("HTTP/1.1 200"));
    let (page, cookie) = open_room(&app);
    assert!(page.starts_with("/r/") && page.ends_with('/'), "{page}");
    assert!(get(&app, "/w/demo/", "").contains("Cache-Control: no-cache"), "a page is asked for again");
    assert!(get(&app, &page, "").contains("<canvas"));
    assert!(get(&app, &page, "").contains("Cache-Control: no-cache"));
    assert!(get(&app, &format!("{page}marks?from=0"), "").ends_with(r#"{"all":0,"marks":[]}"#), "what its timeline shows");
    assert!(get(&app, &format!("{page}me"), &cookie).ends_with(r#"{"admin":true,"id":1,"signed":false,"signin":false,"streamer":false}"#));
    assert!(get(&app, &format!("{page}me"), "").contains(r#""admin":false"#));
    assert!(get(&app, &page, "").contains("Set-Cookie: stvtf_guest="), "everyone in a room gets a cookie");
    let again = post(&app, "/w/demo/room", &cookie, "");
    assert!(!again.contains("Set-Cookie"), "a guest keeps their cookie: {again}");
}

#[test]
fn only_admins_control_a_room() {
    let (app, _dir) = demo_app();
    let (page, cookie) = open_room(&app);
    let control = format!("{page}control");
    let pause = r#"{"paused":true}"#;
    assert!(post(&app, &control, "", pause).starts_with("HTTP/1.1 403"));
    assert!(post(&app, &control, &format!("stvtf_guest={}", "0".repeat(32)), pause).starts_with("HTTP/1.1 403"));
    assert!(post(&app, &control, &cookie, pause).starts_with("HTTP/1.1 204"));
    assert!(post(&app, &control, &cookie, r#"{"speed":3}"#).starts_with("HTTP/1.1 400"));
    assert!(post(&app, &control, &cookie, r#"{"rewind":1}"#).starts_with("HTTP/1.1 400"));
    let draw = format!("{page}draw");
    let stroke = r#"{"stroke":1,"points":[[0,0,0],[10,10,16]]}"#;
    assert!(post(&app, &draw, "", stroke).starts_with("HTTP/1.1 403"));
    assert!(post(&app, &draw, &cookie, stroke).starts_with("HTTP/1.1 204"));
    assert!(get(&app, "/r/0123/", "").starts_with("HTTP/1.1 404"));
}

#[test]
fn only_admins_take_a_room_to_another_map() {
    let (app, _dir) = demo_app();
    let (page, cookie) = open_room(&app);
    let map = format!("{page}map");
    assert!(post(&app, &map, "", r#"{"map":0}"#).starts_with("HTTP/1.1 403"));
    assert!(post(&app, &map, &cookie, r#"{"map":0}"#).starts_with("HTTP/1.1 204"));
    let gone = post(&app, &map, &cookie, r#"{"map":7}"#);
    assert!(gone.starts_with("HTTP/1.1 400") && gone.ends_with("Map gone"), "{gone}");
}

#[test]
fn only_admins_draw_arcs() {
    let (app, _dir) = demo_app();
    let (page, cookie) = open_room(&app);
    let arc = format!("{page}arc");
    let spots = r#"{"from":[0,0],"to":[500,0]}"#;
    assert!(post(&app, &arc, "", spots).starts_with("HTTP/1.1 403"));
    // An admin's is taken up (here refused for a map not built, not for who sent it).
    assert!(post(&app, &arc, &cookie, spots).starts_with("HTTP/1.1 400"));
}

#[test]
fn admins_make_others_admin() {
    let (app, _dir) = demo_app();
    let (page, owner) = open_room(&app);
    let viewer = format!("stvtf_guest={}", "1".repeat(32));
    let me = get(&app, &format!("{page}me"), &viewer);
    assert!(me.ends_with(r#"{"admin":false,"id":2,"signed":false,"signin":false,"streamer":false}"#), "{me}");
    assert!(post(&app, &format!("{page}name"), &viewer, r#"{"name":"caster"}"#).starts_with("HTTP/1.1 204"));
    let pause = r#"{"paused":true}"#;
    assert!(post(&app, &format!("{page}control"), &viewer, pause).starts_with("HTTP/1.1 403"));
    let promote = r#"{"person":2,"admin":true}"#;
    assert!(post(&app, &format!("{page}admin"), &viewer, promote).starts_with("HTTP/1.1 403"));
    assert!(post(&app, &format!("{page}admin"), &owner, promote).starts_with("HTTP/1.1 204"));
    assert!(post(&app, &format!("{page}control"), &viewer, pause).starts_with("HTTP/1.1 204"));
    assert!(post(&app, &format!("{page}view"), &viewer, r#"{"mode":"auto","take":true}"#).starts_with("HTTP/1.1 204"));
    assert!(post(&app, &format!("{page}view"), &viewer, r#"{"mode":"spin"}"#).starts_with("HTTP/1.1 400"));
    assert!(post(&app, &format!("{page}ink"), &viewer, r##"{"ink":"#123abc"}"##).starts_with("HTTP/1.1 204"));
    let demote_owner = r#"{"person":1,"admin":false}"#;
    assert!(post(&app, &format!("{page}admin"), &viewer, demote_owner).starts_with("HTTP/1.1 400"));
}

#[test]
fn a_live_match_opens_a_room_that_records_it_from_then_on() {
    let app = app(Matches::Many(Sessions::new(false, 2, 20.0)));
    let sessions = sessions_of(&app);
    let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
    publish(&m.hub, &server("cp_process_f12"));
    assert!(get(&app, &format!("/w/{}/room", m.id), "").starts_with("HTTP/1.1 200"));
    let opened = post(&app, &format!("/w/{}/room", m.id), "", "");
    assert!(opened.starts_with("HTTP/1.1 201"), "{opened}");
    let page = page_of(&opened);
    let id = page.trim_start_matches("/r/").trim_end_matches('/');
    let room = app.rooms.get(id, sessions).unwrap();
    let _watching = room.join("me", None, None, true);
    assert!(m.watched(), "the room watches the match");
    // What happens in the match from now on plays in the room.
    let kill = serde_json::from_str::<Line>(r#"{"type":"stalemate","tick":500}"#).unwrap();
    publish(&m.hub, &kill);
    let deadline = Instant::now() + Duration::from_secs(5);
    while room.hub.lock().unwrap().history.is_empty() {
        assert!(Instant::now() < deadline, "the room plays the match");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(room.hub.lock().unwrap().map.as_deref(), Some("cp_process_f12"));
    room.stop();
    sessions.stop_all();
}
