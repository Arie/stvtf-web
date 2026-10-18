//! Live top-down web viewer for a TF2 match, fed by stvmap.
//!
//! One `SourceTV` connection per process, fanned out to every browser over
//! Server-Sent Events. Map geometry comes from the map's BSP on fastdl.

mod bsp;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use stvmap::watch::{self, Watch};

const USAGE: &str = "\
stvmap-web: live top-down viewer of a TF2 match in the browser

  stvmap-web HOST:TVPORT [--password tv] [--listen 127.0.0.1:8080] [--hz 20] [--cache DIR]
  stvmap-web HOST[:PORT] [tv] / 'connect HOST:PORT; password \"tv\"'   (or paste one when asked)
  stvmap-web --replay FILE.jsonl [--listen 127.0.0.1:8080]   (stvmap output, at game speed)";

const INDEX: &str = include_str!("../index.html");
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
/// Request line and headers together; the pages send far less.
const MAX_REQUEST: u64 = 16 << 10;
/// Open connections, browsers on `/events` included; more are closed straight away.
const MAX_CONNECTIONS: usize = 512;

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
}

const HISTORY: usize = 1000;
const FEED: [&str; 17] = [
    "kill",
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
            }
            h.status = Some(line.clone());
        }
        Some("server") => {
            // Signon restarts announce the same map again; only a new map starts a new history.
            let map = v["map"].as_str().map(str::to_string);
            if map != h.map {
                h.history.clear();
                h.map = map;
            }
            (h.server, h.snapshot, h.round_clock) = (Some(line.clone()), None, None);
        }
        Some("world") => h.world = Some(line.clone()),
        Some("snapshot") => {
            h.round_clock = v["tick"].as_u64().zip(v["round_time"].as_f64());
            h.snapshot = Some(line.clone());
            h.snapshot_at = Some(Instant::now());
        }
        Some(t) if FEED.contains(&t) => {
            let mut past = v.clone();
            past["history"] = true.into();
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

fn replay(hub: &SharedHub, path: &Path) -> Result<()> {
    loop {
        let (mut last, mut timed): (Option<u64>, bool) = (None, false);
        for line in BufReader::new(std::fs::File::open(path)?).lines() {
            let v: Value = serde_json::from_str(&line?)?;
            // Live output has wall-clock `time`; offline output (`stvmap replay`) only ticks.
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
}

fn map_geometry(cache: &Path, name: &str) -> Result<Vec<u8>> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("bad map name");
    }
    std::fs::create_dir_all(cache)?;
    // Bump the suffix when the extracted data changes, so cached maps are redone.
    let json = cache.join(format!("{name}.v2.json"));
    if let Ok(body) = std::fs::read(&json) {
        return Ok(body);
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
    Ok(body)
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

fn serve(mut s: TcpStream, hub: &SharedHub, cache: &Path, maps: &Mutex<()>) -> Result<()> {
    // A client that sends nothing, or stops reading `/events`, does not keep its thread forever.
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(s.try_clone()?.take(MAX_REQUEST));
    let mut request = String::new();
    reader.read_line(&mut request)?;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            bail!("request incomplete or over {MAX_REQUEST} bytes");
        }
        if header == "\r\n" {
            break;
        }
    }
    let target = request.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");
    match path {
        "/" | "/index.html" | "/overlay" => respond(&mut s, "200 OK", "text/html; charset=utf-8", INDEX.as_bytes())?,
        "/events" => {
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\n\r\n"
            )?;
            let rx = subscribe(hub);
            loop {
                match rx.recv_timeout(Duration::from_secs(15)) {
                    Ok(line) => write!(s, "data: {line}\n\n")?,
                    Err(RecvTimeoutError::Timeout) => write!(s, ": keepalive\n\n")?,
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }
        }
        "/killicons.css" => respond(&mut s, "200 OK", "text/css", include_bytes!("../killicons/killicons.css"))?,
        "/killicons.webp" => respond(&mut s, "200 OK", "image/webp", include_bytes!("../killicons/killicons.webp"))?,
        p if p.starts_with("/icons/") => match ICONS.iter().find(|(name, _)| p[7..].strip_suffix(".svg") == Some(name)) {
            Some((_, svg)) => respond(&mut s, "200 OK", "image/svg+xml", svg)?,
            None => respond(&mut s, "404 Not Found", "text/plain", b"not found")?,
        },
        p if p.starts_with("/map/") => {
            let result = {
                // Guards nothing, so a panic in an earlier request (poisoning it) does not matter.
                let _one_at_a_time = maps.lock().unwrap_or_else(PoisonError::into_inner);
                map_geometry(cache, &p[5..])
            };
            match result {
                Ok(body) => respond(&mut s, "200 OK", "application/json", &body)?,
                Err(e) => respond(&mut s, "404 Not Found", "text/plain", e.to_string().as_bytes())?,
            }
        }
        _ => respond(&mut s, "404 Not Found", "text/plain", b"not found")?,
    }
    Ok(())
}

const FLAGS: [&str; 5] = ["password", "listen", "hz", "cache", "replay"];

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
    let cache = Arc::new(PathBuf::from(get("cache").unwrap_or("cache")));
    let hub: SharedHub = Arc::default();

    if let Some(file) = get("replay") {
        let (hub, file) = (hub.clone(), PathBuf::from(file));
        std::thread::spawn(move || {
            if let Err(e) = replay(&hub, &file) {
                eprintln!("replay: {e}");
            }
        });
    } else {
        let line = if positional.is_empty() {
            eprint!("Paste the SourceTV connect string (connect HOST:PORT; password \"...\"): ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line
        } else {
            positional.join(" ")
        };
        let Some((target, password)) = parse_connect(&line) else {
            eprintln!("{USAGE}");
            std::process::exit(2);
        };
        let mut w = Watch::new(&target, get("password").or(password.as_deref()).unwrap_or("tv"));
        w.snapshot_hz = get("hz").map(|hz| hz.parse().with_context(|| format!("--hz {hz}: not a number"))).transpose()?.unwrap_or(20.0);
        watch::stop_on_ctrl_c(&STOP)?;
        let hub = hub.clone();
        std::thread::spawn(move || {
            let result = watch::watch(&w, &STOP, &mut |v| publish(&hub, &v));
            if let Err(e) = &result {
                publish(&hub, &serde_json::json!({"type": "status", "state": "gave_up", "reason": e.to_string()}));
                eprintln!("stvmap-web: {e}");
            }
            if STOP.load(Ordering::Relaxed) {
                std::process::exit(0);
            }
        });
    }

    let listener = TcpListener::bind(&listen)?;
    eprintln!("stvmap-web: http://{listen}/");
    let maps = Arc::new(Mutex::new(()));
    for conn in listener.incoming() {
        let Ok(s) = conn else { continue };
        let Some(open) = Connection::open(&OPEN) else { continue };
        let (hub, cache, maps) = (hub.clone(), cache.clone(), maps.clone());
        std::thread::spawn(move || {
            let _open = open;
            let _ = serve(s, &hub, &cache, &maps);
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

    /// Runs `serve` on one connection, with `send` written by the client; returns serve's result and the response.
    fn request(send: &[u8]) -> (Result<()>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        client.write_all(send).unwrap();
        let result = serve(server, &SharedHub::default(), Path::new("unused"), &Mutex::new(()));
        let mut response = String::new();
        let _ = client.read_to_string(&mut response);
        (result, response)
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
        let path = std::env::temp_dir().join(format!("stvmap-web-replay-{}.jsonl", std::process::id()));
        for contents in ["", "{\"type\":\"status\"}\n"] {
            std::fs::write(&path, contents).unwrap();
            assert!(replay(&SharedHub::default(), &path).is_err(), "{contents:?}");
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn write_atomic_replaces_the_file() {
        let dir = std::env::temp_dir().join(format!("stvmap-web-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.v2.json");
        std::fs::write(&path, b"old, longer contents").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn broken_cached_bsp_is_removed() {
        let cache = std::env::temp_dir().join(format!("stvmap-web-test-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        let bsp = cache.join("broken.bsp");
        std::fs::write(&bsp, b"VBSP\0\0\0\0").unwrap();
        assert!(map_geometry(&cache, "broken").is_err());
        // So the next request downloads it again instead of failing on it forever.
        assert!(!bsp.exists());
        std::fs::remove_dir_all(&cache).unwrap();
    }
}
