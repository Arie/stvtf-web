//! A demo uploaded here, in parts, and played.

use super::*;

/// A server that keeps demos, and its folder.
fn upload_app() -> (TestApp, TempDir) {
    let dir = TempDir::new("uploads");
    let mut sessions = Sessions::new(false, 8, 20.0);
    sessions.store = Some(sessions::Store { dir: dir.to_path_buf(), keys: None });
    (app(Matches::Many(sessions)), dir)
}

fn put(app: &TestApp, path: &str, body: &[u8]) -> String {
    let mut send = format!("PUT {path} HTTP/1.1\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    send.extend_from_slice(body);
    request_to(app, &send).1
}

/// Sends `demo` in two parts; what the last request answers.
fn send(app: &TestApp, demo: &[u8], name: &str) -> String {
    let begun = post(app, "/upload", "", &format!(r#"{{"size":{}}}"#, demo.len()));
    assert!(begun.starts_with("HTTP/1.1 200") && begun.contains(r#""part":33554432"#), "{begun}");
    let id = begun.split(r#""id":""#).nth(1).unwrap().split('"').next().unwrap().to_string();
    let half = demo.len() / 2;
    assert!(put(app, &format!("/upload/{id}?at=0"), &demo[..half]).ends_with(&half.to_string()));
    // A part sent again (its answer was lost) is not added twice: where it is, instead.
    let again = put(app, &format!("/upload/{id}?at=0"), &demo[..half]);
    assert!(again.starts_with("HTTP/1.1 409") && again.ends_with(&half.to_string()), "{again}");
    assert!(put(app, &format!("/upload/{id}?at={half}"), &demo[half..]).ends_with(&demo.len().to_string()));
    post(app, &format!("/upload/{id}"), "", &format!(r#"{{"name":"{name}"}}"#))
}

#[test]
fn a_demo_uploaded_in_parts_plays_as_a_match_and_again_while_it_is_kept() {
    let (app, dir) = upload_app();
    assert!(get(&app, "/", "").contains(r#"data-signed="1""#), "nobody signs in here: anyone");
    assert!(get(&app, "/upload", "").contains("Location: /\r\n"), "the form is on the front page");
    assert!(get(&app, "/mumble", "").contains("<td>stv.tf-2</td>"));
    let listed = get(&app, "/mumble.json", "");
    let listed: serde_json::Value = serde_json::from_str(listed.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["sha1"].as_str().unwrap().len() == 40 && c["name"].as_str().unwrap().starts_with("stv.tf"))
    );
    let demo = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../stvtf/tests/fixtures/tc_hydro.dem")).unwrap();
    let done = send(&app, &demo, "match.dem");
    assert!(done.starts_with("HTTP/1.1 200"), "{done}");
    let page = page_of(&done);
    let number = sessions::id_demo(page.trim_matches('/').trim_start_matches("w/")).unwrap();
    assert!(sessions::is_own_demo(number) && page.starts_with("/w/demo/") && page.len() == "/w/demo/".len() + 14, "{page}");
    assert!(get(&app, &page, "").starts_with("HTTP/1.1 200"));
    let info = std::fs::read_to_string(dir.join(sessions::demo_info(number))).unwrap();
    assert!(info.contains(r#""pov":false"#) && info.contains(r#""map":"tc_hydro""#) && info.contains(r#""name":"match.dem""#), "{info}");
    // The same file again, from anyone, is the one kept.
    assert_eq!(page_of(&send(&app, &demo, "again.dem")), page);
    assert_eq!(std::fs::read_dir(&*dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count(), 0);
    // Its match ended, the link plays it again.
    let sessions = sessions_of(&app);
    let m = sessions.get(&sessions::demo_id(number)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !dir.join(sessions::demo_output(number)).exists() {
        assert!(Instant::now() < deadline, "never decoded");
        std::thread::sleep(Duration::from_millis(10));
    }
    sessions.stop_all();
    drop(m);
    assert!(get(&app, &page, "").starts_with("HTTP/1.1 200"), "played again from what is kept");
    sessions.stop_all();
}

#[test]
fn a_demo_sent_zipped_is_the_demo_and_someones_next_takes_the_place_of_their_last() {
    let (app, dir) = upload_app();
    let demo = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../stvtf/tests/fixtures/tc_hydro.dem")).unwrap();
    let plain = send(&app, &demo, "match.dem");
    let zipped = send(&app, &unpack::tests::zip_of(&[("match.dem", &demo, 8)]), "match.zip");
    assert!(zipped.starts_with("HTTP/1.1 200"), "{zipped}");
    assert_eq!(plain.split("\r\n\r\n").nth(1), zipped.split("\r\n\r\n").nth(1), "the same demo, the same link");
    let refused = send(&app, &unpack::tests::zip_of(&[("notes.txt", &[7; 2000], 0)]), "x.zip");
    assert!(refused.starts_with("HTTP/1.1 400") && refused.contains("No demo in it"), "{refused}");
    // One of each person is kept.
    let store = sessions::Store { dir: dir.to_path_buf(), keys: None };
    let keep = |bytes: &[u8], by| {
        let part = dir.join("demo-up-x.part");
        std::fs::write(&part, bytes).unwrap();
        upload::keep(&store, &part, "x.dem", Some(by), None).unwrap()
    };
    let other = [&demo[..], &[0]].concat();
    let (first, theirs) = (keep(&[&demo[..], &[9]].concat(), "steam:1"), keep(&other, "steam:2"));
    let kept = |demo| store.has(&sessions::demo_info(demo));
    assert!(kept(first) && kept(theirs));
    let second = keep(&[&demo[..], &[0, 0]].concat(), "steam:1");
    assert!(!kept(first) && kept(second) && kept(theirs), "their last went, another's stays");
    // Theirs to list: by who they count as, or by who signed in (a streamer counts as nobody).
    assert_eq!(upload::kept_by(&store, None, Some("steam:1")), [(second, "x.dem".to_string())]);
    let part = dir.join("demo-up-y.part");
    std::fs::write(&part, [&demo[..], &[1, 2, 3]].concat()).unwrap();
    let streamers = upload::keep(&store, &part, "<b>.dem", None, Some("u9")).unwrap();
    assert_eq!(upload::kept_by(&store, Some("u9"), None), [(streamers, "<b>.dem".to_string())]);
    assert_eq!(upload::kept_by(&store, None, None), []);
}

#[test]
fn what_is_no_tf2_demo_or_too_big_or_from_nobody_signed_in_is_refused() {
    let (mut app, dir) = upload_app();
    let junk = vec![7u8; 2000];
    let refused = send(&app, &junk, "x.dem");
    assert!(refused.starts_with("HTTP/1.1 400") && refused.contains("Not a TF2 demo"), "{refused}");
    assert_eq!(std::fs::read_dir(&*dir).unwrap().count(), 0, "nothing of it is kept");
    assert!(post(&app, "/upload", "", r#"{"size":10}"#).contains("Not a demo"));
    assert!(post(&app, "/upload", "", r#"{"size":999999999999}"#).contains("Over 512 MB"));
    assert!(put(&app, &format!("/upload/{}?at=0", "ab".repeat(16)), b"x").starts_with("HTTP/1.1 404"));
    assert!(put(&app, "/upload/..%2fx?at=0", b"x").starts_with("HTTP/1.1 404"));
    // Behind a site: signed in.
    app.keys = Some(Arc::new(token::Keys::from_hex(&"42".repeat(32)).unwrap()));
    assert!(get(&app, "/", "").contains(r#"data-signed="""#));
    let refused = post(&app, "/upload", "", r#"{"size":5000}"#);
    assert!(refused.starts_with("HTTP/1.1 403") && refused.contains("Sign in first"), "{refused}");
}
