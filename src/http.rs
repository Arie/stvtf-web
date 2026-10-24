//! The HTTP side: pages, files, event streams and starting matches.

use crate::cache::{map_geometry, overview};
use crate::hub::{SharedHub, subscribe};
use crate::sessions::{Session, Sessions, Viewer};
use crate::{App, Matches, is_name, lock, sessions};
use anyhow::{Result, anyhow, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::{BufRead, BufReader, Read, Take, Write};
use std::net::{IpAddr, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

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

/// The page's stylesheet and modules, served at `/app/<version>/`: `main.js` imports the rest.
const APP: [(&str, &str, &[u8]); 13] = [
    ("style.css", CSS, include_bytes!("../app/style.css")),
    ("core.js", JS, include_bytes!("../app/core.js")),
    ("layers.js", JS, include_bytes!("../app/layers.js")),
    ("stream.js", JS, include_bytes!("../app/stream.js")),
    ("view.js", JS, include_bytes!("../app/view.js")),
    ("input.js", JS, include_bytes!("../app/input.js")),
    ("events.js", JS, include_bytes!("../app/events.js")),
    ("feed.js", JS, include_bytes!("../app/feed.js")),
    ("objectives.js", JS, include_bytes!("../app/objectives.js")),
    ("overkill.js", JS, include_bytes!("../app/overkill.js")),
    ("players.js", JS, include_bytes!("../app/players.js")),
    ("hud.js", JS, include_bytes!("../app/hud.js")),
    ("main.js", JS, include_bytes!("../app/main.js")),
];
const ICONS: [(&str, &[u8]); 9] = [
    ("scout", include_bytes!("../icons/scout.svg")),
    ("soldier", include_bytes!("../icons/soldier.svg")),
    ("pyro", include_bytes!("../icons/pyro.svg")),
    ("demoman", include_bytes!("../icons/demoman.svg")),
    ("heavy", include_bytes!("../icons/heavy.svg")),
    ("engineer", include_bytes!("../icons/engineer.svg")),
    ("medic", include_bytes!("../icons/medic.svg")),
    ("sniper", include_bytes!("../icons/sniper.svg")),
    ("spy", include_bytes!("../icons/spy.svg")),
];

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

/// FNV-1a, 64 bits.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// What the server needs of a request; the body, if any, is still to be read.
struct Request {
    method: String,
    path: String,
    query: String,
    length: usize,
    gzip: bool,
    /// The client's address as the proxy in front says it.
    forwarded: Option<IpAddr>,
}

/// The request line and headers.
fn read_request(reader: &mut impl BufRead) -> Result<Request> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or("GET").to_string();
    let target = words.next().unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut request = Request { method, path: path.to_string(), query: query.to_string(), length: 0, gzip: false, forwarded: None };
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
        } else if name.eq_ignore_ascii_case("accept-encoding") {
            request.gzip = value.split(',').any(|e| e.split(';').next().is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip")));
        }
    }
}

/// A query string's or form's value for `name`, undecoded.
fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v))
}

/// The status line and headers; every response says `nosniff`.
fn write_head(s: &mut impl Write, status: &str, headers: &[(&str, &str)]) -> std::io::Result<()> {
    write!(s, "HTTP/1.1 {status}\r\n")?;
    for (name, value) in headers {
        write!(s, "{name}: {value}\r\n")?;
    }
    write!(s, "X-Content-Type-Options: nosniff\r\n\r\n")
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
    let client = client_key(request.forwarded.filter(|_| on_our_network(peer)).unwrap_or(peer));
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
        path => serve_file(&mut s, app, path)?,
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
        ("/start", _) => match start(app, sessions, &request.query) {
            Ok(m) => started(&mut s, &m)?,
            Err(e) => respond(&mut s, "400 Bad Request", TEXT, format!("{e:#}").as_bytes())?,
        },
        ("/", "GET") => respond(&mut s, "200 OK", HTML, &app.page(&landing("")))?,
        ("/watch", "POST") if request.length as u64 > MAX_REQUEST => {
            respond(&mut s, "413 Content Too Large", TEXT, format!("over {MAX_REQUEST} bytes").as_bytes())?;
        }
        ("/watch", "POST") => {
            // The body has an allowance of its own: the headers may have used up most of theirs.
            let buffered = reader.buffer().len() as u64;
            reader.get_mut().set_limit((request.length as u64).saturating_sub(buffered));
            let mut body = vec![0; request.length];
            reader.read_exact(&mut body)?;
            let form = String::from_utf8_lossy(&body);
            let line = param(&form, "connect").map(form_decode).unwrap_or_default();
            match sessions.open(&line, None) {
                Ok(m) => started(&mut s, &m)?,
                Err(e) => respond(&mut s, "400 Bad Request", HTML, &app.page(&landing(&format!("{e:#}"))))?,
            }
        }
        (p, _) if let Some(path) = p.strip_prefix("/w/") => serve_match(s, app, sessions, request, path, client)?,
        (path, _) => serve_file(&mut s, app, path)?,
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

/// The page's files, pictures and maps.
fn serve_file(s: &mut TcpStream, app: &App, path: &str) -> std::io::Result<()> {
    match path {
        "/fonts/RobotoCondensed-Regular.ttf" => respond_cached(s, TTF, include_bytes!("../fonts/RobotoCondensed-Regular.ttf")),
        "/fonts/RobotoCondensed-Bold.ttf" => respond_cached(s, TTF, include_bytes!("../fonts/RobotoCondensed-Bold.ttf")),
        "/killicons.css" => respond(s, "200 OK", CSS, include_bytes!("../killicons/killicons.css")),
        "/killicons.webp" => respond(s, "200 OK", WEBP, include_bytes!("../killicons/killicons.webp")),
        p if let Some(file) = p.strip_prefix("/icons/") => match ICONS.iter().find(|(name, _)| file.strip_suffix(".svg") == Some(name)) {
            Some((_, svg)) => respond(s, "200 OK", SVG, svg),
            None => not_found(s),
        },
        p if let Some(file) = p.strip_prefix("/app/") => {
            match APP.iter().find(|(name, ..)| file.split_once('/').is_some_and(|(_, n)| n == *name)) {
                Some((_, ctype, body)) => respond_cached(s, ctype, body),
                None => not_found(s),
            }
        }
        p if let Some(file) = p.strip_prefix("/backpack/") => serve_backpack(s, app, file),
        p if p.starts_with("/overview/") || p.starts_with("/map/") => serve_cached(s, app, p),
        _ => not_found(s),
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

/// Only a proxy on our own network (kamal-proxy) is believed about who the client is.
fn on_our_network(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        v4 @ IpAddr::V4(_) => !sessions::is_public(v4),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
    }
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

/// A map's geometry, or a demos.tf overview, from the cache (fetched and made on a miss).
fn serve_cached(s: &mut TcpStream, app: &App, p: &str) -> std::io::Result<()> {
    let result = if let Some(file) = p.strip_prefix("/overview/") {
        let _one_at_a_time = lock(&app.overviews);
        overview(&app.cache, file)
    } else if let Some(name) = p.strip_prefix("/map/") {
        // Only a map being watched is built, so only those are locked: any other is read from the
        // cache, which is written whole or not at all.
        let fetch = app.on_map(name);
        let map_lock = fetch.then(|| app.map_lock(name));
        let _one_at_a_time = map_lock.as_deref().map(lock);
        map_geometry(&app.cache, name, fetch)
    } else {
        return not_found(s);
    };
    match result {
        Ok(body) if p.starts_with("/map/") => respond(s, "200 OK", JSON, &body),
        Ok(body) if p == "/overview/boundaries.json" => respond_cached(s, JSON, &body),
        Ok(body) => respond_cached(s, WEBP, &body),
        Err(e) => respond(s, "404 Not Found", TEXT, e.to_string().as_bytes()),
    }
}

/// A backpack picture or `items.json`; the name is checked so it cannot leave the directory.
fn serve_backpack(s: &mut TcpStream, app: &App, file: &str) -> std::io::Result<()> {
    let found = match file.rsplit_once('.') {
        Some((stem, "webp")) if is_name(stem, b"_") => Some(WEBP),
        Some(("items", "json")) => Some(JSON),
        _ => None,
    };
    match found.and_then(|ctype| Some((ctype, std::fs::read(app.backpack.join(file)).ok()?))) {
        Some((ctype, body)) => respond_cached(s, ctype, &body),
        None => not_found(s),
    }
}

/// Changes whenever a page file does, so browsers holding `/app/` files for a day fetch new ones after a deploy;
/// the modules' relative imports carry it along.
pub(crate) fn app_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let hash = APP.iter().flat_map(|(.., body)| body.iter()).fold(FNV_OFFSET_BASIS, |h, b| (h ^ u64::from(*b)).wrapping_mul(FNV_PRIME));
        format!("{hash:016x}")
    })
}

/// The match a site's `/start?t=` token asks for.
fn start(app: &App, sessions: &Sessions, query: &str) -> Result<Arc<Session>> {
    let keys = app.keys.as_ref().ok_or_else(|| anyhow!("this server starts matches from its own page"))?;
    let token = param(query, "t").unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let t = keys.open(token, now)?;
    sessions.open(&t.connect, t.owner.as_deref())
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
    use super::*;
    use crate::hub::SharedHub;
    use crate::testing::TempDir;
    use crate::testing::app;
    use crate::{App, Matches, TcpListener, token};

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
        let exp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 60;
        let t =
            token::seal(&key, &format!(r#"{{"connect":"connect 192.0.2.1:27020; password \"tv\"","owner":"u1","exp":{exp}}}"#), [3; 12]);
        let response = request_to(&app, format!("GET /start?t={t} HTTP/1.1\r\n\r\n").as_bytes()).1;
        let location = response.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap_or_default().to_string();
        assert!(location.starts_with("/live/w/") && location.ends_with('/'), "{response}");
        let page = request_to(&app, format!("GET {} HTTP/1.1\r\n\r\n", &location[5..]).as_bytes()).1;
        assert!(page.contains(r#"<base href="/live/">"#), "{page}");
        assert!(request_to(&app, b"GET /start?t=bogus HTTP/1.1\r\n\r\n").1.starts_with("HTTP/1.1 400"));
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
            assert!(ctype != JS || name == "main.js" || imported.contains(&format!("from \"./{name}\"")), "nothing imports {name}");
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
    fn clients_are_told_apart_by_address_or_ipv6_prefix() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(on_our_network(ip("172.18.0.2")) && on_our_network(ip("::1")) && on_our_network(ip("fd00::1")));
        assert!(!on_our_network(ip("141.94.96.119")) && !on_our_network(ip("2001:db8::1")) && !on_our_network(ip("::ffff:141.94.96.119")));
        assert_eq!(client_key(ip("2001:db8:1:2:aaaa::1")), client_key(ip("2001:db8:1:2:bbbb::2")));
        assert_ne!(client_key(ip("2001:db8:1:2::1")), client_key(ip("2001:db8:1:3::1")));
        assert_eq!(client_key(ip("::ffff:203.0.113.1")), ip("203.0.113.1"));
    }
}
