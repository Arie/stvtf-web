//! Starting a match, its page and timeline, and its demos.

use super::*;

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
    let sessions = sessions_of(&app);
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
    let page = page_of(&back);
    let cookie = cookie_of(&back);
    assert!(get(&app, &format!("{page}me"), &cookie).contains(r#""admin":true"#), "theirs to control");
    assert_eq!(app.rooms.shared_count(), 0, "not counted as a room");
    assert!(post(&app, "/w/demo/rewind", "", r#"{"at":"soon"}"#).starts_with("HTTP/1.1 400"));
}

#[test]
fn an_ended_live_match_is_at_its_end() {
    let app = app(Matches::Many(Sessions::new(false, 2, 20.0)));
    let sessions = sessions_of(&app);
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

#[test]
fn while_the_server_changes_map_the_timeline_is_of_the_map_just_played() {
    let app = app(Matches::Many(Sessions::new(false, 2, 20.0)));
    let sessions = sessions_of(&app);
    let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
    let rec = m.recording().unwrap();
    publish(&m.hub, &server("cp_a"));
    publish(&m.hub, &crate::matches::hub::tests::snapshot(50, None, Vec::new()));
    let stalemate = serde_json::from_str::<Line>(r#"{"type":"stalemate","tick":500}"#).unwrap();
    while rec.length() < 50 || !rec.latest().ready {
        publish(&m.hub, &stalemate);
        std::thread::sleep(Duration::from_millis(5));
    }
    for state in ["changelevel", "map_change"] {
        publish(&m.hub, &serde_json::from_str(&format!(r#"{{"type":"status","state":"{state}"}}"#)).unwrap());
    }
    while rec.parts().len() < 2 {
        std::thread::sleep(Duration::from_millis(5));
    }
    let end = rec.parts()[0].end.unwrap();
    let timeline = get(&app, &format!("/w/{}/timeline", m.id), "");
    for want in [format!(r#""at":{end}"#), format!(r#""length":{end}"#), r#""origin":0"#.into(), r#""live":false"#.into()] {
        assert!(timeline.contains(&want), "{want} in {timeline}");
    }
    sessions.stop_all();
}

#[test]
fn many_matches_start_from_a_connect_string() {
    let app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    assert!(get(&app, "/", "").contains("<form"));
    let response = post_watch(&app, "connect+192.0.2.1%3A27020%3B+password+%22tv%22");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    let location = response.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap().to_string();
    assert!(location.starts_with("/w/") && location.ends_with('/'), "{location}");
    assert_eq!(post_watch(&app, "192.0.2.1:27020+tv").lines().find_map(|l| l.strip_prefix("Location: ")), Some(location.as_str()));
    let page = get(&app, &location, "");
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("/main.js\""), "{page}");
    let bare = get(&app, location.trim_end_matches('/'), "");
    assert!(bare.starts_with("HTTP/1.1 308"), "{bare}");
    assert!(get(&app, "/w/nope/", "").starts_with("HTTP/1.1 404"));
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
        let page = get(&app, &format!("{location}{rest}"), "");
        assert!(page.starts_with(&format!("HTTP/1.1 {status}")), "{rest}: {page}");
    }
    sessions_of(&app).stop_all();
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
    sessions_of(&app).stop_all();
}

#[test]
fn behind_a_site_matches_start_only_from_its_tokens() {
    let key = "11".repeat(32);
    let mut app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    app.base = "/live".into();
    app.keys = Some(Arc::new(token::Keys::from_hex(&key).unwrap()));
    assert!(get(&app, "/", "").starts_with("HTTP/1.1 403"));
    assert!(post_watch(&app, "192.0.2.1%3A27020+tv").starts_with("HTTP/1.1 403"));
    app.home = Some("https://serveme.tf/stv".into());
    let home = get(&app, "/", "");
    assert!(home.starts_with("HTTP/1.1 302") && home.contains("Location: https://serveme.tf/stv\r\n"), "{home}");
    assert!(post_watch(&app, "192.0.2.1%3A27020+tv").starts_with("HTTP/1.1 403"));
    let exp = unix_now() + 60;
    let t = token::seal(&key, &format!(r#"{{"connect":"connect 192.0.2.1:27020; password \"tv\"","owner":"u1","exp":{exp}}}"#), [3; 12]);
    let response = get(&app, &format!("/start?t={t}"), "");
    let location = response.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap_or_default().to_string();
    assert!(location.starts_with("/live/w/") && location.ends_with('/'), "{response}");
    let page = get(&app, &location[5..], "");
    assert!(page.contains(r#"<base href="/live/">"#), "{page}");
    let overlay = get(&app, &format!("/start?t={t}&view=overlay&delay=4.5&names=1&x=y&score=%0d"), "");
    assert!(overlay.contains(&format!("Location: {location}overlay/4.5?names=1\r\n")), "{overlay}");
    let late = get(&app, &format!("/start?t={t}&delay=99"), "");
    assert!(late.contains(&format!("Location: {location}\r\n")), "{late}");
    let json = get(&app, &format!("/start?t={t}&format=json"), "");
    assert!(json.contains(&format!(r#"{{"page":"{location}"}}"#)) && json.contains("Access-Control-Allow-Origin: *"), "{json}");
    assert!(get(&app, "/start?t=bogus", "").starts_with("HTTP/1.1 400"));
    sessions_of(&app).stop_all();
}

#[test]
fn the_site_lists_the_matches_with_a_list_token() {
    let key = "11".repeat(32);
    let mut app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    assert!(get(&app, "/matches", "").starts_with("HTTP/1.1 404"), "only behind a site");
    app.keys = Some(Arc::new(token::Keys::from_hex(&key).unwrap()));
    let exp = unix_now() + 60;
    let start = token::seal(&key, &format!(r#"{{"connect":"192.0.2.1:27020 tv","reservation":5,"exp":{exp}}}"#), [4; 12]);
    assert!(get(&app, &format!("/start?t={start}"), "").starts_with("HTTP/1.1 303"));
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
    assert!(get(&app, &format!("/start?t={list}"), "").starts_with("HTTP/1.1 400"));
    sessions_of(&app).stop_all();
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
fn a_match_no_longer_watched_says_where_to_start_it_again() {
    let mut a = app(Matches::Many(Sessions::new(false, 1, 20.0)));
    let gone = |a: &App| get(a, "/w/nope/", "");
    let page = gone(&a);
    assert!(page.starts_with("HTTP/1.1 404") && page.contains(HTML) && page.contains("paste its connect string"), "{page}");
    a.keys = Some(Arc::new(token::Keys::from_hex(&"00".repeat(32)).unwrap()));
    let text = gone(&a);
    assert!(text.starts_with("HTTP/1.1 404") && text.contains(TEXT) && text.contains("open it again from the site"), "{text}");
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

/// A server whose store has match `id`'s demo of `cp_a` kept with comms in it.
fn kept_app(id: &str) -> (App, TempDir) {
    let dir = TempDir::new("kept-web");
    let header = stvtf::demo::Header {
        protocol: 24,
        server: String::new(),
        client: String::new(),
        map: "cp_a".into(),
        game: "tf".into(),
        seconds: 0.0,
        ticks: 0,
        frames: 0,
        signon_bytes: 0,
    };
    let file = std::fs::File::create(dir.join(format!("kept-{id}-100.dem"))).unwrap();
    let mut w = stvtf::demo::Writer::new(std::io::BufWriter::new(file), header, 0.015).unwrap();
    w.signon(b"signon").unwrap();
    w.message(100, b"game").unwrap();
    w.finish().unwrap();
    let said = r#"{"tick":100,"slot":3,"account":5,"frames":["aA=="]}"#;
    std::fs::write(dir.join(format!("kept-{id}-100.dem.voice")), format!("{said}\n")).unwrap();
    let mut sessions = Sessions::new(false, 4, 20.0);
    sessions.store = Some(sessions::Store { dir: dir.to_path_buf(), keys: None });
    (app(Matches::Many(sessions)), dir)
}

#[test]
fn a_matchs_demos_with_comms_are_listed_and_downloaded_the_match_gone_too() {
    let id = "ab".repeat(16);
    let (app, _dir) = kept_app(&id);
    let list = get(&app, &format!("/w/{id}/demos"), "");
    assert!(list.starts_with("HTTP/1.1 200"), "{list}");
    assert!(list.contains(&format!(r#""href":"/w/{id}/demos/100.dem""#)) && list.contains(r#""map":"cp_a""#), "{list}");
    let (head, body) = {
        let response = exchange(&app, format!("GET /w/{id}/demos/100.dem HTTP/1.1\r\n\r\n").as_bytes()).1;
        let (h, b) = head_and_body(&response);
        (h, b.to_vec())
    };
    assert!(
        head.starts_with("HTTP/1.1 200") && head.contains(r#"Content-Disposition: attachment; filename="cp_a-comms-100.dem""#),
        "{head}"
    );
    let demo = stvtf::demo::Reader::new(std::io::Cursor::new(&body)).unwrap();
    assert_eq!((demo.header.map.as_str(), demo.header.frames), ("cp_a", 2), "the frame and the voice");
    // Asked for with a mark, the answer carries it back in a cookie: the page then knows the demo is built.
    let marked = |mark: &str| get(&app, &format!("/w/{id}/demos/100.dem?built={mark}"), "");
    assert!(marked("a1b2").contains("Set-Cookie: built=a1b2; Path=/; Max-Age=60; SameSite=Lax"), "{}", marked("a1b2"));
    assert!(!marked("a;b").contains("Set-Cookie") && !head.contains("Set-Cookie"));
    // Asked for again soon after, it is the one built already (unless another was built between).
    let Matches::Many(sessions) = &app.matches else { panic!() };
    let dir = &sessions.store.as_ref().unwrap().dir;
    let again = || matches::built(dir, &id, "100").unwrap();
    assert!((0..5).any(|_| Arc::ptr_eq(&again(), &again())));
    // Nothing of another match, or by another name.
    let other = "cd".repeat(16);
    assert!(get(&app, &format!("/w/{other}/demos"), "").ends_with("[]"));
    assert!(get(&app, &format!("/w/{other}/demos/100.dem"), "").starts_with("HTTP/1.1 404"));
    assert!(get(&app, &format!("/w/{id}/demos/..%2f100.dem"), "").starts_with("HTTP/1.1 404"));
    // The match's page, gone, says what there is to download.
    let gone = get(&app, &format!("/w/{id}/"), "");
    assert!(gone.starts_with("HTTP/1.1 404") && gone.contains(&format!("/w/{id}/demos/100.dem")), "{gone}");
}
