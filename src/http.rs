//! The HTTP side: pages, files, event streams and starting matches.

use crate::hub::{SharedHub, subscribe};
use crate::room::{Control, Draw, ViewPost};
use crate::sessions::{Session, Sessions, Viewer};
use crate::token::Source;
use crate::{App, Matches, sessions, token};
use anyhow::{Result, anyhow, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::{BufRead, BufReader, Read, Take, Write};
use std::net::{IpAddr, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

mod assets;

pub(crate) use assets::app_version;
use assets::serve_file;

const INDEX: &str = include_str!("../index.html");
const LANDING: &str = include_str!("../landing.html");
const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const TEXT: &str = "text/plain";
const JSON: &str = "application/json";
const WEBP: &str = "image/webp";
const SVG: &str = "image/svg+xml";
const TTF: &str = "font/ttf";

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

/// The event stream's gzip level: most of the size win at little CPU.
const STREAM_GZIP: u32 = 5;

/// An event stream with nothing to send sends a comment this often, so proxies keep it open.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// After a restart (a deploy), browsers come back after this instead of their default 3 s.
const RECONNECT_AFTER: Duration = Duration::from_secs(1);

/// How long a browser keeps a file that does not change for a given build.
const CACHE_FOR: Duration = Duration::from_hours(24);

/// What the server needs of a request; the body, if any, is still to be read.
struct Request {
    method: String,
    path: String,
    query: String,
    length: usize,
    gzip: bool,
    /// The client's address as the proxy in front says it.
    forwarded: Option<IpAddr>,
    /// An `Authorization: Bearer` token.
    bearer: Option<String>,
    /// The `stvtf_guest` cookie: who the browser is, to the rooms.
    guest: Option<String>,
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
        forwarded: None,
        bearer: None,
        guest: None,
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
            // The proxy in front adds the address it saw last; earlier ones are the client's say-so.
            request.forwarded = value.rsplit(',').next().and_then(|a| a.trim().parse::<IpAddr>().ok());
        } else if name.eq_ignore_ascii_case("authorization") {
            request.bearer = value.trim().strip_prefix("Bearer ").map(|t| t.trim().to_string());
        } else if name.eq_ignore_ascii_case("cookie") {
            let guest = value.split(';').find_map(|c| c.trim().strip_prefix("stvtf_guest="));
            request.guest = guest.filter(|g| sessions::is_id(g)).map(str::to_string);
        } else if name.eq_ignore_ascii_case("accept-encoding") {
            request.gzip = value.split(',').any(|e| e.split(';').next().is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip")));
        }
    }
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
    respond_with(s, status, &[("Content-Type", ctype)], body)
}

/// A file that never changes for a given build, so the browser keeps it.
fn respond_cached(s: &mut TcpStream, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let max_age = format!("max-age={}", CACHE_FOR.as_secs());
    respond_with(s, "200 OK", &[("Content-Type", ctype), ("Cache-Control", &max_age)], body)
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
    let mut reader = BufReader::new(s.try_clone()?.take(MAX_REQUEST));
    let request = read_request(&mut reader)?;
    let peer = s.peer_addr()?.ip();
    let client = client_of(request.forwarded, peer);
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
            Some(_slot) => stream(s, hub, request.gzip, wants_detail(&request.query))?,
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
    let started = |s: &mut TcpStream, m: &Session| app.redirect(s, "303 See Other", &format!("/w/{}/", m.id));
    match (request.path.as_str(), request.method.as_str()) {
        (path @ ("/" | "/watch"), _) if app.keys.is_some() => front_door(&mut s, app, path)?,
        ("/matches", "GET") if let Some(keys) = &app.keys => list_matches(&mut s, keys, sessions, request)?,
        ("/start", _) => match start(app, sessions, &request.query) {
            // <stv-map> asks where the match is instead of following the redirect.
            Ok(m) if param(&request.query, "format") == Some("json") => {
                let page = format!("{}/w/{}/", app.base, m.id);
                respond(&mut s, "200 OK", JSON, serde_json::json!({ "page": page }).to_string().as_bytes())?;
            }
            Ok(m) => app.redirect(&mut s, "303 See Other", &format!("/w/{}/{}", m.id, start_view(&request.query)))?,
            Err(e) => respond(&mut s, "400 Bad Request", TEXT, format!("{e:#}").as_bytes())?,
        },
        ("/", "GET") => respond(&mut s, "200 OK", HTML, &app.page(&landing("")))?,
        ("/watch", "POST") if request.length as u64 > MAX_REQUEST => {
            respond(&mut s, "413 Content Too Large", TEXT, format!("over {MAX_REQUEST} bytes").as_bytes())?;
        }
        ("/watch", "POST") => {
            let body = read_body(reader, request)?;
            let form = String::from_utf8_lossy(&body);
            let line = param(&form, "connect").map(form_decode).unwrap_or_default();
            match sessions.open(&line, None, None) {
                Ok(m) => started(&mut s, &m)?,
                Err(e) => respond(&mut s, "400 Bad Request", HTML, &app.page(&landing(&format!("{e:#}"))))?,
            }
        }
        (p, _) if let Some(path) = p.strip_prefix("/w/") => serve_match(s, app, sessions, request, path, client)?,
        (p, _) if let Some(path) = p.strip_prefix("/r/") => serve_room(s, reader, app, request, path, client)?,
        (path, _) => serve_file(&mut s, app, path, request.gzip)?,
    }
    Ok(())
}

/// A match's page and event stream: `path` is what follows `/w/`.
fn serve_match(mut s: TcpStream, app: &App, sessions: &Sessions, request: &Request, path: &str, client: IpAddr) -> Result<()> {
    let (id, rest) = path.split_once('/').unwrap_or((path, ""));
    match (sessions.get(id), rest) {
        (None, _) if app.keys.is_some() => {
            respond(&mut s, "404 Not Found", TEXT, b"That match is no longer being watched; open it again from the site.")?;
        }
        (None, _) => {
            let gone = landing("That match is no longer being watched; paste its connect string to watch it again.");
            respond(&mut s, "404 Not Found", HTML, &app.page(&gone))?;
        }
        (Some(_), "") if !path.ends_with('/') => {
            app.redirect(&mut s, "308 Permanent Redirect", &format!("{}/", request.path))?;
        }
        (Some(_), rest) if is_page(rest) => respond(&mut s, "200 OK", HTML, &app.page(INDEX))?,
        (Some(m), "room") => open_room(&mut s, app, sessions, &m, request)?,
        (Some(m), "events") => match app.stream_slot(client) {
            Some(_slot) => {
                let _viewer = Viewer::new(m.clone());
                stream(s, &m.hub, request.gzip, wants_detail(&request.query))?;
            }
            None => too_many_streams(&mut s)?,
        },
        _ => not_found(&mut s)?,
    }
    Ok(())
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

/// The browser's guest cookie, and a `Set-Cookie` for a new one if it had none.
fn guest(app: &App, request: &Request) -> (String, Option<String>) {
    if let Some(g) = &request.guest {
        return (g.clone(), None);
    }
    let g = sessions::random_id();
    let secure = if app.keys.is_some() { "; Secure" } else { "" };
    let cookie = format!("stvtf_guest={g}; Path={}/; Max-Age=31536000; HttpOnly; SameSite=Lax{secure}", app.base);
    (g, Some(cookie))
}

/// A JSON body read as `T`, or why not.
fn json_body<T: serde::de::DeserializeOwned>(reader: &mut BufReader<Take<TcpStream>>, request: &Request) -> Result<T> {
    Ok(serde_json::from_slice(&read_body(reader, request)?)?)
}

/// `204` when `done` is, `400` with why not.
fn done(s: &mut TcpStream, done: Result<()>) -> std::io::Result<()> {
    match done {
        Ok(()) => respond(s, "204 No Content", TEXT, b""),
        Err(e) => respond(s, "400 Bad Request", TEXT, format!("{e:#}").as_bytes()),
    }
}

/// `GET` says whether a room can be opened on match `m`; `POST` opens one, owned by the browser.
fn open_room(s: &mut TcpStream, app: &App, sessions: &Sessions, m: &Session, request: &Request) -> Result<()> {
    if !m.recordable() {
        return Ok(respond(s, "409 Conflict", TEXT, b"Rooms play demos; this match has none (yet).")?);
    }
    if request.method != "POST" {
        return Ok(respond(s, "200 OK", JSON, b"{}")?);
    }
    if sessions.running() + app.rooms.count() >= sessions.max {
        return Ok(respond(s, "503 Service Unavailable", TEXT, b"This server is watching all it can.")?);
    }
    let recording = match m.recording() {
        Ok(r) => r,
        Err(e) => return Ok(respond(s, "409 Conflict", TEXT, format!("{e:#}").as_bytes())?),
    };
    let (guest, cookie) = guest(app, request);
    let room = app.rooms.open(recording, &guest);
    eprintln!("stvtf-web: room {} opened on match {}", &room.id[..8], &m.id[..m.id.len().min(8)]);
    let body = serde_json::json!({ "page": format!("{}/r/{}/", app.base, room.id) }).to_string();
    let mut headers = vec![("Content-Type", JSON)];
    if let Some(c) = &cookie {
        headers.push(("Set-Cookie", c));
    }
    Ok(respond_with(s, "201 Created", &headers, body.as_bytes())?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Name {
    name: String,
}

/// Person `person` made an admin, or not.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Admin {
    person: u32,
    admin: bool,
}

/// A room's page, event stream and controls: `path` is what follows `/r/`.
fn serve_room(
    mut s: TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    request: &Request,
    path: &str,
    client: IpAddr,
) -> Result<()> {
    let (id, rest) = path.split_once('/').unwrap_or((path, ""));
    let Some(room) = app.rooms.get(id) else {
        return Ok(respond(&mut s, "404 Not Found", TEXT, b"That room has closed.")?);
    };
    // Without a cookie (none yet, or refused), a page is someone new each time.
    let (guest, cookie) = guest(app, request);
    let admin = room.is_admin(&guest);
    match (rest, request.method.as_str()) {
        ("", _) if !path.ends_with('/') => app.redirect(&mut s, "308 Permanent Redirect", &format!("{}/", request.path))?,
        (rest, "GET") if is_page(rest) => {
            let mut headers = vec![("Content-Type", HTML)];
            if let Some(c) = &cookie {
                headers.push(("Set-Cookie", c));
            }
            respond_with(&mut s, "200 OK", &headers, &app.page(INDEX))?;
        }
        ("events", "GET") => match app.stream_slot(client) {
            Some(_slot) => {
                let name = param(&request.query, "name").map(form_decode);
                let listed = param(&request.query, "overlay") != Some("1");
                let _watching = room.join(&guest, name.as_deref(), listed);
                stream(s, &room.hub, request.gzip, wants_detail(&request.query))?;
            }
            None => too_many_streams(&mut s)?,
        },
        ("me", "GET") => {
            let (id, admin) = room.me(&guest);
            respond(&mut s, "200 OK", JSON, serde_json::json!({ "id": id, "admin": admin }).to_string().as_bytes())?;
        }
        ("name", "POST") => {
            let name = json_body::<Name>(reader, request).map(|n| room.rename(&guest, &n.name));
            done(&mut s, name)?;
        }
        ("control" | "draw" | "admin" | "view", "POST") if !admin => {
            respond(&mut s, "403 Forbidden", TEXT, b"Only the room's admins do that.")?;
        }
        ("view", "POST") => done(&mut s, json_body::<ViewPost>(reader, request).and_then(|v| room.set_view(&v, &guest)))?,
        ("draw", "POST") => done(&mut s, json_body::<Draw>(reader, request).and_then(|d| room.draw(&d, &guest)))?,
        ("control", "POST") => done(&mut s, json_body::<Control>(reader, request).and_then(|c| room.control(&c)))?,
        ("admin", "POST") => done(&mut s, json_body::<Admin>(reader, request).and_then(|a| room.set_admin(&guest, a.person, a.admin)))?,
        _ => not_found(&mut s)?,
    }
    Ok(())
}

/// Matches start only from the site's links: `/` sends visitors to it (`--home`), and the form to
/// watch any server is closed.
fn front_door(s: &mut TcpStream, app: &App, path: &str) -> std::io::Result<()> {
    match app.home.as_deref().filter(|_| path == "/") {
        Some(home) => redirect(s, "302 Found", home),
        None => respond(s, "403 Forbidden", TEXT, b"Start watching a match from the site that sent you here."),
    }
}

/// Only a proxy on our own network (kamal-proxy) is believed about who the client is.
fn on_our_network(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        v4 @ IpAddr::V4(_) => !sessions::is_public(v4),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
    }
}

/// Who is asking: what the proxy says if the connection is from one of ours, or else whoever connected.
fn client_of(forwarded: Option<IpAddr>, peer: IpAddr) -> IpAddr {
    client_key(forwarded.filter(|_| on_our_network(peer)).unwrap_or(peer))
}

/// What a client is counted by: its address, or for IPv6 its /64, which one user usually has all of.
fn client_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6((v6.to_bits() & !u128::from(u64::MAX)).into()),
        v4 @ IpAddr::V4(_) => v4,
    }
}

fn too_many_streams(s: &mut TcpStream) -> std::io::Result<()> {
    respond(s, "429 Too Many Requests", TEXT, b"too many matches open from your address")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Every match, for the site in front, with a list token.
fn list_matches(s: &mut TcpStream, keys: &token::Keys, sessions: &Sessions, request: &Request) -> Result<()> {
    if request.bearer.as_deref().is_none_or(|t| keys.open_list(t, unix_now()).is_err()) {
        respond_with(s, "401 Unauthorized", &[("Content-Type", TEXT), ("WWW-Authenticate", "Bearer")], b"needs a list token")?;
        return Ok(());
    }
    let body = serde_json::to_vec(&serde_json::json!({ "matches": sessions.list() }))?;
    respond_with(s, "200 OK", &[("Content-Type", JSON), ("Cache-Control", "no-store")], &body)?;
    Ok(())
}

/// The match a site's `/start?t=` token asks for.
fn start(app: &App, sessions: &Sessions, query: &str) -> Result<Arc<Session>> {
    let keys = app.keys.as_ref().ok_or_else(|| anyhow!("this server starts matches from its own page"))?;
    let token = param(query, "t").unwrap_or_default();
    let t = keys.open(token, unix_now())?;
    match t.source {
        Source::Connect(connect) => sessions.open(&connect, t.owner.as_deref(), t.reservation),
        Source::Demo(demo) => sessions.open_demo(demo, t.owner.as_deref()),
    }
}

/// Display options a start link can carry over to the page.
const VIEW_OPTIONS: [&str; 26] = [
    "names",
    "score",
    "panel",
    "rotate",
    "dot",
    "ui",
    "zoom",
    "side",
    "bg",
    "bgopacity",
    "health",
    "projectiles",
    "beams",
    "buildings",
    "shots",
    "deaths",
    "caps",
    "heals",
    "hits",
    "marks",
    "jumps",
    "pickups",
    "points",
    "flow",
    "loadout",
    "effects",
];

/// Where a start link lands below `/w/<id>/`: `view=overlay`, `delay=<s>` and the display options
/// become `overlay/4.5?names=1`. Anything else, or a delay out of range, is left out.
fn start_view(query: &str) -> String {
    let overlay = param(query, "view") == Some("overlay");
    let delay = param(query, "delay").unwrap_or_default();
    let mut path = match (overlay, delay) {
        (true, "") => "overlay".to_string(),
        (true, d) => format!("overlay/{d}"),
        (false, d) => d.to_string(),
    };
    if !is_page(&path) {
        path = if overlay { "overlay".into() } else { String::new() };
    }
    let options: Vec<String> = VIEW_OPTIONS
        .iter()
        .filter_map(|k| {
            param(query, k)
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
                .map(|v| format!("{k}={v}"))
        })
        .collect();
    if !options.is_empty() {
        path = format!("{path}?{}", options.join("&"));
    }
    path
}

/// The page or its overlay, either with a delay in seconds after it: `5`, `overlay/4.5`.
fn is_page(rest: &str) -> bool {
    let delay =
        |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit() || b == b'.') && d.parse::<f64>().is_ok_and(|s| s <= MAX_DELAY_S);
    match rest.split_once('/') {
        None => matches!(rest, "" | "overlay") || delay(rest),
        Some(("overlay", d)) => delay(d),
        _ => false,
    }
}

fn wants_detail(query: &str) -> bool {
    param(query, "detail") == Some("1")
}

/// A match's events to one browser, until it goes away.
/// Snapshots are about 17 a second of very similar JSON: gzip, flushed after each batch so nothing
/// waits, brings a viewer from about 95 KB/s down to a few.
fn stream(mut s: TcpStream, hub: &SharedHub, gzip: bool, detail: bool) -> Result<()> {
    let mut headers = vec![("Content-Type", "text/event-stream"), ("Cache-Control", "no-cache"), ("X-Accel-Buffering", "no")];
    if gzip {
        headers.push(("Content-Encoding", "gzip"));
    }
    write_head(&mut s, "200 OK", &headers)?;
    let mut out: Box<dyn Write> = if gzip { Box::new(GzEncoder::new(s, Compression::new(STREAM_GZIP))) } else { Box::new(s) };
    write!(out, "retry: {}\n\n", RECONNECT_AFTER.as_millis())?;
    let rx = subscribe(hub, detail);
    loop {
        match rx.recv_timeout(KEEPALIVE) {
            Ok(line) => {
                write!(out, "data: {line}\n\n")?;
                // Whatever else is already queued goes out in the same flush.
                for line in rx.try_iter() {
                    write!(out, "data: {line}\n\n")?;
                }
            }
            Err(RecvTimeoutError::Timeout) => write!(out, ": keepalive\n\n")?,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        out.flush()?;
    }
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
mod tests {
    use super::assets::APP;
    use super::*;
    use crate::hub::{SharedHub, publish};
    use crate::testing::{TempDir, app, server};
    use crate::{App, Matches, TcpListener, token};
    use flate2::read::GzDecoder;
    use std::path::PathBuf;
    use std::time::Instant;
    use stvtf::Line;
    use stvtf::output::{self, PlayerId};

    #[test]
    fn start_links_carry_every_display_toggle() {
        let toggles = "names=0&score=1&panel=0.5&rotate=auto&dot=14&ui=1.5&zoom=map&side=0&bg=geometry&bgopacity=0.4&health=0&projectiles=0&beams=0&buildings=0&shots=0&deaths=0\
                       &caps=0&heals=1&hits=1&marks=1&jumps=1&pickups=1&points=0&flow=1&loadout=0&effects=0";
        assert_eq!(start_view(&format!("view=overlay&{toggles}&x=y")), format!("overlay?{toggles}"));
    }

    /// Runs `serve` on one connection, with `send` written by the client; returns serve's result and the response.
    fn request_to(app: &App, send: &[u8]) -> (Result<()>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        client.write_all(send).unwrap();
        let result = serve(server, app);
        let mut response = String::new();
        let _ = client.read_to_string(&mut response);
        (result, response)
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
    fn a_demo_opens_a_room_for_its_owner() {
        let (app, _dir) = demo_app();
        assert!(get(&app, "/w/demo/room", "").starts_with("HTTP/1.1 200"));
        let (page, cookie) = open_room(&app);
        assert!(page.starts_with("/r/") && page.ends_with('/'), "{page}");
        assert!(get(&app, &page, "").contains("<canvas"));
        assert!(get(&app, &format!("{page}me"), &cookie).ends_with(r#"{"admin":true,"id":1}"#));
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
    fn admins_make_others_admin() {
        let (app, _dir) = demo_app();
        let (page, owner) = open_room(&app);
        let viewer = format!("stvtf_guest={}", "1".repeat(32));
        let me = get(&app, &format!("{page}me"), &viewer);
        assert!(me.ends_with(r#"{"admin":false,"id":2}"#), "{me}");
        assert!(post(&app, &format!("{page}name"), &viewer, r#"{"name":"caster"}"#).starts_with("HTTP/1.1 204"));
        let pause = r#"{"paused":true}"#;
        assert!(post(&app, &format!("{page}control"), &viewer, pause).starts_with("HTTP/1.1 403"));
        let promote = r#"{"person":2,"admin":true}"#;
        assert!(post(&app, &format!("{page}admin"), &viewer, promote).starts_with("HTTP/1.1 403"));
        assert!(post(&app, &format!("{page}admin"), &owner, promote).starts_with("HTTP/1.1 204"));
        assert!(post(&app, &format!("{page}control"), &viewer, pause).starts_with("HTTP/1.1 204"));
        assert!(post(&app, &format!("{page}view"), &viewer, r#"{"mode":"auto","take":true}"#).starts_with("HTTP/1.1 204"));
        assert!(post(&app, &format!("{page}view"), &viewer, r#"{"mode":"spin"}"#).starts_with("HTTP/1.1 400"));
        let demote_owner = r#"{"person":1,"admin":false}"#;
        assert!(post(&app, &format!("{page}admin"), &viewer, demote_owner).starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn a_live_match_offers_no_room() {
        let app = app(Matches::Many(Sessions::new(false, 2, 20.0)));
        let Matches::Many(sessions) = &app.matches else { unreachable!() };
        let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
        assert!(get(&app, &format!("/w/{}/room", m.id), "").starts_with("HTTP/1.1 409"));
        assert!(post(&app, &format!("/w/{}/room", m.id), "", "").starts_with("HTTP/1.1 409"));
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
        let t =
            token::seal(&key, &format!(r#"{{"connect":"connect 192.0.2.1:27020; password \"tv\"","owner":"u1","exp":{exp}}}"#), [3; 12]);
        let response = request_to(&app, format!("GET /start?t={t} HTTP/1.1\r\n\r\n").as_bytes()).1;
        let location = response.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap_or_default().to_string();
        assert!(location.starts_with("/live/w/") && location.ends_with('/'), "{response}");
        let page = request_to(&app, format!("GET {} HTTP/1.1\r\n\r\n", &location[5..]).as_bytes()).1;
        assert!(page.contains(r#"<base href="/live/">"#), "{page}");
        let overlay =
            request_to(&app, format!("GET /start?t={t}&view=overlay&delay=4.5&names=1&x=y&score=%0d HTTP/1.1\r\n\r\n").as_bytes()).1;
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

    #[test]
    fn serves_the_page_and_its_modules_under_the_version() {
        let a = app(Matches::One(SharedHub::default()));
        let get = |path: &str| request_to(&a, format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).1;
        let page = get("/");
        let v = app_version();
        assert!(page.contains(&format!("\"app/{v}/style.css\"")) && page.contains(&format!("\"app/{v}/main.js\"")), "{page}");
        let mut imported = String::new();
        for (name, ctype, _) in APP {
            let file = get(&format!("/app/{v}/{name}"));
            assert!(file.starts_with("HTTP/1.1 200 OK") && file.contains("max-age=86400"), "{name}");
            assert!(file.contains(&format!("Content-Type: {ctype}")), "{name}");
            imported += &file;
        }
        for (name, ctype, _) in APP {
            // The page's entry, and the one /embed/v1/stv-map.js loads for other sites.
            let entry = name == "main.js" || name == "stv-map.js";
            assert!(ctype != JS || entry || imported.contains(&format!("from \"./{name}\"")), "nothing imports {name}");
        }
        // And everything imported is served: a module missing from APP would 404 in the browser.
        for import in imported.split("from \"./").skip(1).map(|r| r.split('"').next().unwrap()) {
            assert!(APP.iter().any(|(name, ..)| *name == import), "{import} is imported but not in APP");
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
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            write!(client, "GET /{} HTTP/1.1\r\nHost: x\r\n\r\n", path.join("/")).unwrap();
            serve(listener.accept().unwrap().0, &a).unwrap();
            let mut font = Vec::new();
            client.read_to_end(&mut font).unwrap();
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
        for bad in [
            "/backpack/../x.webp",
            "/backpack/a/b.webp",
            "/backpack/missing.webp",
            "/backpack/scattergun.png",
            "/backpack/.webp",
            "/backpack/",
        ] {
            assert!(get(bad).starts_with("HTTP/1.1 404"), "{bad}");
        }
    }

    #[test]
    fn map_geometry_is_gzipped_for_browsers_that_take_it() {
        let dir = TempDir::new("mapgzip");
        let json = format!(r#"{{"floors":[{}]}}"#, r#"{"z":0,"p":[0,0,1,0,1,1],"k":"grass"},"#.repeat(200).trim_end_matches(','));
        std::fs::write(dir.join("cp_test.v7.json"), &json).unwrap();
        let mut a = app(Matches::One(SharedHub::default()));
        a.cache = dir.to_path_buf();
        let get = |accept: &str| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            write!(client, "GET /map/cp_test HTTP/1.1\r\nHost: x\r\n{accept}\r\n").unwrap();
            serve(listener.accept().unwrap().0, &a).unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).unwrap();
            let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            (String::from_utf8(response[..split].to_vec()).unwrap(), response[split + 4..].to_vec())
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
        assert!(dir.join("cp_test.v7.json.gz").exists());
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
        Line::Heal(output::Heal {
            tick: 1,
            patient: PlayerId(4),
            healer: Some(PlayerId(3)),
            amount: 20,
            source: output::HealSource::Medigun,
        })
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
        let claimed = Some(ip("203.0.113.9"));
        assert_eq!(client_of(claimed, ip("172.18.0.2")), ip("203.0.113.9"));
        assert_eq!(client_of(claimed, ip("141.94.96.119")), ip("141.94.96.119"), "a stranger cannot choose whose streams count");
        assert_eq!(client_of(None, ip("172.18.0.2")), ip("172.18.0.2"));
    }

    #[test]
    fn the_client_is_the_last_forwarded_address() {
        let forwarded = |header: &str| {
            let send = format!("GET / HTTP/1.1\r\n{header}\r\n\r\n");
            read_request(&mut send.as_bytes()).unwrap().forwarded.map(|ip| ip.to_string())
        };
        assert_eq!(forwarded("X-Forwarded-For: 203.0.113.9").as_deref(), Some("203.0.113.9"));
        assert_eq!(forwarded("x-forwarded-for: 198.51.100.7, 203.0.113.9").as_deref(), Some("203.0.113.9"));
        assert_eq!(forwarded("X-Forwarded-For: 2001:db8::1").as_deref(), Some("2001:db8::1"));
        assert_eq!(forwarded("X-Forwarded-For: 203.0.113.9, nonsense"), None);
        assert_eq!(forwarded("Host: x"), None);
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        write!(client, "GET {path} HTTP/1.1\r\n\r\n").unwrap();
        serve(server, app).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        let end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        String::from_utf8_lossy(&response[..end]).into_owned()
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
        std::fs::write(cache.join("cp_x.v7.json"), b"{}").unwrap();
        let mut a = app(Matches::One(SharedHub::default()));
        a.cache = cache.to_path_buf();

        let list = head_of(&a, "/overview/boundaries.json");
        assert!(list.contains("Content-Type: application/json") && list.contains("max-age=86400"), "{list}");
        let image = head_of(&a, "/overview/cp_x.webp");
        assert!(image.contains("Content-Type: image/webp") && image.contains("max-age=86400"), "{image}");
        let map = head_of(&a, "/map/cp_x");
        assert!(
            map.contains("Content-Type: application/json") && map.contains("Vary: Accept-Encoding") && !map.contains("max-age"),
            "{map}"
        );
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
}
