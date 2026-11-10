//! The HTTP side: pages, files, event streams and starting matches.

use crate::matches::hub::SharedHub;
use crate::matches::recording::Mark;
use crate::matches::sessions;
use crate::matches::sessions::Sessions;
use crate::{App, Matches};
use anyhow::{Result, bail};
use std::io::{BufRead, BufReader, Read, Take, Write};
use std::net::{IpAddr, TcpStream};
use std::time::Duration;

mod assets;
mod client;
mod events;
mod matches;
mod rooms;
pub(crate) mod token;
mod visitor;

pub(crate) use assets::app_version;
use assets::serve_file;
use client::{client_of, counted};
use events::{stream, too_many_streams, wants_detail, wants_voice};
use matches::{is_page, list_matches, serve_match, start, start_view};
use rooms::serve_room;
use visitor::{origin, return_to, set_cookie, url_encode};

const INDEX: &str = include_str!("../app/pages/index.html");
const LANDING: &str = include_str!("../app/pages/landing.html");
const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const TEXT: &str = "text/plain";
const JSON: &str = "application/json";
const WEBP: &str = "image/webp";
const SVG: &str = "image/svg+xml";
const TTF: &str = "font/ttf";

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

/// A request must arrive in this time, and a response (or an event) be taken in this time.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a browser keeps a file that does not change for a given build.
const CACHE_FOR: Duration = Duration::from_hours(24);

/// What the server needs of a request; the body, if any, is still to be read.
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
}

/// The request line and headers.
fn read_request(reader: &mut impl BufRead) -> Result<Request> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or("GET").to_string();
    let target = words.next().unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut request = Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        length: 0,
        gzip: false,
        forwarded: Vec::new(),
        bearer: None,
        guest: None,
        user: None,
    };
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            bail!("request incomplete or over {MAX_REQUEST} bytes");
        }
        if header == "\r\n" {
            return Ok(request);
        }
        let Some((name, value)) = header.split_once(':') else { continue };
        if name.eq_ignore_ascii_case("content-length") {
            request.length = value.trim().parse().unwrap_or(0);
        } else if name.eq_ignore_ascii_case("x-forwarded-for") {
            request.forwarded = value.split(',').map(|a| a.trim().parse::<IpAddr>()).collect::<Result<_, _>>().unwrap_or_default();
        } else if name.eq_ignore_ascii_case("authorization") {
            request.bearer = value.trim().strip_prefix("Bearer ").map(|t| t.trim().to_string());
        } else if name.eq_ignore_ascii_case("cookie") {
            request.guest = cookie(value, GUEST_COOKIE).filter(|g| sessions::is_id(g)).map(str::to_string);
            request.user = cookie(value, USER_COOKIE).map(str::to_string);
        } else if name.eq_ignore_ascii_case("accept-encoding") {
            request.gzip = value.split(',').any(|e| e.split(';').next().is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip")));
        }
    }
}

/// Cookie `name`'s value in a `Cookie` header.
fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|c| c.trim().strip_prefix(name)?.strip_prefix('='))
}

/// A query string's or form's value for `name`, undecoded.
fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v))
}

/// The status line and headers; every response says `nosniff` and may be read by any site.
fn write_head(s: &mut impl Write, status: &str, headers: &[(&str, &str)]) -> std::io::Result<()> {
    write!(s, "HTTP/1.1 {status}\r\n")?;
    for (name, value) in headers {
        write!(s, "{name}: {value}\r\n")?;
    }
    // Everything here is public and nothing uses cookies: <stv-map> on other sites reads it all.
    write!(s, "Access-Control-Allow-Origin: *\r\nX-Content-Type-Options: nosniff\r\n\r\n")
}

/// A whole response, after which the connection closes.
fn respond_with(s: &mut TcpStream, status: &str, headers: &[(&str, &str)], body: &[u8]) -> std::io::Result<()> {
    let length = body.len().to_string();
    let mut all = headers.to_vec();
    all.extend([("Content-Length", length.as_str()), ("Connection", "close")]);
    write_head(s, status, &all)?;
    s.write_all(body)
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    // Pages name the build's files: no-cache, so a reload gets a new build.
    if ctype == HTML {
        return respond_with(s, status, &[("Content-Type", ctype), ("Cache-Control", "no-cache")], body);
    }
    respond_with(s, status, &[("Content-Type", ctype)], body)
}

/// A file that never changes for a given build, so the browser keeps it.
fn respond_cached(s: &mut TcpStream, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let max_age = format!("max-age={}", CACHE_FOR.as_secs());
    respond_with(s, "200 OK", &[("Content-Type", ctype), ("Cache-Control", &max_age)], body)
}

/// `status`, with why as text.
fn refuse(s: &mut TcpStream, status: &str, why: &anyhow::Error) -> std::io::Result<()> {
    respond(s, status, TEXT, format!("{why:#}").as_bytes())
}

fn not_found(s: &mut TcpStream) -> std::io::Result<()> {
    respond(s, "404 Not Found", TEXT, b"not found")
}

pub(crate) fn redirect(s: &mut TcpStream, status: &str, location: &str) -> std::io::Result<()> {
    respond_with(s, status, &[("Location", location)], b"")
}

pub(crate) fn serve(mut s: TcpStream, app: &App) -> Result<()> {
    // A client that sends nothing, or stops reading `/events`, does not keep its thread forever.
    s.set_read_timeout(Some(READ_TIMEOUT))?;
    s.set_write_timeout(Some(WRITE_TIMEOUT))?;
    // Every write is a whole response or a flushed batch of events: no waiting to fill a packet
    // (with delayed acks, up to 40 ms an event).
    s.set_nodelay(true)?;
    let mut reader = BufReader::new(s.try_clone()?.take(MAX_REQUEST));
    let request = read_request(&mut reader)?;
    let peer = s.peer_addr()?.ip();
    let client = client_of(&request.forwarded, peer);
    match &app.matches {
        _ if request.path == "/up" => respond(&mut s, "200 OK", TEXT, b"ok")?,
        Matches::One(hub) => serve_one(s, app, hub, &request, client)?,
        Matches::Many(sessions) => serve_many(s, &mut reader, app, sessions, &request, client)?,
    }
    Ok(())
}

/// The one match a server given a server address watches, at `/`.
fn serve_one(mut s: TcpStream, app: &App, hub: &SharedHub, request: &Request, client: IpAddr) -> Result<()> {
    match request.path.as_str() {
        p if p == "/index.html" || p.strip_prefix('/').is_some_and(is_page) => respond(&mut s, "200 OK", HTML, &app.page(INDEX))?,
        "/events" => match app.stream_slot(client) {
            Some(_slot) => stream(s, hub, request.gzip, wants_detail(&request.query), wants_voice(&request.query))?,
            None => too_many_streams(&mut s)?,
        },
        path => serve_file(&mut s, app, path, request.gzip)?,
    }
    Ok(())
}

/// Any number of matches, each at `/w/<id>/`, and the ways to start one.
fn serve_many(
    mut s: TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    sessions: &Sessions,
    request: &Request,
    client: IpAddr,
) -> Result<()> {
    match (request.path.as_str(), request.method.as_str()) {
        (path @ ("/" | "/watch"), _) if app.keys.is_some() => front_door(&mut s, app, path)?,
        ("/matches", "GET") if let Some(keys) = &app.keys => list_matches(&mut s, keys, sessions, request)?,
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
        ("/start", _) => match start(app, sessions, request, client) {
            // <stv-map> asks where the match is instead of following the redirect.
            Ok(m) if param(&request.query, "format") == Some("json") => {
                let page = format!("{}/w/{}/", app.base, m.id);
                respond(&mut s, "200 OK", JSON, serde_json::json!({ "page": page }).to_string().as_bytes())?;
            }
            Ok(m) => app.redirect(&mut s, "303 See Other", &format!("/w/{}/{}", m.id, start_view(&request.query)))?,
            Err(e) => refuse(&mut s, "400 Bad Request", &e)?,
        },
        ("/", "GET") => respond(&mut s, "200 OK", HTML, &app.page(&landing("")))?,
        ("/watch", "POST") if request.length as u64 > MAX_REQUEST => {
            respond(&mut s, "413 Content Too Large", TEXT, format!("over {MAX_REQUEST} bytes").as_bytes())?;
        }
        ("/watch", "POST") => {
            let body = read_body(reader, request)?;
            let form = String::from_utf8_lossy(&body);
            let line = param(&form, "connect").map(form_decode).unwrap_or_default();
            match sessions.open_as(counted(None, None, client).as_deref(), &line, None, None) {
                Ok(m) => app.redirect(&mut s, "303 See Other", &format!("/w/{}/", m.id))?,
                Err(e) => respond(&mut s, "400 Bad Request", HTML, &app.page(&landing(&format!("{e:#}"))))?,
            }
        }
        (p, _) if let Some(path) = p.strip_prefix("/w/") => serve_match(s, reader, app, sessions, request, path, client)?,
        (p, _) if let Some(path) = p.strip_prefix("/r/") => serve_room(s, reader, app, sessions, request, path, client)?,
        (path, _) => serve_file(&mut s, app, path, request.gzip)?,
    }
    Ok(())
}

/// Back to the page `?return=` names, setting `cookie`.
fn back_with(s: &mut TcpStream, app: &App, request: &Request, cookie: &str) -> std::io::Result<()> {
    respond_with(s, "303 See Other", &[("Location", &return_to(app, request)), ("Set-Cookie", cookie)], b"")
}

/// The body of a request, up to [`MAX_REQUEST`] bytes.
fn read_body(reader: &mut BufReader<Take<TcpStream>>, request: &Request) -> Result<Vec<u8>> {
    if request.length as u64 > MAX_REQUEST {
        bail!("over {MAX_REQUEST} bytes");
    }
    // The body has an allowance of its own: the headers may have used up most of theirs.
    let buffered = reader.buffer().len() as u64;
    reader.get_mut().set_limit((request.length as u64).saturating_sub(buffered));
    let mut body = vec![0; request.length];
    reader.read_exact(&mut body)?;
    Ok(body)
}

/// A JSON body read as `T`, or why not.
fn json_body<T: serde::de::DeserializeOwned>(reader: &mut BufReader<Take<TcpStream>>, request: &Request) -> Result<T> {
    Ok(serde_json::from_slice(&read_body(reader, request)?)?)
}

/// The marks from `?from=` on, and how many there are in all.
fn respond_marks(s: &mut TcpStream, query: &str, marks: impl FnOnce(usize) -> (Vec<Mark>, usize)) -> std::io::Result<()> {
    let from = param(query, "from").and_then(|f| f.parse().ok()).unwrap_or(0);
    let (marks, all) = marks(from);
    respond(s, "200 OK", JSON, serde_json::json!({ "marks": marks, "all": all }).to_string().as_bytes())
}

/// `204` when `done` is, `400` with why not.
fn done(s: &mut TcpStream, done: Result<()>) -> std::io::Result<()> {
    match done {
        Ok(()) => respond(s, "204 No Content", TEXT, b""),
        Err(e) => refuse(s, "400 Bad Request", &e),
    }
}

/// Matches start only from the site's links: `/` sends visitors to it (`--home`), and the form to
/// watch any server is closed.
fn front_door(s: &mut TcpStream, app: &App, path: &str) -> std::io::Result<()> {
    match app.home.as_deref().filter(|_| path == "/") {
        Some(home) => redirect(s, "302 Found", home),
        None => respond(s, "403 Forbidden", TEXT, b"Start watching a match from the site that sent you here."),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The start page, with an error from the last attempt if there was one.
fn landing(error: &str) -> String {
    let escaped = error.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    LANDING.replace("{error}", &escaped)
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
