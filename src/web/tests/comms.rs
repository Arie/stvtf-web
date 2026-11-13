//! The Mumble channels the bot is asked into, and what it says of its channels.

use super::*;

fn bot(app: &App, method: &str, key: &str, body: &str) -> String {
    let send = format!("{method} /voice/mumble HTTP/1.1\r\nAuthorization: Bearer {key}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    request_to(app, send.as_bytes()).1
}

#[test]
fn a_mumble_channel_is_asked_for_on_the_page_and_the_bot_hears_of_it() {
    let (app, id) = voice_app();
    let page = format!("/w/{id}/");
    assert!(get(&app, &format!("{page}voice-comms"), "").ends_with(r#"{"discord":null,"mumble":[]}"#), "none yet");
    assert!(bot(&app, "GET", "k", "").contains(r#""jobs":[]"#));
    let ask = r#"{"host":"mumble.example","port":64738,"password":"pw","channel":"Team A","delay":0}"#;
    assert!(post(&app, &format!("{page}voice-mumble"), "", ask).starts_with("HTTP/1.1 204"));
    // The page sees where the bot is wanted, not the password.
    let shown = get(&app, &format!("{page}voice-comms"), "");
    assert!(
        shown.contains(r#""host":"mumble.example""#)
            && shown.contains(r#""channel":"Team A""#)
            && shown.contains(r#""status":"connecting""#),
        "{shown}"
    );
    assert!(!shown.contains("pw"), "{shown}");
    // The bot, with its key, gets it all, and the invite to post voice with.
    assert!(bot(&app, "GET", "wrong", "").starts_with("HTTP/1.1 401"));
    let jobs = bot(&app, "GET", "k", "");
    for part in [
        format!(r#""match":"{id}""#),
        format!(r#""invite":"{}""#, invite_of(&app, &id)),
        r#""password":"pw""#.into(),
        r#""port":64738"#.into(),
        r#""delay":0.0"#.into(),
    ] {
        assert!(jobs.contains(&part), "{part} in {jobs}");
    }
    // The bot waits for what changes, and hears of it at once: asked again with what it has, it
    // is answered when something is asked for or taken back, not before.
    let version = jobs.split(r#""version":"#).nth(1).unwrap().trim_end_matches('}').to_string();
    std::thread::scope(|scope| {
        let waiting = scope.spawn(|| {
            let started = Instant::now();
            let send = format!("GET /voice/mumble?since={version} HTTP/1.1\r\nAuthorization: Bearer k\r\n\r\n");
            (request_to(&app, send.as_bytes()).1, started.elapsed())
        });
        std::thread::sleep(Duration::from_millis(150));
        let other = r#"{"id":1,"host":"other.example","channel":"B"}"#;
        assert!(post(&app, &format!("{page}voice-mumble"), "", other).starts_with("HTTP/1.1 204"));
        let (answer, took) = waiting.join().unwrap();
        assert!(answer.contains("other.example"), "{answer}");
        assert!(took >= Duration::from_millis(100) && took < Duration::from_secs(5), "{took:?}");
    });
    // What the bot says of it reaches the page.
    let said = r#"{"kind":"mumble","status":"In Team A with 3 people"}"#;
    assert!(status(&app, &id, "k", said).starts_with("HTTP/1.1 204"));
    assert!(status(&app, &id, "wrong", said).starts_with("HTTP/1.1 401"));
    assert!(get(&app, &format!("{page}voice-comms"), "").contains("In Team A with 3 people"));
    // Sent away: the bot hears of it no more.
    assert!(post(&app, &format!("{page}voice-mumble"), "", r#"{"leave":true}"#).starts_with("HTTP/1.1 204"));
    assert!(bot(&app, "GET", "k", "").contains(r#""jobs":[]"#));
    assert!(get(&app, &format!("{page}voice-comms"), "").contains(r#""mumble":[]"#));
}

#[test]
fn the_same_mumble_channel_asked_for_again_is_joined_anew_but_not_for_a_new_delay() {
    let (app, id) = voice_app();
    let page = format!("/w/{id}/");
    let ask = |delay: u32| {
        let body = format!(r#"{{"host":"mumble.example","channel":"B","delay":{delay}}}"#);
        assert!(post(&app, &format!("{page}voice-mumble"), "", &body).starts_with("HTTP/1.1 204"));
    };
    let asked = || bot(&app, "GET", "k", "").split(r#""asked":"#).nth(1).unwrap().split([',', '}']).next().unwrap().to_string();
    ask(90);
    assert!(status(&app, &id, "k", r#"{"kind":"mumble","status":"B · 3"}"#).starts_with("HTTP/1.1 204"));
    let before = asked();
    // A new delay for where it sits: no joining anew, and how it goes stands.
    ask(30);
    assert!(get(&app, &format!("{page}voice-comms"), "").contains("B · 3"));
    assert_eq!(asked(), before);
    // The same asked for again: the bot is to join anew, and is on its way until it says so.
    ask(30);
    assert!(get(&app, &format!("{page}voice-comms"), "").contains(r#""status":"connecting""#));
    assert_ne!(asked(), before);
}

#[test]
fn a_mumble_server_is_somewhere_out_on_the_internet() {
    let (app, id) = voice_app();
    let page = format!("/w/{id}/");
    assert!(post(&app, &format!("{page}voice-mumble"), "", r#"{"host":"203.0.114.7","channel":"a"}"#).starts_with("HTTP/1.1 204"));
    // Not anywhere, nor anything, for a server.
    for bad in [
        r#"{"host":"mumble.example/x","channel":"a"}"#,
        r#"{"host":"","channel":"a"}"#,
        r#"{"host":"mumble.example","channel":""}"#,
        r#"{"host":"mumble.example","port":0,"channel":"a"}"#,
        r#"{"host":"mumble.example","channel":"a","delay":900}"#,
        // Nor this machine or a network of its own.
        r#"{"host":"localhost","channel":"a"}"#,
        r#"{"host":"relay","channel":"a"}"#,
        r#"{"host":"127.0.0.1","channel":"a"}"#,
        r#"{"host":"10.0.0.5","channel":"a"}"#,
        r#"{"host":"192.168.1.1","channel":"a"}"#,
        r#"{"host":"172.17.0.1","channel":"a"}"#,
        r#"{"host":"169.254.169.254","channel":"a"}"#,
        r#"{"host":"100.64.0.1","channel":"a"}"#,
        r#"{"host":"0.0.0.0","channel":"a"}"#,
        r#"{"host":"db.internal","channel":"a"}"#,
    ] {
        assert!(post(&app, &format!("{page}voice-mumble"), "", bad).starts_with("HTTP/1.1 400"), "{bad}");
    }
}

#[test]
fn at_a_lan_a_mumble_server_may_be_on_the_network_there() {
    let (mut app, id) = voice_app();
    app.voice.mumble_anywhere = true;
    let page = format!("/w/{id}/");
    for near in [r#"{"host":"192.168.1.20","channel":"a"}"#, r#"{"host":"mumble.lan","channel":"a"}"#, r#"{"host":"mumble","channel":"a"}"#]
    {
        assert!(post(&app, &format!("{page}voice-mumble"), "", near).starts_with("HTTP/1.1 204"), "{near}");
    }
    // Still a server's name or address, and nothing else.
    assert!(post(&app, &format!("{page}voice-mumble"), "", r#"{"host":"a/b","channel":"a"}"#).starts_with("HTTP/1.1 400"));
}

#[test]
fn two_teams_channels_in_one_match_each_keep_their_own_people_and_how_it_goes() {
    let (app, id) = voice_app();
    let page = format!("/w/{id}/");
    let says = |body: &str| assert!(status(&app, &id, "k", body).starts_with("HTTP/1.1 204"), "{body}");
    says(r#"{"kind":"discord","channel":"111","status":"Red · 1","people":[{"id":"77","name":"gummy"}]}"#);
    says(r#"{"kind":"discord","channel":"222","status":"Blu · 1","people":[{"id":"88","name":"aoki"}]}"#);
    let voices = get(&app, &format!("{page}voices"), "");
    assert!(voices.contains(r#""name":"gummy""#) && voices.contains(r#""name":"aoki""#), "{voices}");
    let comms = get(&app, &format!("{page}voice-comms"), "");
    assert!(comms.contains("Red · 1") && comms.contains("Blu · 1"), "{comms}");
    // One leaves: the other's people and how it goes stand.
    says(r#"{"kind":"discord","channel":"111","left":true}"#);
    let voices = get(&app, &format!("{page}voices"), "");
    assert!(!voices.contains("gummy") && voices.contains("aoki"), "{voices}");
    let comms = get(&app, &format!("{page}voice-comms"), "");
    assert!(!comms.contains("Red") && comms.contains("Blu · 1"), "{comms}");
}

#[test]
fn what_the_bot_says_of_a_mumble_channel_it_was_since_asked_out_of_is_not_taken() {
    let (app, id) = voice_app();
    let page = format!("/w/{id}/");
    let ask = |channel: &str, which: &str| {
        let body = format!(r#"{{{which}"host":"mumble.example","channel":"{channel}"}}"#);
        assert!(post(&app, &format!("{page}voice-mumble"), "", &body).starts_with("HTTP/1.1 204"));
    };
    ask("A", "");
    ask("B", r#""id":1,"#);
    assert!(
        status(&app, &id, "k", r#"{"kind":"mumble","asked":1,"status":"Left: moved","people":[{"id":"77","name":"late"}]}"#)
            .starts_with("HTTP/1.1 204")
    );
    assert!(get(&app, &format!("{page}voice-comms"), "").contains(r#""status":"connecting""#));
    assert!(!get(&app, &format!("{page}voices"), "").contains("late"));
    assert!(status(&app, &id, "k", r#"{"kind":"mumble","asked":2,"status":"B · 1"}"#).starts_with("HTTP/1.1 204"));
    assert!(get(&app, &format!("{page}voice-comms"), "").contains("B · 1"));
}

#[test]
fn those_in_a_mumble_channel_the_bot_was_taken_out_of_are_no_longer_listed() {
    let (app, id) = voice_app();
    let page = format!("/w/{id}/");
    let ask = |body: &str| assert!(post(&app, &format!("{page}voice-mumble"), "", body).starts_with("HTTP/1.1 204"));
    let there =
        r#"{"kind":"mumble","channel":"mumble:mumble.example:A","status":"A · 1","people":[{"id":"mumble:cert:a1","name":"in mumble"}]}"#;
    ask(r#"{"host":"mumble.example","channel":"A"}"#);
    assert!(status(&app, &id, "k", there).starts_with("HTTP/1.1 204"));
    assert!(
        status(&app, &id, "k", r#"{"kind":"discord","channel":"111","status":"D · 1","people":[{"id":"77","name":"in discord"}]}"#)
            .starts_with("HTTP/1.1 204")
    );
    assert!(get(&app, &format!("{page}voices"), "").contains("in mumble"));
    // Moved to another channel: those of the old one go, at once.
    ask(r#"{"id":1,"host":"mumble.example","channel":"B"}"#);
    let voices = get(&app, &format!("{page}voices"), "");
    assert!(!voices.contains("in mumble") && voices.contains("in discord"), "{voices}");
    // And taken out altogether.
    assert!(status(&app, &id, "k", &there.replace(":A", ":B")).starts_with("HTTP/1.1 204"));
    ask(r#"{"leave":true}"#);
    let voices = get(&app, &format!("{page}voices"), "");
    assert!(!voices.contains("in mumble") && voices.contains("in discord"), "{voices}");
}

#[test]
fn two_teams_are_each_in_a_mumble_channel_of_their_own_on_one_server_or_two() {
    let (app, id) = voice_app();
    let page = format!("/w/{id}/");
    let ask = |body: &str| post(&app, &format!("{page}voice-mumble"), "", body);
    let comms = || get(&app, &format!("{page}voice-comms"), "");
    assert!(ask(r#"{"host":"mumble.example","password":"pw","channel":"Red"}"#).starts_with("HTTP/1.1 204"));
    // The other team's channel on the same server: its password is the one given already.
    assert!(ask(r#"{"host":"mumble.example","channel":"Blu"}"#).starts_with("HTTP/1.1 204"));
    assert!(ask(r#"{"host":"other.example","channel":"Casters","delay":0}"#).starts_with("HTTP/1.1 204"));
    let shown = comms();
    for part in [r#""id":1"#, r#""id":2"#, r#""id":3"#, r#""channel":"Red""#, r#""channel":"Blu""#, r#""host":"other.example""#] {
        assert!(shown.contains(part), "{part} in {shown}");
    }
    // The bot is asked into all three, each by its number, and says how each goes.
    let jobs = bot(&app, "GET", "k", "");
    assert_eq!(jobs.matches(r#""password":"pw""#).count(), 2, "{jobs}");
    assert!(jobs.contains(r#""job":1"#) && jobs.contains(r#""job":2"#) && jobs.contains(r#""job":3"#), "{jobs}");
    let says = |body: &str| assert!(status(&app, &id, "k", body).starts_with("HTTP/1.1 204"), "{body}");
    says(
        r#"{"kind":"mumble","job":1,"asked":1,"channel":"mumble:mumble.example:Red","status":"Red · 1","people":[{"id":"mumble:cert:r1","name":"on red"}]}"#,
    );
    says(
        r#"{"kind":"mumble","job":2,"asked":1,"channel":"mumble:mumble.example:Blu","status":"Blu · 1","people":[{"id":"mumble:cert:b1","name":"on blu"}]}"#,
    );
    let shown = comms();
    assert!(shown.contains("Red · 1") && shown.contains("Blu · 1") && shown.contains(r#""status":"connecting""#), "{shown}");
    let voices = get(&app, &format!("{page}voices"), "");
    assert!(voices.contains("on red") && voices.contains("on blu"), "{voices}");
    // One is taken out by its number: the others stay, and so do their people.
    assert!(ask(r#"{"leave":true,"id":1}"#).starts_with("HTTP/1.1 204"));
    let (shown, voices) = (comms(), get(&app, &format!("{page}voices"), ""));
    assert!(!shown.contains(r#""channel":"Red""#) && shown.contains("Blu · 1"), "{shown}");
    assert!(!voices.contains("on red") && voices.contains("on blu"), "{voices}");
    // One is moved by its number; a fifth is one too many; all leave together.
    assert!(ask(r#"{"id":2,"host":"mumble.example","channel":"Blu 2"}"#).starts_with("HTTP/1.1 204"));
    assert!(comms().contains(r#""channel":"Blu 2""#) && !comms().contains(r#""channel":"Blu""#));
    for n in 0..2 {
        assert!(ask(&format!(r#"{{"host":"more.example","channel":"c{n}"}}"#)).starts_with("HTTP/1.1 204"));
    }
    assert!(ask(r#"{"host":"more.example","channel":"one too many"}"#).starts_with("HTTP/1.1 400"));
    assert!(ask(r#"{"leave":true}"#).starts_with("HTTP/1.1 204"));
    assert!(comms().ends_with(r#""mumble":[]}"#), "{}", comms());
}

#[test]
fn behind_a_site_only_who_has_a_say_brings_mumble_in() {
    let (mut app, key, _dir) = site_app();
    app.voice.key = Some("k".into());
    let exp = unix_now() + 60;
    let start = format!(r#"{{"connect":"connect 192.0.2.9:27020; password \"tv\"","owner":"7656","exp":{exp}}}"#);
    let opened = get(&app, &format!("/start?t={}", token::seal(&key, &start, [3; 12])), "");
    let page = opened.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap().to_string();
    let starter = signed_in(&app, &key, r#"{"uid":"7656","name":"Arie","streamer":false}"#);
    let other = signed_in(&app, &key, r#"{"uid":"1111","name":"someone","streamer":false}"#);
    let ask = r#"{"host":"mumble.example","channel":"Team A"}"#;
    assert!(post(&app, &format!("{page}voice-mumble"), &other, ask).starts_with("HTTP/1.1 403"));
    assert!(post(&app, &format!("{page}voice-mumble"), &starter, ask).starts_with("HTTP/1.1 204"));
    assert!(get(&app, &format!("{page}voice-comms"), &other).starts_with("HTTP/1.1 204"), "nor sees where it is");
    assert!(get(&app, &format!("{page}voice-comms"), &starter).contains("mumble.example"));
}

/// Runs `serve` on `send` from a client that reads nothing until the answer is stuck on its way:
/// whether `free` held then.
fn free_while_answering(app: &App, send: &[u8], free: impl Fn() -> bool) -> bool {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    client.write_all(send).unwrap();
    // Nothing more fits on the way to the client.
    server.set_nonblocking(true).unwrap();
    let block = vec![0; 1 << 16];
    while server.write(&block).is_ok() {}
    server.set_nonblocking(false).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| serve(server, app));
        std::thread::sleep(Duration::from_millis(300));
        let was = free();
        let _ = std::io::copy(&mut client, &mut std::io::sink());
        was
    })
}

#[test]
fn a_refusal_is_answered_with_the_match_not_held() {
    let (app, id) = voice_app();
    let m = sessions_of(&app).get(&id).unwrap();
    let free = || m.comms.mumble.try_lock().is_ok();
    let bad = r#"{"host":"","channel":"a"}"#;
    let send = format!("POST /w/{id}/voice-mumble HTTP/1.1\r\nContent-Length: {}\r\n\r\n{bad}", bad.len());
    assert!(free_while_answering(&app, send.as_bytes(), free), "a channel refused");
    let said = invited(&app, &id, r#"{"kind":"mumble","job":9,"status":"x"}"#);
    let send = format!("POST /w/{id}/voice-status HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: {}\r\n\r\n{said}", said.len());
    assert!(free_while_answering(&app, send.as_bytes(), free), "a channel's status not taken");
}
