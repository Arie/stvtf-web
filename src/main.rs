//! Live top-down web viewer for a TF2 match, fed by stvmap.
//!
//! One `SourceTV` connection per process, fanned out to every browser over
//! Server-Sent Events. Map geometry comes from the map's BSP on fastdl.

mod bsp;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use stvmap::watch::{self, Watch};

const USAGE: &str = "\
stvmap-web: live top-down viewer of a TF2 match in the browser

  stvmap-web HOST:TVPORT [--password tv] [--listen 127.0.0.1:8080] [--hz 20] [--cache DIR]
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

/// The latest match state for browsers that join late, and a queue per browser.
#[derive(Default)]
struct Hub {
    status: Option<Arc<str>>,
    server: Option<Arc<str>>,
    world: Option<Arc<str>>,
    snapshot: Option<Arc<str>>,
    clients: Vec<SyncSender<Arc<str>>>,
}

type SharedHub = Arc<Mutex<Hub>>;

fn publish(hub: &SharedHub, v: &Value) {
    let line: Arc<str> = v.to_string().into();
    let mut h = hub.lock().unwrap();
    match v["type"].as_str() {
        Some("status") => {
            if v["state"] == "map_change" {
                h.snapshot = None;
            }
            h.status = Some(line.clone());
        }
        Some("server") => (h.server, h.snapshot) = (Some(line.clone()), None),
        Some("world") => h.world = Some(line.clone()),
        Some("snapshot") => h.snapshot = Some(line.clone()),
        _ => {}
    }
    // A browser that cannot keep up is dropped; it reconnects and resyncs from the latest state.
    h.clients.retain(|c| !matches!(c.try_send(line.clone()), Err(TrySendError::Full(_) | TrySendError::Disconnected(_))));
}

fn subscribe(hub: &SharedHub) -> Receiver<Arc<str>> {
    let (tx, rx) = sync_channel(4096);
    let mut h = hub.lock().unwrap();
    for line in [&h.status, &h.server, &h.world, &h.snapshot].into_iter().flatten() {
        let _ = tx.try_send(line.clone());
    }
    h.clients.push(tx);
    rx
}

fn replay(hub: &SharedHub, path: &Path) -> Result<()> {
    loop {
        let mut last: Option<u64> = None;
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
            }
            publish(hub, &v);
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
        ureq::get(&format!("{FASTDL}{name}.bsp")).call()?.into_reader().take(512 << 20).read_to_end(&mut d)?;
        std::fs::write(&bsp_path, &d)?;
        d
    };
    let body = bsp::geometry(&data)?.to_string().into_bytes();
    std::fs::write(&json, &body)?;
    Ok(body)
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(s, "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    s.write_all(body)
}

fn serve(mut s: TcpStream, hub: &SharedHub, cache: &Path, maps: &Mutex<()>) -> Result<()> {
    let mut reader = BufReader::new(s.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" {
            break;
        }
    }
    let target = request.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");
    match path {
        "/" | "/index.html" | "/overlay" => respond(&mut s, "200 OK", "text/html; charset=utf-8", INDEX.as_bytes())?,
        "/events" => {
            write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n")?;
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

fn main() -> Result<()> {
    static STOP: AtomicBool = AtomicBool::new(false);
    let mut positional = Vec::new();
    let mut flags = std::collections::HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.strip_prefix("--") {
            Some(name) => {
                flags.insert(name.to_string(), it.next().ok_or_else(|| anyhow!("--{name} needs a value"))?);
            }
            None => positional.push(a),
        }
    }
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
        let Some(target) = positional.first() else {
            eprintln!("{USAGE}");
            std::process::exit(2);
        };
        let mut w = Watch::new(target, get("password").unwrap_or("tv"));
        w.snapshot_hz = get("hz").map(str::parse).transpose()?.unwrap_or(20.0);
        watch::stop_on_ctrl_c(&STOP)?;
        let hub = hub.clone();
        std::thread::spawn(move || {
            let result = watch::watch(&w, &STOP, &mut |v| publish(&hub, &v));
            if let Err(e) = &result {
                publish(&hub, &serde_json::json!({"type": "status", "state": "gave_up", "reason": e.to_string()}));
                eprintln!("stvmap-web: {e}");
            }
            if STOP.load(std::sync::atomic::Ordering::Relaxed) {
                std::process::exit(0);
            }
        });
    }

    let listener = TcpListener::bind(&listen)?;
    eprintln!("stvmap-web: http://{listen}/");
    let maps = Arc::new(Mutex::new(()));
    for conn in listener.incoming() {
        let Ok(s) = conn else { continue };
        let (hub, cache, maps) = (hub.clone(), cache.clone(), maps.clone());
        std::thread::spawn(move || {
            let _ = serve(s, &hub, &cache, &maps);
        });
    }
    Ok(())
}
