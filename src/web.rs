//! The HTTP side: pages, files, event streams and starting matches.

use crate::matches::hub::SharedHub;
use crate::matches::recording::Mark;
use crate::matches::sessions;
use crate::matches::sessions::Sessions;
use crate::{App, Matches, lock};
use anyhow::{Result, bail};
use std::io::Write;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod assets;
mod client;
mod events;
mod matches;
mod mine;
mod rooms;
pub(crate) mod server;
pub(crate) mod token;
mod unpack;
mod upload;
mod visitor;
mod voice_in;
pub(crate) mod voice_link;
pub(crate) mod voice_mumble;
pub(crate) mod voice_routes;
pub(crate) mod voice_who;
pub(crate) mod voice_who_sync;

use assets::serve_file;
pub(crate) use assets::{app_version, module_preloads};
use client::{client_of, counted};
use events::{stream, too_many_streams, wants_detail, wants_voice};
use matches::{is_page, list_matches, serve_match, start, start_view};
use rooms::serve_room;
use visitor::{origin, return_to, set_cookie, url_encode};

const INDEX: &str = include_str!("../app/pages/index.html");
const LANDING: &str = include_str!("../app/pages/landing.html");
/// The voice bot's Mumble certificates: a page, and JSON.
const MUMBLE: &str = include_str!("../app/pages/mumble.html");
const MUMBLE_JSON: &str = include_str!("../app/pages/mumble.json");
const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const TEXT: &str = "text/plain";
const JSON: &str = "application/json";
const WEBP: &str = "image/webp";
const SVG: &str = "image/svg+xml";
const WOFF2: &str = "font/woff2";

/// Who the browser is, to the rooms; and who signed in with the site, still sealed.
const GUEST_COOKIE: &str = "stvtf_guest";
const USER_COOKIE: &str = "stvtf_user";

/// Request line and headers together; the pages send far less.
const MAX_REQUEST: u64 = 16 << 10;

/// Event streams from one address, so a single client cannot take all the connections. A LAN
/// event or an office behind one address stays well under it.
pub(crate) const MAX_STREAMS_PER_CLIENT: usize = 32;

/// The longest delay a page can ask for after its last `/`, to line up with a stream of the match.
const MAX_DELAY_S: f64 = 60.0;

/// How long a browser keeps a file that does not change for a given build.
const CACHE_FOR: Duration = Duration::from_hours(24);

/// What a request is answered with: its handler puts it here, the server sends it.
#[derive(Clone, Default)]
pub(crate) struct Out(Arc<Mutex<Option<Reply>>>);

pub(crate) struct Reply {
    pub(crate) status: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Body,
}

/// A whole body, or events written as they come until the client is gone.
pub(crate) enum Body {
    Whole(Vec<u8>),
    Events(Events),
}

/// Writes events until the client is gone.
pub(crate) type Events = Box<dyn FnOnce(&mut dyn Write) -> Result<()> + Send>;

impl Out {
    fn put(&self, status: &str, headers: &[(&str, &str)], body: Body) -> std::io::Result<()> {
        let mut reply = lock(&self.0);
        if reply.is_some() {
            return Err(std::io::Error::other("answered twice"));
        }
        let headers = headers.iter().map(|(n, v)| ((*n).to_string(), (*v).to_string())).collect();
        *reply = Some(Reply { status: status.to_string(), headers, body });
        Ok(())
    }

    pub(crate) fn take(&self) -> Option<Reply> {
        lock(&self.0).take()
    }
}

/// What the server needs of a request, and its body.
struct Request {
    method: String,
    path: String,
    query: String,
    length: usize,
    gzip: bool,
    /// `X-Forwarded-For`, nearest hop last; empty if any of it is not an address.
    forwarded: Vec<IpAddr>,
    /// An `Authorization: Bearer` token.
    bearer: Option<String>,
    /// The [`GUEST_COOKIE`].
    guest: Option<String>,
    /// The [`USER_COOKIE`].
    user: Option<String>,
    /// The cookie of a Discord sign-in begun ([`voice_link::STATE_COOKIE`]).
    voice_link: Option<String>,
    /// `User-Agent`.
    agent: String,
    body: Vec<u8>,
}

/// Cookie `name`'s value in a `Cookie` header.
fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|c| c.trim().strip_prefix(name)?.strip_prefix('='))
}

/// A query string's or form's value for `name`, undecoded.
fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v))
}

/// A whole response.
fn respond_with(s: &mut Out, status: &str, headers: &[(&str, &str)], body: &[u8]) -> std::io::Result<()> {
    s.put(status, headers, Body::Whole(body.to_vec()))
}

fn respond(s: &mut Out, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    // Pages name the build's files: no-cache, so a reload gets a new build.
    if ctype == HTML {
        return respond_with(s, status, &[("Content-Type", ctype), ("Cache-Control", "no-cache")], body);
    }
    respond_with(s, status, &[("Content-Type", ctype)], body)
}

/// A file that never changes for a given build, so the browser keeps it.
fn respond_cached(s: &mut Out, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let max_age = format!("max-age={}", CACHE_FOR.as_secs());
    respond_with(s, "200 OK", &[("Content-Type", ctype), ("Cache-Control", &max_age)], body)
}

/// `status`, with why as text.
fn refuse(s: &mut Out, status: &str, why: &anyhow::Error) -> std::io::Result<()> {
    respond(s, status, TEXT, format!("{why:#}").as_bytes())
}

fn not_found(s: &mut Out) -> std::io::Result<()> {
    respond(s, "404 Not Found", TEXT, b"not found")
}

pub(crate) fn redirect(s: &mut Out, status: &str, location: &str) -> std::io::Result<()> {
    respond_with(s, status, &[("Location", location)], b"")
}

/// The most a request's body may be: a part of an upload, or little.
fn body_limit(request: &Request) -> usize {
    if request.method == "PUT" && request.path.starts_with("/upload/") { upload::MAX_PART } else { MAX_REQUEST as usize }
}

/// The answer to `request`, from `client`.
fn answer(mut s: Out, app: &App, request: &Request, client: IpAddr) -> Result<()> {
    match &app.matches {
        _ if request.path == "/up" => respond(&mut s, "200 OK", TEXT, b"ok")?,
        Matches::One(hub) => serve_one(s, app, hub, request, client)?,
        Matches::Many(sessions) => serve_many(s, app, sessions, request, client)?,
    }
    Ok(())
}

/// The one match a server given a server address watches, at `/`.
fn serve_one(mut s: Out, app: &App, hub: &SharedHub, request: &Request, client: IpAddr) -> Result<()> {
    match request.path.as_str() {
        p if p == "/index.html" || p.strip_prefix('/').is_some_and(is_page) => respond(&mut s, "200 OK", HTML, &app.page(INDEX))?,
        "/events" => match app.stream_slot(client) {
            Some(slot) => stream(&s, hub, request.gzip, wants_detail(&request.query), wants_voice(&request.query), slot)?,
            None => too_many_streams(&mut s)?,
        },
        path => serve_file(&mut s, app, path, request.gzip)?,
    }
    Ok(())
}

/// Any number of matches, each at `/w/<id>/`, and the ways to start one.
fn serve_many(mut s: Out, app: &App, sessions: &Sessions, request: &Request, client: IpAddr) -> Result<()> {
    match (request.path.as_str(), request.method.as_str()) {
        ("/matches", "GET") if let Some(keys) = &app.keys => list_matches(&mut s, keys, sessions, request)?,
        ("/mumble", "GET") => respond(&mut s, "200 OK", HTML, &app.page(MUMBLE))?,
        ("/mumble.json", "GET") => respond(&mut s, "200 OK", JSON, MUMBLE_JSON.as_bytes())?,
        (path, _) if path == "/upload" || path.starts_with("/upload/") => {
            upload::serve_upload(&mut s, app, sessions, request, client)?;
        }
        ("/signin", "GET") if let Some(site) = app.home.as_deref().and_then(origin) => {
            redirect(&mut s, "302 Found", &format!("{site}/stv/sign_in?return={}", url_encode(&return_to(app, request))))?;
        }
        ("/signed_in", "GET") if let Some(keys) = &app.keys => {
            match keys.open_signin(param(&request.query, "t").unwrap_or_default(), unix_now()) {
                Ok(user) => {
                    let kept = set_cookie(app, USER_COOKIE, &keys.keep(&user, unix_now()), token::SIGNED_IN_FOR);
                    back_with(&mut s, app, request, &kept)?;
                }
                Err(e) => refuse(&mut s, "400 Bad Request", &e)?,
            }
        }
        ("/signout", "GET") => back_with(&mut s, app, request, &set_cookie(app, USER_COOKIE, "", 0))?,
        ("/voice/link", "GET") => voice_link::link(&mut s, app)?,
        ("/voice/linked", "GET") => voice_link::linked(&mut s, app, request)?,
        ("/voice/who", "GET") => voice_link::who(&mut s, app, request)?,
        ("/voice/mumble", "GET") => voice_mumble::get_mumble_jobs(&mut s, app, sessions, request)?,
        ("/voice/known", "GET") => voice_who_sync::get_known(&mut s, app, request)?,
        ("/voice/known", "POST") => voice_who_sync::post_known(&mut s, app, request)?,
        ("/start", _) => match start(app, sessions, request, client) {
            // <stv-map> asks where the match is instead of following the redirect.
            Ok(m) if param(&request.query, "format") == Some("json") => {
                let page = format!("{}/w/{}/", app.base, m.id);
                respond(&mut s, "200 OK", JSON, serde_json::json!({ "page": page }).to_string().as_bytes())?;
            }
            Ok(m) => app.redirect(&mut s, "303 See Other", &format!("/w/{}/{}", m.id, start_view(&request.query)))?,
            Err(e) => refuse(&mut s, "400 Bad Request", &e)?,
        },
        ("/", "GET") => respond(&mut s, "200 OK", HTML, &app.page(&landing(app, sessions, request, client, "")))?,
        // What a page says went wrong in it (see `report`): taken or not, it is not told.
        ("/fault", "POST") => {
            let who = visitor::who(app, request).user;
            crate::report::page_fault(&read_body(request)?, &request.agent, who.as_ref().map(|u| u.uid.as_str()));
            respond(&mut s, "204 No Content", TEXT, b"")?;
        }
        ("/watch", "POST") if request.length as u64 > MAX_REQUEST => {
            respond(&mut s, "413 Content Too Large", TEXT, format!("over {MAX_REQUEST} bytes").as_bytes())?;
        }
        ("/watch", "POST") => post_watch(&mut s, app, sessions, request, client)?,
        (p, _) if let Some(path) = p.strip_prefix("/w/") => serve_match(s, app, sessions, request, path, client)?,
        (p, _) if let Some(path) = p.strip_prefix("/r/") => serve_room(s, app, sessions, request, path, client)?,
        (path, _) => serve_file(&mut s, app, path, request.gzip)?,
    }
    Ok(())
}

/// Back to the page `?return=` names, setting `cookie`.
fn back_with(s: &mut Out, app: &App, request: &Request, cookie: &str) -> std::io::Result<()> {
    respond_with(s, "303 See Other", &[("Location", &return_to(app, request)), ("Set-Cookie", cookie)], b"")
}

/// The body of a request, up to [`MAX_REQUEST`] bytes.
fn read_body(request: &Request) -> Result<Vec<u8>> {
    if request.length as u64 > MAX_REQUEST {
        bail!("over {MAX_REQUEST} bytes");
    }
    Ok(request.body.clone())
}

/// A JSON body read as `T`, or why not.
fn json_body<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T> {
    Ok(serde_json::from_slice(&read_body(request)?)?)
}

/// The marks from `?from=` on, and how many there are in all.
fn respond_marks(s: &mut Out, query: &str, marks: impl FnOnce(usize) -> (Vec<Mark>, usize)) -> std::io::Result<()> {
    let from = param(query, "from").and_then(|f| f.parse().ok()).unwrap_or(0);
    let (marks, all) = marks(from);
    respond(s, "200 OK", JSON, serde_json::json!({ "marks": marks, "all": all }).to_string().as_bytes())
}

/// `204` when `done` is, `400` with why not.
fn done(s: &mut Out, done: Result<()>) -> std::io::Result<()> {
    match done {
        Ok(()) => respond(s, "204 No Content", TEXT, b""),
        Err(e) => refuse(s, "400 Bad Request", &e),
    }
}

/// `POST /watch`: the front page's form.
fn post_watch(s: &mut Out, app: &App, sessions: &Sessions, request: &Request, client: IpAddr) -> Result<()> {
    let body = read_body(request)?;
    let form = String::from_utf8_lossy(&body);
    let line = param(&form, "connect").map_or_default(form_decode);
    let user = visitor::who(app, request).user;
    match watch(app, sessions, user.as_ref(), &line, client) {
        Ok(m) => app.redirect(s, "303 See Other", &format!("/w/{}/", m.id))?,
        Err(e) => {
            let status = if app.keys.is_some() && user.is_none() { "403 Forbidden" } else { "400 Bad Request" };
            respond(s, status, HTML, &app.page(&landing(app, sessions, request, client, &format!("{e:#}"))))?;
        }
    }
    Ok(())
}

/// The match `line` asks for: a connect string, or a demos.tf link. Behind a site, signed in.
fn watch(app: &App, sessions: &Sessions, user: Option<&token::User>, line: &str, client: IpAddr) -> Result<Arc<sessions::Session>> {
    if app.keys.is_some() && user.is_none() {
        anyhow::bail!("Sign in first");
    }
    let by = counted(user, None, client);
    let owner = user.map(|u| u.uid.as_str());
    match demostf_link(line) {
        Some(demo) => sessions.open_demo_as(by.as_deref(), demo, owner),
        None => sessions.open_as(by.as_deref(), line, owner, None),
    }
}

/// The demo of a demos.tf link.
fn demostf_link(line: &str) -> Option<u64> {
    let rest = line.trim().trim_start_matches("https://").trim_start_matches("www.").strip_prefix("demos.tf/")?;
    rest.trim_end_matches('/').parse().ok().filter(|d| !sessions::is_own_demo(*d))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The front page, with `error` if any.
fn landing(app: &App, sessions: &Sessions, request: &Request, client: IpAddr, error: &str) -> String {
    let signed = app.keys.is_none() || visitor::who(app, request).user.is_some();
    LANDING
        .replace("{mine}", &mine::mine(app, sessions, request, client))
        .replace("{signed}", if signed { "1" } else { "" })
        .replace("{max}", &(upload::MAX_DEMO >> 20).to_string())
        .replace("{base}", &app.base)
        .replace("{error}", &escape(error))
}

/// `text` for HTML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A form value: `+` is a space and `%XX` a byte.
fn form_decode(v: &str) -> String {
    let mut out = Vec::with_capacity(v.len());
    let mut bytes = v.bytes();
    while let Some(b) = bytes.next() {
        match b {
            b'+' => out.push(b' '),
            b'%' => {
                let hex: Vec<u8> = bytes.by_ref().take(2).collect();
                match std::str::from_utf8(&hex).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => out.push(byte),
                    None => out.extend_from_slice(&hex),
                }
            }
            _ => out.push(b),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests;
