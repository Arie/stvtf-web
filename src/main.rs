//! Live top-down web viewer for a TF2 match, fed by stvtf.
//!
//! Given a server, one `SourceTV` connection fanned out to every browser over Server-Sent
//! Events; without one, a page to start watching any server, each match at its own link.
//! Map geometry comes from the map's BSP on fastdl.

mod bsp;
mod sessions;
mod token;

use anyhow::{Context, Result, anyhow, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::Value;
use sessions::{Sessions, Viewer};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use stvtf::watch::{self, Watch};

const USAGE: &str = "\
stvtf-web: live top-down viewer of a TF2 match in the browser

  stvtf-web HOST:TVPORT [--password tv] [--listen 127.0.0.1:8080] [--hz 20] [--cache DIR] [--backpack DIR]
  stvtf-web HOST[:PORT] [tv] / 'connect HOST:PORT; password \"tv\"'
  stvtf-web [--public] [--max-matches 20] [--base-path /live] [--demo MATCH.jsonl[.gz]] [--sessions DIR]   (watch any server: paste a connect string on the page)
     With STVTF_TOKEN_KEY (64 hex digits) set, matches start only from a site's encrypted /start?t= links.
  stvtf-web --replay FILE.jsonl [--listen 127.0.0.1:8080]   (stvtf output, at game speed)";

const INDEX: &str = include_str!("../index.html");
const LANDING: &str = include_str!("../landing.html");
/// How long a match nobody watches keeps its `SourceTV` connection.
const WARM: &str = include_str!("../maps.txt");
const LINGER: Duration = Duration::from_secs(180);
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
const FASTDL: &str = "https://fastdl.serveme.tf/maps/";
const MAX_BSP: u64 = 512 << 20;
const DEMOSTF: &str = "https://maps.demos.tf/";
const MAX_OVERVIEW: u64 = 8 << 20;
const DAY: Duration = Duration::from_secs(86400);
/// Request line and headers together; the pages send far less.
const MAX_REQUEST: u64 = 16 << 10;
/// Open connections, browsers on `/events` included; more are closed straight away.
const MAX_CONNECTIONS: usize = 512;
/// Event streams from one address, so a single client cannot take all the connections. A LAN
/// event or an office behind one address stays well under it.
const MAX_STREAMS_PER_CLIENT: usize = 32;

/// The latest match state for browsers that join late, and a queue per browser.
#[derive(Default)]
struct Hub {
    status: Option<Arc<str>>,
    server: Option<Arc<str>>,
    world: Option<Arc<str>>,
    snapshot: Option<Arc<str>>,
    clients: Vec<SyncSender<Arc<str>>>,
    map: Option<String>,
    /// Feed events of the current map, so a page opened late still shows kills and chat.
    history: VecDeque<Arc<str>>,
    /// Tick and round time of the latest snapshot, to stamp events with time into the round.
    round_clock: Option<(u64, f64)>,
    /// When the latest snapshot came in, so a page opened after the stream stopped can say so.
    snapshot_at: Option<Instant>,
    /// Name and team of everyone seen on this map, by id: the feed keeps them for players who left.
    names: HashMap<u64, Value>,
}

const HISTORY: usize = 1000;
/// Event fields that hold player ids.
const PLAYER_FIELDS: [&str; 11] =
    ["player", "attacker", "victim", "assister", "medic", "target", "blocker", "owner", "builder", "cappers", "carrier"];
const FEED: [&str; 18] = [
    "kill",
    "flag",
    "uber",
    "uber_ready",
    "medic_death",
    "capture_blocked",
    "building_destroyed",
    "banner",
    "point_captured",
    "round_start",
    "round_win",
    "scores",
    "stalemate",
    "overtime",
    "game_over",
    "player_leave",
    "team_change",
    "chat",
];

type SharedHub = Arc<Mutex<Hub>>;

fn publish(hub: &SharedHub, v: &Value) {
    let line: Arc<str> = v.to_string().into();
    let mut h = hub.lock().unwrap();
    match v["type"].as_str() {
        Some("status") => {
            if v["state"] == "map_change" {
                h.snapshot = None;
                h.history.clear();
                h.names.clear();
            }
            h.status = Some(line.clone());
        }
        Some("server") => {
            // Signon restarts announce the same map again; only a new map starts a new history.
            let map = v["map"].as_str().map(str::to_string);
            if map != h.map {
                h.history.clear();
                h.names.clear();
                h.map = map;
            }
            (h.server, h.snapshot, h.round_clock) = (Some(line.clone()), None, None);
        }
        Some("world") => h.world = Some(line.clone()),
        Some("snapshot") => {
            h.round_clock = v["tick"].as_u64().zip(v["round_time"].as_f64());
            h.snapshot = Some(line.clone());
            h.snapshot_at = Some(Instant::now());
            for p in v["players"].as_array().into_iter().flatten() {
                if let Some(id) = p["id"].as_u64() {
                    h.names.insert(id, serde_json::json!({"name": p["name"], "team": p["team"]}));
                }
            }
        }
        Some(t) if FEED.contains(&t) => {
            let mut past = v.clone();
            past["history"] = true.into();
            let ids = PLAYER_FIELDS.iter().flat_map(|f| v[f].as_array().cloned().unwrap_or_else(|| vec![v[f].clone()]));
            let names: serde_json::Map<String, Value> =
                ids.filter_map(|id| Some((id.as_u64()?.to_string(), h.names.get(&id.as_u64()?)?.clone()))).collect();
            if !names.is_empty() {
                past["names"] = names.into();
            }
            if let (Some((tick, time)), Some(at)) = (h.round_clock, v["tick"].as_u64()) {
                let seconds = time + (at as f64 - tick as f64) * 0.015;
                past["at"] = ((seconds * 10.0).round() / 10.0).max(0.0).into();
            }
            if h.history.len() == HISTORY {
                h.history.pop_front();
            }
            h.history.push_back(past.to_string().into());
        }
        _ => {}
    }
    // A browser that cannot keep up is dropped; it reconnects and resyncs from the latest state.
    h.clients.retain(|c| !matches!(c.try_send(line.clone()), Err(TrySendError::Full(_) | TrySendError::Disconnected(_))));
}

fn subscribe(hub: &SharedHub) -> Receiver<Arc<str>> {
    let (tx, rx) = sync_channel(4096);
    let mut h = hub.lock().unwrap();
    // The snapshot goes before the history so the page knows the players' names.
    for line in [&h.status, &h.server, &h.world, &h.snapshot].into_iter().flatten().chain(&h.history) {
        let _ = tx.try_send(line.clone());
    }
    if let Some(at) = h.snapshot_at {
        let _ = tx.try_send(serde_json::json!({"type": "data_age", "ms": at.elapsed().as_millis() as u64}).to_string().into());
    }
    h.clients.push(tx);
    rx
}

/// Plays a saved stvtf output (gzipped if it ends in `.gz`) on a loop, each pass as a new match,
/// holding still whenever `play` says nobody is watching.
fn replay(hub: &SharedHub, path: &Path, play: &dyn Fn() -> bool) -> Result<()> {
    for pass in 0.. {
        if pass > 0 {
            // A pause between passes, which also keeps a file with (almost) no timing from
            // looping flat out.
            std::thread::sleep(Duration::from_secs(3));
            publish(hub, &serde_json::json!({"type": "status", "state": "map_change"}));
        }
        let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let reader: Box<dyn Read> =
            if path.extension().is_some_and(|e| e == "gz") { Box::new(flate2::read::GzDecoder::new(file)) } else { Box::new(file) };
        let (mut last, mut timed): (Option<u64>, bool) = (None, false);
        for line in BufReader::new(reader).lines() {
            while !play() {
                std::thread::sleep(Duration::from_millis(250));
            }
            let v: Value = serde_json::from_str(&line?)?;
            // Live output has wall-clock `time`; offline output (`stvtf replay`) only ticks.
            // Each line waits for the gap since the previous one, capped so map loads and
            // tick resets do not stall the replay.
            if let Some(t) = v["time"].as_u64().or_else(|| v["tick"].as_u64().filter(|&t| t > 0).map(|t| t * 15)) {
                if let Some(prev) = last {
                    std::thread::sleep(Duration::from_millis(t.saturating_sub(prev).min(1000)));
                }
                last = Some(t);
                timed = true;
            }
            publish(hub, &v);
        }
        // Without any, the loop would replay the file as fast as it can, forever.
        if !timed {
            bail!("{}: no line has a time or tick", path.display());
        }
    }
    Ok(())
}

/// Builds a map that is not in the cache yet only when `fetch`: otherwise anyone could have every
/// map on fastdl downloaded and kept here.
fn map_geometry(cache: &Path, name: &str, fetch: bool) -> Result<Vec<u8>> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("bad map name");
    }
    std::fs::create_dir_all(cache)?;
    // Bump the suffix when the extracted data changes, so cached maps are redone.
    let json = cache.join(format!("{name}.v3.json"));
    if let Ok(body) = std::fs::read(&json) {
        return Ok(body);
    }
    if !fetch {
        bail!("no match is on {name}");
    }
    let bsp_path = cache.join(format!("{name}.bsp"));
    let data = if let Ok(d) = std::fs::read(&bsp_path) {
        d
    } else {
        let mut d = Vec::new();
        // One byte over the limit tells a map that is too big from one that just fits.
        ureq::get(&format!("{FASTDL}{name}.bsp")).call()?.into_reader().take(MAX_BSP + 1).read_to_end(&mut d)?;
        if d.len() as u64 > MAX_BSP {
            bail!("{name}.bsp is over {} MiB", MAX_BSP >> 20);
        }
        write_atomic(&bsp_path, &d)?;
        d
    };
    let body = match bsp::geometry(&data) {
        Ok(g) => g.to_string().into_bytes(),
        Err(e) => {
            // A broken download would otherwise fail every request for this map; the next one fetches it again.
            let _ = std::fs::remove_file(&bsp_path);
            return Err(e);
        }
    };
    write_atomic(&json, &body)?;
    // Only a bump of the suffix needs the BSP again, and the most played maps add up to GBs.
    let _ = std::fs::remove_file(&bsp_path);
    Ok(body)
}

/// demos.tf's map overviews, `boundaries.json` and `<map>.webp`, kept in the cache so viewers do
/// not each load them from demos.tf's own server. The list is fetched again after a day; while
/// demos.tf is down, what the cache has is served.
fn overview(cache: &Path, file: &str) -> Result<Vec<u8>> {
    let image = match file.strip_suffix(".webp") {
        _ if file == "boundaries.json" => None,
        Some(map) if !map.is_empty() && map.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => Some(map),
        _ => bail!("no such overview"),
    };
    let remote = if image.is_some() { format!("images/{file}") } else { file.to_string() };
    let dir = cache.join("demostf");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(file);
    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().map(|t| t.elapsed().unwrap_or_default());
    if age.is_some_and(|a| image.is_some() || a < DAY) {
        return Ok(std::fs::read(&path)?);
    }
    // Only the maps demos.tf has: any other name would be a request to demos.tf every time.
    if image.is_some_and(|map| !demostf_maps(cache).iter().any(|m| m == map)) {
        bail!("no such overview");
    }
    let mut body = Vec::new();
    let fetched = ureq::get(&format!("{DEMOSTF}{remote}"))
        .call()
        .map_err(anyhow::Error::from)
        .and_then(|r| Ok(r.into_reader().take(MAX_OVERVIEW + 1).read_to_end(&mut body)?));
    match fetched {
        Ok(n) if n as u64 <= MAX_OVERVIEW => {
            write_atomic(&path, &body)?;
            Ok(body)
        }
        Ok(_) => bail!("{file} is over {} MiB", MAX_OVERVIEW >> 20),
        Err(e) => std::fs::read(&path).map_err(|_| e),
    }
}

/// The maps demos.tf has an overview of, from its `boundaries.json`.
fn demostf_maps(cache: &Path) -> Vec<String> {
    overview(cache, "boundaries.json")
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Map<String, Value>>(&b).ok())
        .map(|m| m.into_iter().map(|(k, _)| k).collect())
        .unwrap_or_default()
}

/// Writes through a temporary file and a rename: a cached file that can be read is complete,
/// even after a crash halfway through writing it.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)
}

/// What the server watches: one match at `/`, or any number at `/w/<id>/`.
enum Matches {
    One(SharedHub),
    Many(Sessions),
}

struct App {
    matches: Matches,
    cache: PathBuf,
    /// The backpack pictures and `items.json`.
    backpack: PathBuf,
    maps: Mutex<()>,
    overviews: Mutex<()>,
    /// Where a proxy serves us, like `/live`; empty at the root.
    base: String,
    /// When set, matches start only from tokens a site in front of us made.
    keys: Option<Arc<token::Keys>>,
    /// Open event streams by client address.
    streams: Mutex<HashMap<IpAddr, usize>>,
}

/// Counts an event stream against its client's limit until dropped.
struct Stream<'a>(&'a App, IpAddr);

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        let mut streams = self.0.streams.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = streams.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                streams.remove(&self.1);
            }
        }
    }
}

impl App {
    /// A page with its links resolved under `base`.
    fn page(&self, html: &str) -> Vec<u8> {
        html.replace(r#"<base href="/">"#, &format!(r#"<base href="{}/">"#, self.base)).into_bytes()
    }

    /// Whether a match being watched is on `map`: only those maps are fetched and built on request.
    fn on_map(&self, map: &str) -> bool {
        match &self.matches {
            Matches::One(hub) => hub.lock().unwrap_or_else(PoisonError::into_inner).map.as_deref() == Some(map),
            Matches::Many(sessions) => sessions.on_map(map),
        }
    }

    /// A slot for one more event stream from `client`, or None when it has too many open.
    fn stream_slot(&self, client: IpAddr) -> Option<Stream<'_>> {
        let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        let n = streams.entry(client).or_default();
        if *n >= MAX_STREAMS_PER_CLIENT {
            return None;
        }
        *n += 1;
        Some(Stream(self, client))
    }

    fn redirect(&self, s: &mut TcpStream, status: &str, path: &str) -> std::io::Result<()> {
        write!(s, "HTTP/1.1 {status}\r\nLocation: {}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", self.base)
    }
}

fn serve(mut s: TcpStream, app: &App) -> Result<()> {
    // A client that sends nothing, or stops reading `/events`, does not keep its thread forever.
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(s.try_clone()?.take(MAX_REQUEST));
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let (mut length, mut gzip, mut forwarded) = (0, false, None);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            bail!("request incomplete or over {MAX_REQUEST} bytes");
        }
        if header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("x-forwarded-for") {
                // The proxy in front adds the address it saw last; earlier ones are the client's say-so.
                forwarded = value.rsplit(',').next().and_then(|a| a.trim().parse::<IpAddr>().ok());
            } else if name.eq_ignore_ascii_case("accept-encoding") {
                gzip = value.split(',').any(|e| e.split(';').next().is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip")));
            }
        }
    }
    let peer = s.peer_addr()?.ip();
    let client = client_key(forwarded.filter(|_| on_our_network(peer)).unwrap_or(peer));
    let method = request.split_whitespace().next().unwrap_or("GET");
    let target = request.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let html = "text/html; charset=utf-8";
    match (&app.matches, path) {
        (_, "/up") => respond(&mut s, "200 OK", "text/plain", b"ok")?,
        (Matches::One(_), "/" | "/index.html" | "/overlay") => respond(&mut s, "200 OK", html, &app.page(INDEX))?,
        (Matches::One(hub), "/events") => match app.stream_slot(client) {
            Some(_slot) => stream(s, hub, gzip)?,
            None => too_many_streams(&mut s)?,
        },
        (Matches::Many(_), "/" | "/watch") if app.keys.is_some() => {
            respond(&mut s, "403 Forbidden", "text/plain", b"Start watching a match from the site that sent you here.")?;
        }
        (Matches::Many(sessions), "/start") => match start(app, sessions, query) {
            Ok(m) => app.redirect(&mut s, "303 See Other", &format!("/w/{}/", m.id))?,
            Err(e) => respond(&mut s, "400 Bad Request", "text/plain", e.to_string().as_bytes())?,
        },
        (Matches::Many(_), "/") if method == "GET" => respond(&mut s, "200 OK", html, &app.page(&landing("")))?,
        (Matches::Many(sessions), "/watch") if method == "POST" => {
            let mut body = vec![0; length.min(MAX_REQUEST as usize)];
            reader.read_exact(&mut body)?;
            let form = String::from_utf8_lossy(&body);
            let line = form.split('&').find_map(|kv| kv.strip_prefix("connect=")).map(form_decode).unwrap_or_default();
            match sessions.open(&line, None) {
                Ok(m) => app.redirect(&mut s, "303 See Other", &format!("/w/{}/", m.id))?,
                Err(e) => respond(&mut s, "400 Bad Request", html, &app.page(&landing(&e.to_string())))?,
            }
        }
        (Matches::Many(sessions), p) if p.starts_with("/w/") => {
            let (id, rest) = p[3..].split_once('/').unwrap_or((&p[3..], ""));
            match (sessions.get(id), rest) {
                (None, _) if app.keys.is_some() => {
                    respond(&mut s, "404 Not Found", "text/plain", b"That match is no longer being watched; open it again from the site.")?;
                }
                (None, _) => respond(
                    &mut s,
                    "404 Not Found",
                    html,
                    &app.page(&landing("That match is no longer being watched; paste its connect string to watch it again.")),
                )?,
                (Some(_), "") if !p.ends_with('/') => {
                    app.redirect(&mut s, "308 Permanent Redirect", &format!("{p}/"))?;
                }
                (Some(_), "" | "overlay") => respond(&mut s, "200 OK", html, &app.page(INDEX))?,
                (Some(m), "events") => match app.stream_slot(client) {
                    Some(_slot) => {
                        let _viewer = Viewer::new(m.clone());
                        stream(s, &m.hub, gzip)?;
                    }
                    None => too_many_streams(&mut s)?,
                },
                _ => respond(&mut s, "404 Not Found", "text/plain", b"not found")?,
            }
        }
        (_, "/fonts/RobotoCondensed-Regular.ttf") => {
            respond_cached(&mut s, "font/ttf", include_bytes!("../fonts/RobotoCondensed-Regular.ttf"))?;
        }
        (_, "/fonts/RobotoCondensed-Bold.ttf") => respond_cached(&mut s, "font/ttf", include_bytes!("../fonts/RobotoCondensed-Bold.ttf"))?,
        (_, "/killicons.css") => respond(&mut s, "200 OK", "text/css", include_bytes!("../killicons/killicons.css"))?,
        (_, "/killicons.webp") => respond(&mut s, "200 OK", "image/webp", include_bytes!("../killicons/killicons.webp"))?,
        (_, p) if p.starts_with("/icons/") => match ICONS.iter().find(|(name, _)| p[7..].strip_suffix(".svg") == Some(name)) {
            Some((_, svg)) => respond(&mut s, "200 OK", "image/svg+xml", svg)?,
            None => respond(&mut s, "404 Not Found", "text/plain", b"not found")?,
        },
        (_, p) if p.starts_with("/backpack/") => serve_backpack(&mut s, app, &p[10..])?,
        (_, p) if p.starts_with("/overview/") || p.starts_with("/map/") => serve_cached(&mut s, app, p)?,
        _ => respond(&mut s, "404 Not Found", "text/plain", b"not found")?,
    }
    Ok(())
}

/// Only a proxy on our own network (kamal-proxy) is believed about who the client is.
fn on_our_network(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(_) => !sessions::is_public(ip.to_canonical()),
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
    respond(s, "429 Too Many Requests", "text/plain", b"too many matches open from your address")
}

/// A map's geometry, or a demos.tf overview, from the cache (fetched and made on a miss).
fn serve_cached(s: &mut TcpStream, app: &App, p: &str) -> std::io::Result<()> {
    // The locks guard nothing, so a panic in an earlier request (poisoning them) does not matter.
    let result = if let Some(file) = p.strip_prefix("/overview/") {
        let _one_at_a_time = app.overviews.lock().unwrap_or_else(PoisonError::into_inner);
        overview(&app.cache, file)
    } else {
        let name = &p[5..];
        let fetch = app.on_map(name);
        let _one_at_a_time = app.maps.lock().unwrap_or_else(PoisonError::into_inner);
        map_geometry(&app.cache, name, fetch)
    };
    match result {
        Ok(body) if p.starts_with("/map/") => respond(s, "200 OK", "application/json", &body),
        Ok(body) if p == "/overview/boundaries.json" => respond_cached(s, "application/json", &body),
        Ok(body) => respond_cached(s, "image/webp", &body),
        Err(e) => respond(s, "404 Not Found", "text/plain", e.to_string().as_bytes()),
    }
}

/// A backpack picture or `items.json`; the name is checked so it cannot leave the directory.
fn serve_backpack(s: &mut TcpStream, app: &App, file: &str) -> std::io::Result<()> {
    let (stem, ctype) = match file.rsplit_once('.') {
        Some((stem, "webp")) => (stem, "image/webp"),
        Some(("items", "json")) => ("items", "application/json"),
        _ => ("", ""),
    };
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return respond(s, "404 Not Found", "text/plain", b"not found");
    }
    match std::fs::read(app.backpack.join(file)) {
        Ok(body) => respond_cached(s, ctype, &body),
        Err(_) => respond(s, "404 Not Found", "text/plain", b"not found"),
    }
}

/// Fetches demos.tf's overviews and builds the most played maps, so their first viewer does not wait.
fn warm(app: &App) {
    let lock = || app.overviews.lock().unwrap_or_else(PoisonError::into_inner);
    let known = {
        let _one_at_a_time = lock();
        demostf_maps(&app.cache)
    };
    for map in known {
        let _one_at_a_time = lock();
        if let Err(e) = overview(&app.cache, &format!("{map}.webp")) {
            eprintln!("stvtf-web: overview of {map}: {e}");
        }
    }
    for name in WARM.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let _one_at_a_time = app.maps.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(e) = map_geometry(&app.cache, name, true) {
            eprintln!("stvtf-web: {name}: {e}");
        }
    }
}

/// A file that never changes for a given build, so the browser keeps it for a day.
fn respond_cached(s: &mut TcpStream, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: max-age=86400\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)
}

/// The match a site's `/start?t=` token asks for.
fn start(app: &App, sessions: &Sessions, query: &str) -> Result<Arc<sessions::Session>> {
    let keys = app.keys.as_ref().ok_or_else(|| anyhow!("this server starts matches from its own page"))?;
    let token = query.split('&').find_map(|kv| kv.strip_prefix("t=")).unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let t = keys.open(token, now)?;
    sessions.open(&t.connect, t.owner.as_deref())
}

/// A match's events to one browser, until it goes away.
/// Snapshots are 20 a second of very similar JSON: gzip, flushed after each batch so nothing waits,
/// brings a viewer from about 110 KB/s down to a few.
fn stream(mut s: TcpStream, hub: &SharedHub, gzip: bool) -> Result<()> {
    write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nX-Accel-Buffering: no\r\nX-Content-Type-Options: nosniff\r\n{}\r\n",
        if gzip { "Content-Encoding: gzip\r\n" } else { "" }
    )?;
    let mut out: Box<dyn Write> = if gzip { Box::new(GzEncoder::new(s, Compression::new(5))) } else { Box::new(s) };
    // After a restart (a deploy), browsers come back after 1 s instead of their default 3 s.
    write!(out, "retry: 1000\n\n")?;
    let rx = subscribe(hub);
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
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

const FLAGS: [&str; 10] = ["password", "listen", "hz", "cache", "backpack", "replay", "max-matches", "base-path", "demo", "sessions"];
const SWITCHES: [&str; 1] = ["public"];

/// Positional arguments and `--name value` options; a mistyped option is an error rather than
/// silently falling back to its default.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<(Vec<String>, HashMap<String, String>)> {
    let (mut positional, mut flags) = (Vec::new(), HashMap::new());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.strip_prefix("--") {
            Some(name) if FLAGS.contains(&name) => {
                let value = it.next().ok_or_else(|| anyhow!("--{name} needs a value"))?;
                flags.insert(name.to_string(), value);
            }
            Some(name) if SWITCHES.contains(&name) => {
                flags.insert(name.to_string(), String::new());
            }
            Some(name) => bail!("unknown option --{name}\n\n{USAGE}"),
            None => positional.push(a),
        }
    }
    Ok((positional, flags))
}

/// Liberal in what it takes: `connect HOST:PORT; password "tv"`, `HOST:PORT tv`, `HOST; password tv`
/// or just `HOST`. Without a port, `SourceTV`'s default 27020; quotes and `connect` are optional.
fn parse_connect(line: &str) -> Option<(String, Option<String>)> {
    let mut line = line.trim();
    if line.get(..8).is_some_and(|c| c.eq_ignore_ascii_case("connect ")) {
        line = line[8..].trim_start();
    }
    let unquote = |s: &str| s.trim().trim_matches('"').to_string();
    let mut parts = line.split(';');
    let first = parts.next()?.trim();
    let (host, rest) = first.split_once(char::is_whitespace).unwrap_or((first, ""));
    let mut password = Some(unquote(rest.trim().strip_prefix("password").unwrap_or(rest)));
    for p in parts {
        if let Some(pw) = p.trim().strip_prefix("password") {
            password = Some(unquote(pw));
        }
    }
    let host = host.trim_matches('"');
    if host.is_empty() {
        return None;
    }
    let target = if host.contains(':') { host.to_string() } else { format!("{host}:27020") };
    Some((target, password.filter(|p| !p.is_empty())))
}

/// Counts an open connection in `count` until dropped.
struct Connection(&'static AtomicUsize);

impl Connection {
    fn open(count: &'static AtomicUsize) -> Option<Self> {
        // fetch_update is deprecated and its replacement still unstable on our toolchain.
        if count.fetch_add(1, Ordering::Relaxed) >= MAX_CONNECTIONS {
            count.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        Some(Connection(count))
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Any number of matches: the hub's options, the matches a previous process left, the demo.
fn many(flags: &HashMap<String, String>, hz: f32, keys: Option<&Arc<token::Keys>>) -> Result<Sessions> {
    let get = |k: &str| flags.get(k).map(String::as_str);
    let max = get("max-matches").map(|n| n.parse().with_context(|| format!("--max-matches {n}: not a number"))).transpose()?.unwrap_or(20);
    let mut sessions = Sessions::new(flags.contains_key("public"), max, hz);
    if let Some(dir) = get("sessions") {
        sessions.store = Some(sessions::Store { dir: PathBuf::from(dir), keys: keys.cloned() });
        let restored = sessions.restore();
        if restored > 0 {
            eprintln!("stvtf-web: picked up {restored} running matches");
        }
    }
    if let Some(file) = get("demo") {
        let file = PathBuf::from(file);
        sessions.pin("demo", move |s| {
            if let Err(e) = replay(&s.hub, &file, &|| s.watched()) {
                eprintln!("demo: {e:#}");
            }
        });
    }
    Ok(sessions)
}

/// A connect string pasted in the terminal works like one pasted on the page.
fn prompt(app: &App, url: &str) {
    let Matches::Many(sessions) = &app.matches else { return };
    let stdin = std::io::stdin();
    loop {
        eprint!("Paste a SourceTV connect string (connect HOST:PORT; password \"...\"), or open {url}/ : ");
        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line.trim().is_empty() {
            continue;
        }
        match sessions.open(&line, None) {
            Ok(m) => eprintln!("watching: {url}/w/{}/  (overlay: {url}/w/{}/overlay)", m.id, m.id),
            Err(e) => eprintln!("{e}"),
        }
    }
}

fn main() -> Result<()> {
    static STOP: AtomicBool = AtomicBool::new(false);
    static OPEN: AtomicUsize = AtomicUsize::new(0);
    if std::env::args().skip(1).any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let (positional, flags) = parse_args(std::env::args().skip(1))?;
    let get = |k: &str| flags.get(k).map(String::as_str);
    let listen = get("listen").unwrap_or("127.0.0.1:8080").to_string();
    let hz = get("hz").map(|hz| hz.parse().with_context(|| format!("--hz {hz}: not a number"))).transpose()?.unwrap_or(20.0);
    watch::stop_on_ctrl_c(&STOP)?;

    let keys = std::env::var("STVTF_TOKEN_KEY").ok().map(|k| token::Keys::from_hex(&k).map(Arc::new)).transpose()?;
    let matches = if let Some(file) = get("replay") {
        let (hub, file) = (SharedHub::default(), PathBuf::from(file));
        let replayed = hub.clone();
        std::thread::spawn(move || {
            if let Err(e) = replay(&replayed, &file, &|| true) {
                eprintln!("replay: {e}");
            }
        });
        Matches::One(hub)
    } else if !positional.is_empty() {
        let Some((target, password)) = parse_connect(&positional.join(" ")) else {
            eprintln!("{USAGE}");
            std::process::exit(2);
        };
        let mut w = Watch::new(&target, get("password").or(password.as_deref()).unwrap_or("tv"));
        w.snapshot_hz = hz;
        let hub = SharedHub::default();
        let watched = hub.clone();
        std::thread::spawn(move || {
            let result = watch::watch(&w, &STOP, &mut |v| publish(&watched, &v));
            if let Err(e) = &result {
                publish(&watched, &serde_json::json!({"type": "status", "state": "gave_up", "reason": e.to_string()}));
                eprintln!("stvtf-web: {e}");
            }
            if STOP.load(Ordering::Relaxed) {
                std::process::exit(0);
            }
        });
        Matches::One(hub)
    } else {
        Matches::Many(many(&flags, hz, keys.as_ref())?)
    };
    let base = get("base-path").unwrap_or("").trim_end_matches('/').to_string();
    if !base.is_empty() && !base.starts_with('/') {
        bail!("--base-path {base}: must start with /");
    }
    let console_prompt = keys.is_none() && std::io::IsTerminal::is_terminal(&std::io::stdin());
    let app = Arc::new(App {
        matches,
        cache: PathBuf::from(get("cache").unwrap_or("cache")),
        backpack: PathBuf::from(get("backpack").unwrap_or("backpack")),
        maps: Mutex::new(()),
        overviews: Mutex::new(()),
        base,
        keys,
        streams: Mutex::default(),
    });

    if let Matches::Many(_) = app.matches {
        // Stop idle matches, and on Ctrl-C disconnect them all cleanly before exiting.
        let reaper = app.clone();
        std::thread::spawn(move || {
            let Matches::Many(sessions) = &reaper.matches else { return };
            for round in 0u64.. {
                if STOP.load(Ordering::Relaxed) {
                    sessions.stop_all();
                    std::process::exit(0);
                }
                sessions.reap(LINGER);
                // Every 5 s: what a deploy's next container picks up, feed included.
                if round % 20 == 0 {
                    sessions.persist();
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        let warming = app.clone();
        std::thread::spawn(move || warm(&warming));
        if console_prompt {
            let (console, url) = (app.clone(), format!("http://{listen}{}", app.base));
            std::thread::spawn(move || prompt(&console, &url));
        }
    }

    let listener = TcpListener::bind(&listen)?;
    eprintln!("stvtf-web: http://{listen}/");
    for conn in listener.incoming() {
        let Ok(s) = conn else { continue };
        let Some(open) = Connection::open(&OPEN) else { continue };
        let app = app.clone();
        std::thread::spawn(move || {
            let _open = open;
            let _ = serve(s, &app);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connect_strings() {
        let some = |t: &str, p: Option<&str>| Some((t.to_string(), p.map(str::to_string)));
        let tv = |t: &str| some(t, Some("tv"));
        assert_eq!(parse_connect("connect 1.2.3.4:27015; password \"tv\""), tv("1.2.3.4:27015"));
        assert_eq!(parse_connect("CONNECT 1.2.3.4:27015;password tv\n"), tv("1.2.3.4:27015"));
        assert_eq!(parse_connect("1.2.3.4:27015 tv"), tv("1.2.3.4:27015"));
        assert_eq!(parse_connect("1.2.3.4; password tv"), tv("1.2.3.4:27020"));
        assert_eq!(parse_connect("bolus.fakkelbrigade.eu tv"), tv("bolus.fakkelbrigade.eu:27020"));
        assert_eq!(parse_connect("bolus.fakkelbrigade.eu:27015 tv"), tv("bolus.fakkelbrigade.eu:27015"));
        assert_eq!(parse_connect("bolus.fakkelbrigade.eu:27015; password tv"), tv("bolus.fakkelbrigade.eu:27015"));
        assert_eq!(parse_connect("1.2.3.4:27015 password tv"), tv("1.2.3.4:27015"));
        assert_eq!(parse_connect("connect 1.2.3.4:27020; password \"Floofy Goat\""), some("1.2.3.4:27020", Some("Floofy Goat")));
        assert_eq!(parse_connect("1.2.3.4:27020"), some("1.2.3.4:27020", None));
        assert_eq!(parse_connect("  \n"), None);
    }

    fn app(matches: Matches) -> App {
        App {
            matches,
            cache: PathBuf::from("unused"),
            backpack: PathBuf::from("unused"),
            maps: Mutex::new(()),
            overviews: Mutex::new(()),
            base: String::new(),
            keys: None,
            streams: Mutex::default(),
        }
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
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("EventSource"), "{page}");
        let bare = request_to(&app, format!("GET {} HTTP/1.1\r\n\r\n", location.trim_end_matches('/')).as_bytes()).1;
        assert!(bare.starts_with("HTTP/1.1 308"), "{bare}");
        assert!(request_to(&app, b"GET /w/nope/ HTTP/1.1\r\n\r\n").1.starts_with("HTTP/1.1 404"));
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
        assert!(response.is_empty());
    }

    #[test]
    fn connections_are_capped() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let open: Vec<_> = (0..MAX_CONNECTIONS).map(|_| Connection::open(&COUNT).unwrap()).collect();
        assert!(Connection::open(&COUNT).is_none());
        drop(open);
        assert!(Connection::open(&COUNT).is_some());
    }

    fn args(a: &[&str]) -> Result<(Vec<String>, HashMap<String, String>)> {
        parse_args(a.iter().map(ToString::to_string))
    }

    #[test]
    fn parses_options() {
        let (positional, flags) = args(&["host:27020", "--listen", "0.0.0.0:80", "--hz", "10"]).unwrap();
        assert_eq!(positional, ["host:27020"]);
        assert_eq!(flags["listen"], "0.0.0.0:80");
        assert_eq!(flags["hz"], "10");
    }

    #[test]
    fn unknown_or_incomplete_options_are_errors() {
        assert!(args(&["host:27020", "--lisen", "0.0.0.0:80"]).is_err());
        assert!(args(&["host:27020", "--listen"]).is_err());
    }

    #[test]
    fn replay_without_times_stops() {
        let path = std::env::temp_dir().join(format!("stvtf-web-replay-{}.jsonl", std::process::id()));
        for contents in ["", "{\"type\":\"status\"}\n"] {
            std::fs::write(&path, contents).unwrap();
            assert!(replay(&SharedHub::default(), &path, &|| true).is_err(), "{contents:?}");
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn write_atomic_replaces_the_file() {
        let dir = std::env::temp_dir().join(format!("stvtf-web-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.v3.json");
        std::fs::write(&path, b"old, longer contents").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_feed_keeps_names_of_players_who_left() {
        let hub = SharedHub::default();
        let snapshot = |players: Value| serde_json::json!({"type": "snapshot", "tick": 1, "players": players});
        publish(
            &hub,
            &snapshot(serde_json::json!([{"id": 3, "name": "can of tuna", "team": "red"}, {"id": 4, "name": "silvo", "team": "blue"}])),
        );
        publish(&hub, &serde_json::json!({"type": "kill", "tick": 2, "attacker": 3, "victim": 4, "assister": null}));
        publish(&hub, &serde_json::json!({"type": "point_captured", "tick": 3, "point": 0, "cappers": [3]}));
        publish(&hub, &snapshot(serde_json::json!([])));
        let lines: Vec<Value> = subscribe(&hub).try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
        let kill = lines.iter().find(|l| l["type"] == "kill").unwrap();
        assert_eq!(kill["names"]["3"]["name"], "can of tuna");
        assert_eq!(kill["names"]["4"]["team"], "blue");
        let cap = lines.iter().find(|l| l["type"] == "point_captured").unwrap();
        assert_eq!(cap["names"]["3"]["name"], "can of tuna");
    }

    #[test]
    fn serves_backpack_pictures() {
        let dir = std::env::temp_dir().join(format!("stvtf-web-backpack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("scattergun.webp"), b"RIFF").unwrap();
        std::fs::write(dir.join("items.json"), b"{}").unwrap();
        let mut a = app(Matches::One(SharedHub::default()));
        a.backpack = dir.clone();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_overview_files_are_fetched() {
        let cache = std::env::temp_dir().join(format!("stvtf-web-overview-{}", std::process::id()));
        for bad in ["../boundaries.json", "x.json", "a/b.webp", ".webp", "boundaries.webp.json"] {
            assert!(overview(&cache, bad).is_err(), "{bad}");
        }
        // A cached image is served without asking demos.tf.
        std::fs::create_dir_all(cache.join("demostf")).unwrap();
        std::fs::write(cache.join("demostf/cp_test.webp"), b"RIFF").unwrap();
        assert_eq!(overview(&cache, "cp_test.webp").unwrap(), b"RIFF");
        // A map demos.tf does not list is not asked for (a fresh list is not fetched again).
        std::fs::write(cache.join("demostf/boundaries.json"), br#"{"cp_test": {}}"#).unwrap();
        assert_eq!(demostf_maps(&cache), ["cp_test"]);
        assert!(overview(&cache, "cp_nothere.webp").is_err());
        std::fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn only_maps_being_watched_are_built() {
        let cache = std::env::temp_dir().join(format!("stvtf-web-onmap-{}", std::process::id()));
        // Not cached and not being watched: an error, without asking fastdl.
        assert!(map_geometry(&cache, "cp_nothere", false).unwrap_err().to_string().contains("no match"));
        std::fs::write(cache.join("cp_cached.v3.json"), b"{}").unwrap();
        assert_eq!(map_geometry(&cache, "cp_cached", false).unwrap(), b"{}");
        std::fs::remove_dir_all(&cache).unwrap();

        let hub = SharedHub::default();
        let app = app(Matches::One(hub.clone()));
        assert!(!app.on_map("cp_process_f12"));
        publish(&hub, &serde_json::json!({"type": "server", "map": "cp_process_f12"}));
        assert!(app.on_map("cp_process_f12") && !app.on_map("cp_badlands"));
    }

    #[test]
    fn event_streams_are_capped_per_client() {
        let app = app(Matches::One(SharedHub::default()));
        let (a, b): (IpAddr, IpAddr) = ("203.0.113.1".parse().unwrap(), "203.0.113.2".parse().unwrap());
        let open: Vec<_> = (0..MAX_STREAMS_PER_CLIENT).map(|_| app.stream_slot(a).unwrap()).collect();
        assert!(app.stream_slot(a).is_none());
        assert!(app.stream_slot(b).is_some(), "other clients still get in");
        drop(open);
        assert!(app.stream_slot(a).is_some());
        assert!(app.streams.lock().unwrap().is_empty(), "closed streams are forgotten");
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

    #[test]
    fn warm_maps_are_valid_names() {
        let names: Vec<&str> = WARM.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        assert!(names.len() > 10);
        assert!(names.iter().all(|n| n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')), "{names:?}");
    }

    #[test]
    fn broken_cached_bsp_is_removed() {
        let cache = std::env::temp_dir().join(format!("stvtf-web-test-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        let bsp = cache.join("broken.bsp");
        std::fs::write(&bsp, b"VBSP\0\0\0\0").unwrap();
        assert!(map_geometry(&cache, "broken", true).is_err());
        // So the next request downloads it again instead of failing on it forever.
        assert!(!bsp.exists());
        std::fs::remove_dir_all(&cache).unwrap();
    }
}
