use super::assets::APP;
use super::*;
use crate::matches::hub::{SharedHub, publish};
use crate::testing::{TempDir, app, server};
use crate::web::client::{client_key, is_cloudflare, on_our_network};
use crate::{App, Matches, TcpListener};
use flate2::read::GzDecoder;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use stvtf::Line;
use stvtf::output::{self, PlayerId};

#[test]
fn a_start_counts_against_a_steam_id_else_an_address() {
    let user = |streamer| token::User { uid: "765".into(), name: "n".into(), streamer };
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    assert_eq!(counted(None, Some("1"), ip("192.0.2.1")).as_deref(), Some("steam:1"));
    assert_eq!(counted(Some(&user(false)), None, ip("192.0.2.1")).as_deref(), Some("steam:765"));
    assert_eq!(counted(Some(&user(true)), Some("1"), ip("192.0.2.1")), None, "streamers count against nobody");
    assert_eq!(counted(None, None, ip("192.0.2.1")).as_deref(), Some("ip:192.0.2.1"));
    assert_eq!(counted(None, Some("1"), ip("127.0.0.1")), None, "this machine");
}

#[test]
fn start_links_carry_every_display_toggle() {
    let toggles = "names=0&score=1&panel=0.5&rotate=auto&dot=14&ui=1.5&zoom=map&side=0&bg=geometry&bgopacity=0.4&health=0&projectiles=0&beams=0&buildings=0&shots=0&deaths=0\
                   &caps=0&heals=1&hits=1&marks=1&jumps=1&pickups=1&points=0&flow=1&loadout=0&effects=0";
    assert_eq!(start_view(&format!("view=overlay&{toggles}&x=y")), format!("overlay?{toggles}"));
    assert_eq!(start_view("at=1:02:03"), "?t=1:02:03", "a moment, as the page's t");
    assert_eq!(start_view("at=1:2:3:4"), "");
}

/// Runs `serve` on one connection, with `send` written by the client; returns serve's result and the response.
fn exchange(app: &App, send: &[u8]) -> (Result<()>, Vec<u8>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    client.write_all(send).unwrap();
    let result = serve(server, app);
    let mut response = Vec::new();
    let _ = client.read_to_end(&mut response);
    (result, response)
}

fn request_to(app: &App, send: &[u8]) -> (Result<()>, String) {
    let (result, response) = exchange(app, send);
    (result, String::from_utf8_lossy(&response).into_owned())
}

/// A response's head, and its body, which may be binary.
fn head_and_body(response: &[u8]) -> (String, &[u8]) {
    let end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    (String::from_utf8_lossy(&response[..end]).into_owned(), &response[end + 4..])
}

fn request(send: &[u8]) -> (Result<()>, String) {
    request_to(&app(Matches::One(SharedHub::default())), send)
}

#[test]
fn other_sites_load_the_viewer_from_a_fixed_address() {
    let loader = request(b"GET /embed/v1/stv-map.js HTTP/1.1\r\n\r\n").1;
    assert!(loader.contains(&format!("\"../../app/{}/stv-map.js\"", app_version())) && loader.contains("max-age=300"), "{loader}");
    let module = request(format!("GET /app/{}/stv-map.js HTTP/1.1\r\n\r\n", app_version()).as_bytes()).1;
    assert!(module.contains("customElements.define") && module.contains("Access-Control-Allow-Origin: *"), "{module}");
}

#[test]
fn other_sites_get_the_kill_icons_with_crits_under_a_version() {
    let css = request(b"GET /killicons/v1/killicons.css HTTP/1.1\r\n\r\n").1;
    assert!(css.starts_with("HTTP/1.1 200") && css.contains(".killicon-quake_rl") && css.contains(".killicon.crit"), "{css}");
}

/// A server with a pinned demo rooms can play, and the folder its recording is in.
fn demo_app() -> (App, TempDir) {
    let dir = TempDir::new("rooms");
    let path = dir.join("demo.jsonl");
    std::fs::write(&path, "{\"type\":\"stalemate\",\"tick\":5}\n{\"type\":\"stalemate\",\"tick\":55}\n").unwrap();
    let sessions = Sessions::new(false, 2, 20.0);
    sessions.pin("demo", move |s| s.set_file(path));
    while !sessions.get("demo").unwrap().recordable() {
        std::thread::sleep(Duration::from_millis(1));
    }
    (app(Matches::Many(sessions)), dir)
}

/// The demo server behind the site, with its key: (app, key's hex, the folder).
fn site_app() -> (App, String, TempDir) {
    let (mut app, dir) = demo_app();
    let key = "42".repeat(32);
    app.keys = Some(Arc::new(token::Keys::from_hex(&key).unwrap()));
    app.home = Some("https://serveme.tf/stv".into());
    (app, key, dir)
}

/// `stvtf_user` as the site's sign-in leaves it, for `json` (`{"uid", "name", "streamer"}`).
fn signed_in(app: &App, key: &str, json: &str) -> String {
    let t = token::seal(key, &format!(r#"{{"signin":{json},"exp":{}}}"#, unix_now() + 60), [5; 12]);
    let back = get(app, &format!("/signed_in?t={t}&return=/r/x/"), "");
    assert!(back.starts_with("HTTP/1.1 303") && back.contains("Location: /r/x/"), "{back}");
    let cookie = back.split("Set-Cookie: ").nth(1).unwrap().split(';').next().unwrap().to_string();
    assert!(back.contains("HttpOnly; SameSite=Lax; Secure"), "{back}");
    cookie
}

#[test]
fn a_demo_not_playing_is_played_again_by_the_site() {
    let (app, _key, _dir) = site_app();
    let out = get(&app, "/w/d1531482/", "");
    assert!(
        out.starts_with("HTTP/1.1 303") && out.contains("Location: https://serveme.tf/stv?connect=https%3A%2F%2Fdemos.tf%2F1531482"),
        "{out}"
    );
    let at = get(&app, "/w/d1531482/?t=12:34", "");
    assert!(at.contains("demos.tf%2F1531482&at=12:34\r\n"), "the moment goes along: {at}");
    assert!(!get(&app, "/w/d1531482/?t=x%0d", "").contains("&at="));
    assert!(get(&app, "/w/d1531482/events", "").starts_with("HTTP/1.1 404"), "only the page");
    assert!(get(&app, "/w/dx/", "").starts_with("HTTP/1.1 404"));
}

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
    let page = opened.split(r#""page":""#).nth(1).unwrap().split('"').next().unwrap().to_string();
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

fn post(app: &App, path: &str, cookie: &str, body: &str) -> String {
    let send = format!("POST {path} HTTP/1.1\r\nCookie: {cookie}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    request_to(app, send.as_bytes()).1
}

fn get(app: &App, path: &str, cookie: &str) -> String {
    request_to(app, format!("GET {path} HTTP/1.1\r\nCookie: {cookie}\r\n\r\n").as_bytes()).1
}

/// Opens a room on the demo: its page and the owner's cookie.
fn open_room(app: &App) -> (String, String) {
    let opened = post(app, "/w/demo/room", "", "");
    assert!(opened.starts_with("HTTP/1.1 201"), "{opened}");
    let cookie = opened.split("Set-Cookie: ").nth(1).unwrap().split(';').next().unwrap().to_string();
    assert!(opened.contains("HttpOnly; SameSite=Lax"), "{opened}");
    let page = opened.split(r#""page":""#).nth(1).unwrap().split('"').next().unwrap().to_string();
    (page, cookie)
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
fn a_match_page_has_a_timeline() {
    let (app, _dir) = demo_app();
    let deadline = Instant::now() + Duration::from_secs(5);
    let timeline = loop {
        let t = get(&app, "/w/demo/timeline", "");
        if t.starts_with("HTTP/1.1 200") {
            break t;
        }
        assert!(t.starts_with("HTTP/1.1 202") && Instant::now() < deadline, "{t}");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(timeline.contains(r#""length":750"#) && timeline.contains(r#""live":false"#), "{timeline}");
    assert!(get(&app, "/w/demo/marks?from=0", "").ends_with(r#"{"all":0,"marks":[]}"#));
    let Matches::Many(sessions) = &app.matches else { unreachable!() };
    let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
    assert!(get(&app, &format!("/w/{}/timeline", m.id), "").contains(r#""live":true"#));
    sessions.stop_all();
}

#[test]
fn a_viewer_goes_back_on_their_own() {
    let (app, _dir) = demo_app();
    assert!(post(&app, "/w/demo/rewind", "", r#"{"at":0}"#).starts_with("HTTP/1.1 409"), "the timeline is not ready yet");
    while !get(&app, "/w/demo/timeline", "").starts_with("HTTP/1.1 200") {
        std::thread::sleep(Duration::from_millis(5));
    }
    let back = post(&app, "/w/demo/rewind", "", r#"{"at":500,"speed":2,"paused":true}"#);
    assert!(back.starts_with("HTTP/1.1 201") && back.contains("Set-Cookie: stvtf_guest="), "{back}");
    let page = back.split(r#""page":""#).nth(1).unwrap().split('"').next().unwrap().to_string();
    let cookie = back.split("Set-Cookie: ").nth(1).unwrap().split(';').next().unwrap().to_string();
    assert!(get(&app, &format!("{page}me"), &cookie).contains(r#""admin":true"#), "theirs to control");
    assert_eq!(app.rooms.shared_count(), 0, "not counted as a room");
    assert!(post(&app, "/w/demo/rewind", "", r#"{"at":"soon"}"#).starts_with("HTTP/1.1 400"));
}

#[test]
fn a_live_match_opens_a_room_that_records_it_from_then_on() {
    let app = app(Matches::Many(Sessions::new(false, 2, 20.0)));
    let Matches::Many(sessions) = &app.matches else { unreachable!() };
    let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
    publish(&m.hub, &server("cp_process_f12"));
    assert!(get(&app, &format!("/w/{}/room", m.id), "").starts_with("HTTP/1.1 200"));
    let opened = post(&app, &format!("/w/{}/room", m.id), "", "");
    assert!(opened.starts_with("HTTP/1.1 201"), "{opened}");
    let page = opened.split(r#""page":""#).nth(1).unwrap().split('"').next().unwrap().to_string();
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

#[test]
fn an_ended_live_match_is_at_its_end() {
    let app = app(Matches::Many(Sessions::new(false, 2, 20.0)));
    let Matches::Many(sessions) = &app.matches else { unreachable!() };
    let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
    let rec = m.recording().unwrap();
    let stalemate = serde_json::from_str::<Line>(r#"{"type":"stalemate","tick":500}"#).unwrap();
    while rec.length() < 50 {
        publish(&m.hub, &stalemate);
        std::thread::sleep(Duration::from_millis(5));
    }
    // The match ended: no more lines come.
    rec.end();
    let timeline = get(&app, &format!("/w/{}/timeline", m.id), "");
    let at = format!(r#""at":{}"#, rec.length());
    assert!(timeline.contains(&at) && timeline.contains(r#""live":false"#), "{timeline}");
    sessions.stop_all();
}

fn post_watch(app: &App, connect: &str) -> String {
    let body = format!("connect={connect}");
    let send = format!("POST /watch HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    request_to(app, send.as_bytes()).1
}

#[test]
fn many_matches_start_from_a_connect_string() {
    let app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    assert!(request_to(&app, b"GET / HTTP/1.1\r\n\r\n").1.contains("<form"));
    let response = post_watch(&app, "connect+192.0.2.1%3A27020%3B+password+%22tv%22");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    let location = response.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap().to_string();
    assert!(location.starts_with("/w/") && location.ends_with('/'), "{location}");
    assert_eq!(post_watch(&app, "192.0.2.1:27020+tv").lines().find_map(|l| l.strip_prefix("Location: ")), Some(location.as_str()));
    let page = request_to(&app, format!("GET {location} HTTP/1.1\r\n\r\n").as_bytes()).1;
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("/main.js\""), "{page}");
    let bare = request_to(&app, format!("GET {} HTTP/1.1\r\n\r\n", location.trim_end_matches('/')).as_bytes()).1;
    assert!(bare.starts_with("HTTP/1.1 308"), "{bare}");
    assert!(request_to(&app, b"GET /w/nope/ HTTP/1.1\r\n\r\n").1.starts_with("HTTP/1.1 404"));
    // A delay after the last /, for the page and the overlay.
    for (rest, status) in [
        ("5", "200"),
        ("overlay/4.5", "200"),
        ("60", "200"),
        ("61", "404"),
        ("1.2.3", "404"),
        ("5/", "404"),
        ("overlay/", "404"),
        ("x", "404"),
    ] {
        let page = request_to(&app, format!("GET {location}{rest} HTTP/1.1\r\n\r\n").as_bytes()).1;
        assert!(page.starts_with(&format!("HTTP/1.1 {status}")), "{rest}: {page}");
    }
    if let Matches::Many(s) = &app.matches {
        s.stop_all();
    }
}

#[test]
fn a_form_after_long_headers_still_starts_a_match() {
    let app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    let body = "connect=192.0.2.1%3A27020+tv";
    // Headers ending just short of MAX_REQUEST, so the body takes it over.
    let head = |cookie: &str| format!("POST /watch HTTP/1.1\r\nCookie: {cookie}\r\nContent-Length: {}\r\n\r\n", body.len());
    let cookie = "c".repeat(MAX_REQUEST as usize - head("").len() - 10);
    let send = head(&cookie) + body;
    assert!(send.len() as u64 > MAX_REQUEST && (head(&cookie).len() as u64) < MAX_REQUEST);
    let response = request_to(&app, send.as_bytes()).1;
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    let huge = format!("POST /watch HTTP/1.1\r\nContent-Length: {}\r\n\r\n", 17 << 10);
    assert!(request_to(&app, huge.as_bytes()).1.starts_with("HTTP/1.1 413"));
    if let Matches::Many(s) = &app.matches {
        s.stop_all();
    }
}

#[test]
fn behind_a_site_matches_start_only_from_its_tokens() {
    let key = "11".repeat(32);
    let mut app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    app.base = "/live".into();
    app.keys = Some(Arc::new(token::Keys::from_hex(&key).unwrap()));
    assert!(request_to(&app, b"GET / HTTP/1.1\r\n\r\n").1.starts_with("HTTP/1.1 403"));
    assert!(post_watch(&app, "192.0.2.1%3A27020+tv").starts_with("HTTP/1.1 403"));
    app.home = Some("https://serveme.tf/stv".into());
    let home = request_to(&app, b"GET / HTTP/1.1\r\n\r\n").1;
    assert!(home.starts_with("HTTP/1.1 302") && home.contains("Location: https://serveme.tf/stv\r\n"), "{home}");
    assert!(post_watch(&app, "192.0.2.1%3A27020+tv").starts_with("HTTP/1.1 403"));
    let exp = unix_now() + 60;
    let t = token::seal(&key, &format!(r#"{{"connect":"connect 192.0.2.1:27020; password \"tv\"","owner":"u1","exp":{exp}}}"#), [3; 12]);
    let response = request_to(&app, format!("GET /start?t={t} HTTP/1.1\r\n\r\n").as_bytes()).1;
    let location = response.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap_or_default().to_string();
    assert!(location.starts_with("/live/w/") && location.ends_with('/'), "{response}");
    let page = request_to(&app, format!("GET {} HTTP/1.1\r\n\r\n", &location[5..]).as_bytes()).1;
    assert!(page.contains(r#"<base href="/live/">"#), "{page}");
    let overlay = request_to(&app, format!("GET /start?t={t}&view=overlay&delay=4.5&names=1&x=y&score=%0d HTTP/1.1\r\n\r\n").as_bytes()).1;
    assert!(overlay.contains(&format!("Location: {location}overlay/4.5?names=1\r\n")), "{overlay}");
    let late = request_to(&app, format!("GET /start?t={t}&delay=99 HTTP/1.1\r\n\r\n").as_bytes()).1;
    assert!(late.contains(&format!("Location: {location}\r\n")), "{late}");
    let json = request_to(&app, format!("GET /start?t={t}&format=json HTTP/1.1\r\n\r\n").as_bytes()).1;
    assert!(json.contains(&format!(r#"{{"page":"{location}"}}"#)) && json.contains("Access-Control-Allow-Origin: *"), "{json}");
    assert!(request_to(&app, b"GET /start?t=bogus HTTP/1.1\r\n\r\n").1.starts_with("HTTP/1.1 400"));
    if let Matches::Many(s) = &app.matches {
        s.stop_all();
    }
}

#[test]
fn the_site_lists_the_matches_with_a_list_token() {
    let key = "11".repeat(32);
    let mut app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    assert!(request_to(&app, b"GET /matches HTTP/1.1\r\n\r\n").1.starts_with("HTTP/1.1 404"), "only behind a site");
    app.keys = Some(Arc::new(token::Keys::from_hex(&key).unwrap()));
    let exp = unix_now() + 60;
    let start = token::seal(&key, &format!(r#"{{"connect":"192.0.2.1:27020 tv","reservation":5,"exp":{exp}}}"#), [4; 12]);
    assert!(request_to(&app, format!("GET /start?t={start} HTTP/1.1\r\n\r\n").as_bytes()).1.starts_with("HTTP/1.1 303"));
    let list = token::seal(&key, &format!(r#"{{"list":true,"exp":{exp}}}"#), [5; 12]);
    for refused in [String::new(), format!("Authorization: Bearer {start}\r\n"), "Authorization: Bearer bogus\r\n".into()] {
        let response = request_to(&app, format!("GET /matches HTTP/1.1\r\n{refused}\r\n").as_bytes()).1;
        assert!(response.starts_with("HTTP/1.1 401"), "{refused}: {response}");
    }
    let response = request_to(&app, format!("GET /matches HTTP/1.1\r\nAuthorization: Bearer {list}\r\n\r\n").as_bytes()).1;
    assert!(response.starts_with("HTTP/1.1 200") && response.contains("Cache-Control: no-store"), "{response}");
    let body: serde_json::Value = serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["matches"][0]["reservation"], 5);
    assert_eq!(body["matches"][0]["server"], "192.0.2.1:27020");
    assert!(request_to(&app, format!("GET /start?t={list} HTTP/1.1\r\n\r\n").as_bytes()).1.starts_with("HTTP/1.1 400"));
    if let Matches::Many(s) = &app.matches {
        s.stop_all();
    }
}

#[test]
fn a_public_server_refuses_internal_addresses() {
    let app = app(Matches::Many(Sessions::new(true, 4, 20.0)));
    let response = post_watch(&app, "192.168.1.12%3A27020+tv");
    assert!(response.starts_with("HTTP/1.1 400") && response.contains("not a public address"), "{response}");
}

#[test]
fn form_values_are_decoded() {
    assert_eq!(form_decode("connect+a%3A1%3B+password+%22p%26w%22"), "connect a:1; password \"p&w\"");
    assert_eq!(form_decode("bad%zz"), "badzz");
}

#[test]
fn serves_the_page_with_nosniff() {
    let (result, response) = request(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    result.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:.100}");
    assert!(response.contains("\r\nX-Content-Type-Options: nosniff\r\n"));
}

#[test]
fn endless_header_is_cut_off() {
    let mut send = b"GET / HTTP/1.1\r\nX-Long: ".to_vec();
    send.resize(64 << 10, b'a');
    let (result, response) = request(&send);
    assert!(result.is_err());
    assert_eq!(response, "");
}

/// The modules `name` imports (`from "…"` and `import("…")`), as paths under `app/`.
fn imports(name: &str, text: &str) -> Vec<String> {
    let dir: Vec<&str> = name.split('/').rev().skip(1).collect::<Vec<_>>().into_iter().rev().collect();
    let specs = text.split("from \"").chain(text.split("import(\"")).skip(1).filter_map(|r| r.split('"').next());
    specs
        .filter(|s| s.starts_with('.') && std::path::Path::new(s).extension().is_some_and(|e| e == "js"))
        .map(|spec| {
            let mut path = dir.clone();
            for part in spec.split('/') {
                match part {
                    "." => {}
                    ".." => drop(path.pop()),
                    p => path.push(p),
                }
            }
            path.join("/")
        })
        .collect()
}

#[test]
fn serves_the_page_and_its_modules_under_the_version() {
    let a = app(Matches::One(SharedHub::default()));
    let get = |path: &str| request_to(&a, format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).1;
    let page = get("/");
    let v = app_version();
    assert!(page.contains(&format!("\"app/{v}/style.css\"")) && page.contains(&format!("\"app/{v}/main.js\"")), "{page}");
    let mut imported = std::collections::HashSet::new();
    for (name, ctype, _) in APP {
        let file = get(&format!("/app/{v}/{name}"));
        assert!(file.starts_with("HTTP/1.1 200 OK") && file.contains("max-age=86400"), "{name}");
        assert!(file.contains(&format!("Content-Type: {ctype}")), "{name}");
        imported.extend(imports(name, &file));
    }
    for (name, ctype, _) in APP {
        // The page's entry, and the one /embed/v1/stv-map.js loads for other sites.
        let entry = name == "main.js" || name == "stv-map.js";
        assert!(ctype != JS || entry || imported.contains(name), "nothing imports {name}");
    }
    // And everything imported is served: a module missing from APP would 404 in the browser.
    for import in &imported {
        assert!(APP.iter().any(|(name, ..)| name == import), "{import} is imported but not in APP");
    }
    // The stylesheet's own links are relative to where it is served.
    let css = std::str::from_utf8(APP[0].2).unwrap();
    for link in css.split("url(\"").skip(1).map(|r| r.split('"').next().unwrap()).filter(|l| !l.starts_with("data:")) {
        let mut path: Vec<&str> = vec!["app", v];
        for part in link.split('/') {
            if part == ".." {
                path.pop();
            } else {
                path.push(part);
            }
        }
        let (result, font) = exchange(&a, format!("GET /{} HTTP/1.1\r\nHost: x\r\n\r\n", path.join("/")).as_bytes());
        result.unwrap();
        assert!(font.starts_with(b"HTTP/1.1 200 OK"), "{link}");
    }
    assert!(get(&format!("/app/{v}/missing.js")).starts_with("HTTP/1.1 404"));
    assert!(get("/app/main.js").starts_with("HTTP/1.1 404"));
}

#[test]
fn serves_backpack_pictures() {
    let dir = TempDir::new("backpack");
    std::fs::write(dir.join("scattergun.webp"), b"RIFF").unwrap();
    std::fs::write(dir.join("items.json"), b"{}").unwrap();
    let mut a = app(Matches::One(SharedHub::default()));
    a.backpack = dir.to_path_buf();
    let get = |path: &str| request_to(&a, format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).1;
    let picture = get("/backpack/scattergun.webp");
    assert!(picture.starts_with("HTTP/1.1 200 OK"), "{picture}");
    assert!(picture.contains("Content-Type: image/webp") && picture.contains("max-age=86400"));
    let items = get("/backpack/items.json");
    assert!(items.starts_with("HTTP/1.1 200 OK") && items.contains("Content-Type: application/json"), "{items}");
    for bad in
        ["/backpack/../x.webp", "/backpack/a/b.webp", "/backpack/missing.webp", "/backpack/scattergun.png", "/backpack/.webp", "/backpack/"]
    {
        assert!(get(bad).starts_with("HTTP/1.1 404"), "{bad}");
    }
}

#[test]
fn map_geometry_is_gzipped_for_browsers_that_take_it() {
    let dir = TempDir::new("mapgzip");
    let json = format!(r#"{{"floors":[{}]}}"#, r#"{"z":0,"p":[0,0,1,0,1,1],"k":"grass"},"#.repeat(200).trim_end_matches(','));
    std::fs::write(crate::maps::cache::geometry_path(&dir, "cp_test"), &json).unwrap();
    let mut a = app(Matches::One(SharedHub::default()));
    a.cache = dir.to_path_buf();
    let get = |accept: &str| {
        let (result, response) = exchange(&a, format!("GET /map/cp_test HTTP/1.1\r\nHost: x\r\n{accept}\r\n").as_bytes());
        result.unwrap();
        let (head, body) = head_and_body(&response);
        (head, body.to_vec())
    };
    let (head, body) = get("Accept-Encoding: br, gzip;q=0.9\r\n");
    assert!(
        head.starts_with("HTTP/1.1 200") && head.contains("\r\nContent-Encoding: gzip") && head.contains("\r\nVary: Accept-Encoding"),
        "{head}"
    );
    assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", body.len())) && body.len() < json.len() / 4, "{head}");
    let mut unzipped = String::new();
    GzDecoder::new(&body[..]).read_to_string(&mut unzipped).unwrap();
    assert_eq!(unzipped, json);
    // Served again from the gzipped copy in the cache, which the plain JSON stays next to.
    assert!(crate::maps::cache::geometry_path(&dir, "cp_test").with_extension("json.gz").exists());
    assert_eq!(get("Accept-Encoding: gzip\r\n").1, body);
    let (head, body) = get("");
    assert!(!head.contains("Content-Encoding") && head.contains("\r\nVary: Accept-Encoding"), "{head}");
    assert_eq!(body, json.as_bytes());
}

#[test]
fn clients_are_told_apart_by_address_or_ipv6_prefix() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    assert!(on_our_network(ip("172.18.0.2")) && on_our_network(ip("::1")) && on_our_network(ip("fd00::1")));
    assert!(!on_our_network(ip("141.94.96.119")) && !on_our_network(ip("2001:db8::1")) && !on_our_network(ip("::ffff:141.94.96.119")));
    assert_eq!(client_key(ip("2001:db8:1:2:aaaa::1")), client_key(ip("2001:db8:1:2:bbbb::2")));
    assert_ne!(client_key(ip("2001:db8:1:2::1")), client_key(ip("2001:db8:1:3::1")));
    assert_eq!(client_key(ip("::ffff:203.0.113.1")), ip("203.0.113.1"));
}

/// A client of `/events`, with the server's thread serving it.
struct EventStream {
    client: BufReader<TcpStream>,
    server: std::thread::JoinHandle<Result<()>>,
}

impl EventStream {
    fn open(app: &Arc<App>, send: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        client.write_all(send.as_bytes()).unwrap();
        let (conn, _) = listener.accept().unwrap();
        let app = app.clone();
        EventStream { client: BufReader::new(client), server: std::thread::spawn(move || serve(conn, &app)) }
    }

    /// What came in up to and including the first line with `needle`.
    fn read_to(&mut self, needle: &str) -> String {
        read_to(&mut self.client, needle)
    }

    /// Hangs up and waits for the server to notice, which it does when writing the next lines.
    fn close(self, hub: &SharedHub) {
        drop(self.client);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.server.is_finished() {
            assert!(Instant::now() < deadline, "the stream outlived its client");
            publish(hub, &server("cp_process_f12"));
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = self.server.join();
    }
}

fn read_to(reader: &mut impl BufRead, needle: &str) -> String {
    let mut seen = String::new();
    while !seen.lines().last().is_some_and(|l| l.contains(needle)) {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap_or_else(|e| panic!("waiting for {needle}: {e}, after {seen}"));
        assert!(n > 0, "the stream ended waiting for {needle}, after {seen}");
        seen.push_str(&line);
    }
    seen
}

fn one_match() -> (Arc<App>, SharedHub) {
    let hub = SharedHub::default();
    (Arc::new(app(Matches::One(hub.clone()))), hub)
}

fn heal() -> Line {
    Line::Heal(output::Heal { tick: 1, patient: PlayerId(4), healer: Some(PlayerId(3)), amount: 20, source: output::HealSource::Medigun })
}

#[test]
fn an_event_stream_sends_what_happened_and_then_what_happens() {
    let (app, hub) = one_match();
    publish(&hub, &server("cp_process_f12"));
    let mut stream = EventStream::open(&app, "GET /events HTTP/1.1\r\n\r\n");
    let head = stream.read_to("retry: 1000");
    assert!(head.starts_with("HTTP/1.1 200") && head.contains("Content-Type: text/event-stream"), "{head}");
    assert!(!head.contains("Content-Encoding"), "{head}");
    stream.read_to("cp_process_f12");
    publish(&hub, &server("cp_badlands"));
    let live = stream.read_to("cp_badlands");
    assert!(live.lines().last().unwrap().starts_with("data: {"), "{live}");
    stream.close(&hub);
}

#[test]
fn a_gzipped_event_stream_can_be_read_after_every_batch() {
    let (app, hub) = one_match();
    publish(&hub, &server("cp_process_f12"));
    let mut stream = EventStream::open(&app, "GET /events HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
    let head = stream.read_to("Access-Control");
    assert!(head.contains("Content-Encoding: gzip"), "{head}");
    stream.read_to("X-Content-Type-Options");
    let mut blank = String::new();
    stream.client.read_line(&mut blank).unwrap();
    let mut events = BufReader::new(GzDecoder::new(stream.client));
    assert!(read_to(&mut events, "cp_process_f12").starts_with("retry: 1000"));
    // Only got here if the first batch was flushed; so must the next, with the stream still open.
    publish(&hub, &server("cp_badlands"));
    read_to(&mut events, "cp_badlands");
}

#[test]
fn only_streams_that_ask_hear_detail_lines() {
    let (app, hub) = one_match();
    let mut plain = EventStream::open(&app, "GET /events HTTP/1.1\r\n\r\n");
    let mut detail = EventStream::open(&app, "GET /events?detail=1 HTTP/1.1\r\n\r\n");
    plain.read_to("retry:");
    detail.read_to("retry:");
    // Both are subscribed once this line reaches them.
    publish(&hub, &server("cp_process_f12"));
    plain.read_to("cp_process_f12");
    detail.read_to("cp_process_f12");
    publish(&hub, &heal());
    publish(&hub, &server("cp_badlands"));
    assert!(!plain.read_to("cp_badlands").contains("\"heal\""));
    assert!(detail.read_to("cp_badlands").contains("\"heal\""));
    plain.close(&hub);
    detail.close(&hub);
}

#[test]
fn closing_a_match_stream_frees_its_viewer_and_slot() {
    let sessions = Sessions::new(false, 1, 20.0);
    sessions.pin("demo", |_| {});
    let m = sessions.get("demo").unwrap();
    let app = Arc::new(app(Matches::Many(sessions)));
    let mut stream = EventStream::open(&app, "GET /w/demo/events HTTP/1.1\r\n\r\n");
    stream.read_to("retry:");
    assert!(m.watched());
    assert_eq!(app.streams.lock().unwrap().values().sum::<usize>(), 1);
    stream.close(&m.hub);
    assert!(!m.watched());
    assert!(app.streams.lock().unwrap().is_empty());
}

#[test]
fn too_many_streams_are_refused_and_not_counted_as_viewers() {
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    let (one, _hub) = one_match();
    let _open: Vec<_> = (0..MAX_STREAMS_PER_CLIENT).map(|_| one.stream_slot(loopback).unwrap()).collect();
    assert!(EventStream::open(&one, "GET /events HTTP/1.1\r\n\r\n").read_to("HTTP/1.1").starts_with("HTTP/1.1 429"));

    let sessions = Sessions::new(false, 1, 20.0);
    sessions.pin("demo", |_| {});
    let m = sessions.get("demo").unwrap();
    let many = Arc::new(app(Matches::Many(sessions)));
    let _open: Vec<_> = (0..MAX_STREAMS_PER_CLIENT).map(|_| many.stream_slot(loopback).unwrap()).collect();
    assert!(EventStream::open(&many, "GET /w/demo/events HTTP/1.1\r\n\r\n").read_to("HTTP/1.1").starts_with("HTTP/1.1 429"));
    assert!(!m.watched());
}

#[test]
fn only_a_proxy_of_ours_is_believed_about_the_client() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    let claimed = [ip("203.0.113.9")];
    assert_eq!(client_of(&claimed, ip("172.18.0.2")), ip("203.0.113.9"));
    assert_eq!(client_of(&claimed, ip("141.94.96.119")), ip("141.94.96.119"), "a stranger cannot choose whose streams count");
    assert_eq!(client_of(&[], ip("172.18.0.2")), ip("172.18.0.2"));
    // Visitor, Cloudflare, our proxy: the visitor, whatever they claimed before that.
    let via = [ip("10.0.0.1"), ip("81.2.69.160"), ip("162.158.1.2")];
    assert_eq!(client_of(&via, ip("172.18.0.2")), ip("81.2.69.160"));
    assert_eq!(client_of(&[ip("81.2.69.160"), ip("2a06:98c0::1")], ip("172.18.0.2")), ip("81.2.69.160"));
    // Straight to our proxy, claiming to be Cloudflare: themselves.
    assert_eq!(client_of(&[ip("81.2.69.160"), ip("162.158.1.2"), ip("89.160.20.112")], ip("172.18.0.2")), ip("89.160.20.112"));
    assert!(is_cloudflare(ip("104.27.255.255")) && !is_cloudflare(ip("104.28.0.0")));
    assert!(is_cloudflare(ip("::ffff:173.245.63.1")) && !is_cloudflare(ip("2a06:98c8::")));
}

#[test]
fn forwarded_addresses_are_read_in_order() {
    let forwarded = |header: &str| {
        let send = format!("GET / HTTP/1.1\r\n{header}\r\n\r\n");
        read_request(&mut send.as_bytes()).unwrap().forwarded.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
    };
    assert_eq!(forwarded("X-Forwarded-For: 203.0.113.9"), "203.0.113.9");
    assert_eq!(forwarded("x-forwarded-for: 198.51.100.7, 203.0.113.9"), "198.51.100.7 203.0.113.9");
    assert_eq!(forwarded("X-Forwarded-For: 2001:db8::1"), "2001:db8::1");
    assert_eq!(forwarded("X-Forwarded-For: 203.0.113.9, nonsense"), "");
    assert_eq!(forwarded("Host: x"), "");
}

#[test]
fn streams_behind_a_proxy_are_capped_per_forwarded_client() {
    let (app, hub) = one_match();
    let full: IpAddr = "203.0.113.9".parse().unwrap();
    let _open: Vec<_> = (0..MAX_STREAMS_PER_CLIENT).map(|_| app.stream_slot(full).unwrap()).collect();
    let mut refused = EventStream::open(&app, "GET /events HTTP/1.1\r\nX-Forwarded-For: 203.0.113.9\r\n\r\n");
    assert!(refused.read_to("HTTP/1.1").starts_with("HTTP/1.1 429"));
    let mut other = EventStream::open(&app, "GET /events HTTP/1.1\r\nX-Forwarded-For: 203.0.113.10\r\n\r\n");
    other.read_to("retry:");
    other.close(&hub);
}

/// A cache holding a BSP that cannot be built: it is gone once a build was tried, no network needed.
fn broken_map(name: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new("map");
    let bsp = dir.join(format!("{name}.bsp"));
    std::fs::write(&bsp, b"not a bsp").unwrap();
    (dir, bsp)
}

#[test]
fn with_many_matches_only_a_map_being_watched_is_built() {
    let (dir, bsp) = broken_map("cp_watched");
    let sessions = Sessions::new(false, 1, 20.0);
    sessions.pin("demo", |_| {});
    let m = sessions.get("demo").unwrap();
    let mut app = app(Matches::Many(sessions));
    app.cache = dir.to_path_buf();
    let refused = request_to(&app, b"GET /map/cp_watched HTTP/1.1\r\n\r\n").1;
    assert!(refused.starts_with("HTTP/1.1 404") && refused.contains("no match is on"), "{refused}");
    assert!(bsp.exists(), "not built");
    publish(&m.hub, &server("cp_elsewhere"));
    assert!(request_to(&app, b"GET /map/cp_watched HTTP/1.1\r\n\r\n").1.contains("no match is on"));
    publish(&m.hub, &server("cp_watched"));
    let tried = request_to(&app, b"GET /map/cp_watched HTTP/1.1\r\n\r\n").1;
    assert!(tried.starts_with("HTTP/1.1 404") && !tried.contains("no match is on"), "{tried}");
    assert!(!bsp.exists(), "built, and dropped when it turned out broken");
}

#[test]
fn a_backpack_name_cannot_leave_its_folder() {
    let root = TempDir::new("backpackroot");
    let backpack = root.join("backpack");
    std::fs::create_dir(&backpack).unwrap();
    std::fs::write(root.join("secret.webp"), b"secret").unwrap();
    let mut a = app(Matches::One(SharedHub::default()));
    a.backpack = backpack;
    let response = request_to(&a, b"GET /backpack/../secret.webp HTTP/1.1\r\n\r\n").1;
    assert!(response.starts_with("HTTP/1.1 404") && !response.contains("secret"), "{response}");
}

#[test]
fn a_match_no_longer_watched_says_where_to_start_it_again() {
    let mut a = app(Matches::Many(Sessions::new(false, 1, 20.0)));
    let gone = |a: &App| request_to(a, b"GET /w/nope/ HTTP/1.1\r\n\r\n").1;
    let page = gone(&a);
    assert!(page.starts_with("HTTP/1.1 404") && page.contains(HTML) && page.contains("paste its connect string"), "{page}");
    a.keys = Some(Arc::new(token::Keys::from_hex(&"00".repeat(32)).unwrap()));
    let text = gone(&a);
    assert!(text.starts_with("HTTP/1.1 404") && text.contains(TEXT) && text.contains("open it again from the site"), "{text}");
}

/// The head of a response, which may have a binary body.
fn head_of(app: &App, path: &str) -> String {
    let (result, response) = exchange(app, format!("GET {path} HTTP/1.1\r\n\r\n").as_bytes());
    result.unwrap();
    head_and_body(&response).0
}

#[test]
fn the_fixed_files_are_served_as_what_they_are() {
    let a = app(Matches::One(SharedHub::default()));
    for (path, ctype) in [
        ("/killicons.css", CSS),
        ("/killicons.webp", WEBP),
        ("/killicons/v1/killicons.webp", WEBP),
        ("/examples/embed.html", HTML),
        ("/examples/tf2pugs.html", HTML),
        ("/examples/tf2pugs-js.html", HTML),
        ("/icons/scout.svg", SVG),
    ] {
        let head = head_of(&a, path);
        assert!(head.starts_with("HTTP/1.1 200") && head.contains(&format!("Content-Type: {ctype}")), "{path}: {head}");
    }
    for path in ["/icons/nope.svg", "/icons/scout", "/nothing"] {
        assert!(head_of(&a, path).starts_with("HTTP/1.1 404"), "{path}");
    }
}

#[test]
fn cached_overviews_keep_and_maps_do_not() {
    let cache = TempDir::new("cachedheads");
    std::fs::create_dir(cache.join("demostf")).unwrap();
    std::fs::write(cache.join("demostf/boundaries.json"), b"{}").unwrap();
    std::fs::write(cache.join("demostf/cp_x.webp"), b"RIFF").unwrap();
    std::fs::write(crate::maps::cache::geometry_path(&cache, "cp_x"), b"{}").unwrap();
    let mut a = app(Matches::One(SharedHub::default()));
    a.cache = cache.to_path_buf();

    let list = head_of(&a, "/overview/boundaries.json");
    assert!(list.contains("Content-Type: application/json") && list.contains("max-age=86400"), "{list}");
    let image = head_of(&a, "/overview/cp_x.webp");
    assert!(image.contains("Content-Type: image/webp") && image.contains("max-age=86400"), "{image}");
    let map = head_of(&a, "/map/cp_x");
    assert!(map.contains("Content-Type: application/json") && map.contains("Vary: Accept-Encoding") && !map.contains("max-age"), "{map}");
}

#[test]
fn a_form_may_be_as_long_as_the_limit_and_no_longer() {
    let a = app(Matches::Many(Sessions::new(false, 1, 20.0)));
    let post = |len: u64| {
        let body = format!("connect={}", "+".repeat(len as usize - "connect=".len()));
        request_to(&a, format!("POST /watch HTTP/1.1\r\nContent-Length: {len}\r\n\r\n{body}").as_bytes()).1
    };
    let fits = post(MAX_REQUEST);
    assert!(fits.starts_with("HTTP/1.1 400") && fits.contains("no server address"), "{fits}");
    assert!(post(MAX_REQUEST + 1).starts_with("HTTP/1.1 413"));
}

#[test]
fn pages_leave_styling_to_the_stylesheet() {
    for page in [INDEX, LANDING] {
        assert!(!page.contains(" style=") && !page.contains("<style"), "styling in a page: put it in app/css/");
    }
}
