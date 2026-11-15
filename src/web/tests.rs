use super::*;
use crate::matches::hub::{SharedHub, publish};
use crate::testing::{TempDir, TestApp, app, server, web_server};
use crate::web::client::{client_key, client_of, is_cloudflare, on_our_network};
use crate::web::matches::start_view;
use crate::{App, Matches};
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use stvtf::Line;
use stvtf::output::{self, PlayerId};

mod comms;
mod files;
mod match_pages;
mod room_pages;
mod streams;
mod uploads;
mod voice;

/// Serves one connection as the server does, the framework and its layers in between; the
/// response's headers are written as they were named, and the connection closes after it.
fn serve(conn: TcpStream, app: &Arc<App>) -> Result<()> {
    let peer = conn.peer_addr()?;
    conn.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let mut http = server::http();
        http.keep_alive(false).title_case_headers(true);
        server::connection(tokio::net::TcpStream::from_std(conn)?, peer, server::router(app.clone()), &http).await
    })
}

/// Runs `serve` on one connection, with `send` written by the client; returns serve's result and
/// the response.
fn exchange(app: &TestApp, send: &[u8]) -> (Result<()>, Vec<u8>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    client.write_all(send).unwrap();
    let result = serve(server, &app.shared());
    let mut response = Vec::new();
    let _ = client.read_to_end(&mut response);
    (result, response)
}

fn request_to(app: &TestApp, send: &[u8]) -> (Result<()>, String) {
    let (result, response) = exchange(app, send);
    (result, String::from_utf8_lossy(&response).into_owned())
}

/// A response's head, and its body, which may be binary.
fn head_and_body(response: &[u8]) -> (String, &[u8]) {
    let end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    (String::from_utf8_lossy(&response[..end]).into_owned(), &response[end + 4..])
}

/// A query string's values, as a request's.
fn asks(query: &str) -> HashMap<String, String> {
    serde_urlencoded::from_str(query).unwrap()
}

/// The page's stylesheets and modules as the server has them: each one's name under `app/`, its
/// type and its body; `style.css`, the page's, first.
fn app_files() -> Vec<(&'static str, &'static str, &'static [u8])> {
    let typed = |(name, body): &'static (String, Vec<u8>)| {
        let css = std::path::Path::new(name).extension().is_some_and(|e| e == "css");
        (name.as_str(), if css { CSS } else { JS }, body.as_slice())
    };
    assets::app_files().iter().map(typed).collect()
}

fn sessions_of(app: &TestApp) -> &Sessions {
    let Matches::Many(sessions) = &app.matches else { panic!("one match, not many") };
    sessions
}

/// A server with a pinned demo rooms can play, and the folder its recording is in.
fn demo_app() -> (TestApp, TempDir) {
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
fn site_app() -> (TestApp, String, TempDir) {
    let (mut app, dir) = demo_app();
    let key = "42".repeat(32);
    app.keys = Some(Arc::new(token::Keys::from_hex(&key).unwrap()));
    app.home = Some("https://serveme.tf/stv".into());
    (app, key, dir)
}

/// `stvtf_user` as the site's sign-in leaves it, for `json` (`{"uid", "name", "streamer"}`).
fn signed_in(app: &TestApp, key: &str, json: &str) -> String {
    let t = token::seal(key, &format!(r#"{{"signin":{json},"exp":{}}}"#, unix_now() + 60), [5; 12]);
    let back = get(app, &format!("/signed_in?t={t}&return=/r/x/"), "");
    assert!(back.starts_with("HTTP/1.1 303") && back.contains("Location: /r/x/"), "{back}");
    let cookie = cookie_of(&back);
    assert!(back.contains("HttpOnly; SameSite=Lax; Secure"), "{back}");
    cookie
}

/// The `page` a response's JSON sends the browser to.
fn page_of(response: &str) -> String {
    response.split(r#""page":""#).nth(1).unwrap().split('"').next().unwrap().to_string()
}

/// The cookie a response sets, without its attributes.
fn cookie_of(response: &str) -> String {
    response.split("Set-Cookie: ").nth(1).unwrap().split(';').next().unwrap().to_string()
}

fn post(app: &TestApp, path: &str, cookie: &str, body: &str) -> String {
    // As a page sends it: JSON, a form, or nothing.
    let ctype = match body {
        "" => String::new(),
        form if path == "/watch" && !form.starts_with('{') => "Content-Type: application/x-www-form-urlencoded\r\n".into(),
        _ => "Content-Type: application/json\r\n".into(),
    };
    let send = format!("POST {path} HTTP/1.1\r\nCookie: {cookie}\r\n{ctype}Content-Length: {}\r\n\r\n{body}", body.len());
    request_to(app, send.as_bytes()).1
}

/// `path`, with `cookie` unless it is empty.
fn get(app: &TestApp, path: &str, cookie: &str) -> String {
    let cookie = if cookie.is_empty() { String::new() } else { format!("Cookie: {cookie}\r\n") };
    request_to(app, format!("GET {path} HTTP/1.1\r\n{cookie}\r\n").as_bytes()).1
}

/// Opens a room on the demo: its page and the owner's cookie.
fn open_room(app: &TestApp) -> (String, String) {
    let opened = post(app, "/w/demo/room", "", "");
    assert!(opened.starts_with("HTTP/1.1 201"), "{opened}");
    let cookie = cookie_of(&opened);
    assert!(opened.contains("HttpOnly; SameSite=Lax"), "{opened}");
    let page = page_of(&opened);
    (page, cookie)
}

fn post_watch(app: &TestApp, connect: &str) -> String {
    let body = format!("connect={connect}");
    let send = format!(
        "POST /watch HTTP/1.1\r\nHost: x\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    request_to(app, send.as_bytes()).1
}

/// An app that takes voice with key "k", and the id of a live match on it (watching nowhere).
fn voice_app() -> (TestApp, String) {
    let mut app = app(Matches::Many(Sessions::new(false, 4, 20.0)));
    app.voice.key = Some("k".into());
    let started = post_watch(&app, "connect+192.0.2.1%3A27020%3B+password+%22tv%22");
    let location = started.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap();
    let id = location.trim_matches('/').trim_start_matches("w/").to_string();
    (app, id)
}

/// The match's voice invite.
fn invite_of(app: &TestApp, id: &str) -> String {
    let Matches::Many(sessions) = &app.matches else { panic!() };
    sessions.get(id).unwrap().voice_invite().to_string()
}

/// `body` with the match's invite in it.
fn invited(app: &TestApp, id: &str, body: &str) -> String {
    body.replacen('{', &format!(r#"{{"invite":"{}","#, invite_of(app, id)), 1)
}

/// The bot says how it goes in a channel brought into match `id`, and who is in it.
fn status(app: &TestApp, id: &str, key: &str, body: &str) -> String {
    let body = invited(app, id, body);
    let send = format!(
        "POST /w/{id}/voice-status HTTP/1.1\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    request_to(app, send.as_bytes()).1
}
