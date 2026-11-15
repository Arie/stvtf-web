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
use std::time::{Duration, Instant};
use stvtf::Line;
use stvtf::output::{self, PlayerId};

mod comms;
mod files;
mod match_pages;
mod room_pages;
mod streams;
mod uploads;
mod voice;

/// The app served as the server serves it (`axum::serve`, the routes and their layers), on a
/// port of its own, until dropped.
struct Served {
    at: std::net::SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    server: Option<std::thread::JoinHandle<Result<()>>>,
}

impl Served {
    fn start(app: Arc<App>) -> Served {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let at = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            runtime.block_on(async {
                let stopped = async {
                    let _ = stopped.await;
                };
                server::serve(tokio::net::TcpListener::from_std(listener)?, app, stopped).await
            })
        });
        Served { at, stop: Some(stop), server: Some(server) }
    }

    /// Stops it once what is under way is done: its result.
    fn end(&mut self) -> Result<()> {
        drop(self.stop.take());
        self.server.take().map_or(Ok(()), |server| server.join().unwrap())
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.end();
    }
}

/// A response as the tests read it: the header names as they are written out in the specs
/// (`Content-Type`), where the server sends them in lower case.
fn named(response: &[u8]) -> Vec<u8> {
    let Some(end) = response.windows(4).position(|w| w == b"\r\n\r\n") else { return response.to_vec() };
    let head = String::from_utf8_lossy(&response[..end]);
    let title = |name: &str| {
        let part = |p: &str| p.chars().take(1).flat_map(char::to_uppercase).chain(p.chars().skip(1)).collect::<String>();
        name.split('-').map(part).collect::<Vec<_>>().join("-")
    };
    let lines = head.split("\r\n").enumerate().map(|(n, line)| match line.split_once(':') {
        Some((name, value)) if n > 0 => format!("{}:{value}", title(name)),
        _ => line.to_string(),
    });
    [lines.collect::<Vec<_>>().join("\r\n").as_bytes(), &response[end..]].concat()
}

/// Sends `send` to the app, as its one request on a connection; returns the server's result and
/// the response.
fn exchange(app: &TestApp, send: &[u8]) -> (Result<()>, Vec<u8>) {
    let mut served = Served::start(app.shared());
    let mut client = TcpStream::connect(served.at).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    // One request: the server closes the connection after its answer.
    let line = send.windows(2).position(|w| w == b"\r\n").map_or(send.len(), |at| at + 2);
    client.write_all(&[&send[..line], b"Connection: close\r\n", &send[line..]].concat()).unwrap();
    let mut response = Vec::new();
    let _ = client.read_to_end(&mut response);
    drop(client);
    (served.end(), named(&response))
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
