//! Live top-down web viewer for a TF2 match, fed by stvtf.
//!
//! Given a server, one `SourceTV` connection fanned out to every browser over Server-Sent
//! Events; without one, a page to start watching any server, each match at its own link.
//! Map geometry comes from the map's BSP on fastdl.

mod bsp;
mod cache;
mod cli;
mod http;
mod hub;
mod replay;
mod sessions;
mod token;

use crate::cache::{WARM, demostf_maps, map_geometry, overview};
use crate::cli::{USAGE, parse_args, parse_connect};
use crate::http::{MAX_STREAMS_PER_CLIENT, app_version, serve};
use crate::hub::{SharedHub, publish};
use crate::replay::replay;
use crate::sessions::Sessions;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::Write;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use stvtf::Line;
use stvtf::output::Status;
use stvtf::watch::{self, Watch};

/// How long a match nobody watches keeps its `SourceTV` connection.
const LINGER: Duration = Duration::from_secs(180);

/// Open connections, browsers on `/events` included; more are closed straight away.
const MAX_CONNECTIONS: usize = 512;

/// Locks `m` even when a thread panicked while holding it: nothing here leaves its data
/// half-changed, so one failed request must not fail every later one.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How often the sessions are reaped, and every how many rounds of that they are saved (5 s).
const HOUSEKEEPING: Duration = Duration::from_millis(250);
const PERSIST_ROUNDS: u64 = 20;

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
    /// Where `/` sends visitors when matches start only from the site's links.
    home: Option<String>,
    /// Open event streams by client address.
    streams: Mutex<HashMap<IpAddr, usize>>,
}

/// Counts an event stream against its client's limit until dropped.
struct Stream<'a>(&'a App, IpAddr);

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        let mut streams = lock(&self.0.streams);
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
        html.replace(r#"<base href="/">"#, &format!(r#"<base href="{}/">"#, self.base))
            .replace("\"app/dev/", &format!("\"app/{}/", app_version()))
            .into_bytes()
    }

    /// Whether a match being watched is on `map`: only those maps are fetched and built on request.
    fn on_map(&self, map: &str) -> bool {
        match &self.matches {
            Matches::One(hub) => lock(hub).map.as_deref() == Some(map),
            Matches::Many(sessions) => sessions.on_map(map),
        }
    }

    /// A slot for one more event stream from `client`, or None when it has too many open.
    fn stream_slot(&self, client: IpAddr) -> Option<Stream<'_>> {
        let mut streams = lock(&self.streams);
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

/// Fetches demos.tf's overviews and builds the most played maps, so their first viewer does not wait.
fn warm(app: &App) {
    let known = {
        let _one_at_a_time = lock(&app.overviews);
        demostf_maps(&app.cache)
    };
    for map in known {
        let _one_at_a_time = lock(&app.overviews);
        if let Err(e) = overview(&app.cache, &format!("{map}.webp")) {
            eprintln!("stvtf-web: overview of {map}: {e}");
        }
    }
    for name in WARM.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let _one_at_a_time = lock(&app.maps);
        if let Err(e) = map_geometry(&app.cache, name, true) {
            eprintln!("stvtf-web: {name}: {e}");
        }
    }
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
        pin_demo(&sessions, "demo", PathBuf::from(file));
    }
    if let Some(dir) = get("demos") {
        for (id, file) in demo_files(Path::new(dir))? {
            pin_demo(&sessions, &id, file);
        }
    }
    Ok(sessions)
}

/// A fixed match at `/w/<id>/` playing a saved `stvtf` output on a loop while someone watches.
fn pin_demo(sessions: &Sessions, id: &str, file: PathBuf) {
    let name = id.to_string();
    sessions.pin(id, move |s| {
        if let Err(e) = replay(&s.hub, &file, &|| s.watched()) {
            eprintln!("{name}: {e:#}");
        }
    });
}

/// The recordings in a folder of demos (`NAME.jsonl` or `NAME.jsonl.gz`, NAME of letters, digits,
/// `_` and `-`), by NAME, sorted.
fn demo_files(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("--demos {}", dir.display()))? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(id) = name.strip_suffix(".jsonl.gz").or_else(|| name.strip_suffix(".jsonl")) else { continue };
        if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            found.push((id.to_string(), path));
        }
    }
    found.sort();
    Ok(found)
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
    let hz = get("hz").map_or(Ok(stvtf::DEFAULT_SNAPSHOT_HZ), stvtf::parse_snapshot_hz)?;
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
            let result = watch::watch(&w, &STOP, &mut |line| publish(&watched, &line));
            if let Err(e) = &result {
                publish(&watched, &Line::Status(Status::GaveUp { reason: e.to_string() }));
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
        home: get("home").map(str::to_string),
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
                // What a deploy's next container picks up, feed included.
                if round % PERSIST_ROUNDS == 0 {
                    sessions.persist();
                }
                std::thread::sleep(HOUSEKEEPING);
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
mod testing {
    use super::*;
    use stvtf::output;

    pub(crate) fn app(matches: Matches) -> App {
        App {
            matches,
            cache: PathBuf::from("unused"),
            backpack: PathBuf::from("unused"),
            maps: Mutex::new(()),
            overviews: Mutex::new(()),
            base: String::new(),
            keys: None,
            home: None,
            streams: Mutex::default(),
        }
    }

    pub(crate) fn server(map: &str) -> Line {
        Line::Server(output::Server {
            tick: 0,
            map: map.to_string(),
            hostname: "SourceTV".to_string(),
            tick_interval: 0.015,
            max_players: 25,
            spawn_count: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::MAX_STREAMS_PER_CLIENT;
    use crate::hub::{SharedHub, publish};
    use crate::testing::{app, server};

    #[test]
    fn connections_are_capped() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let open: Vec<_> = (0..MAX_CONNECTIONS).map(|_| Connection::open(&COUNT).unwrap()).collect();
        assert!(Connection::open(&COUNT).is_none());
        drop(open);
        assert!(Connection::open(&COUNT).is_some());
    }

    #[test]
    fn a_demo_folder_gives_one_match_per_recording() {
        let dir = std::env::temp_dir().join(format!("stvtf-web-demos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["pass_time.jsonl.gz", "rd.jsonl", "notes.txt", "bad name.jsonl", ".jsonl", "mvm-tank.jsonl"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let found: Vec<(String, String)> = demo_files(&dir)
            .unwrap()
            .into_iter()
            .map(|(id, path)| (id, path.file_name().unwrap().to_string_lossy().into_owned()))
            .collect();
        let expected = [("mvm-tank", "mvm-tank.jsonl"), ("pass_time", "pass_time.jsonl.gz"), ("rd", "rd.jsonl")];
        assert_eq!(found, expected.map(|(a, b)| (a.to_string(), b.to_string())));
        assert!(demo_files(&dir.join("missing")).is_err(), "a missing folder is an error, not no demos");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_server_knows_which_maps_are_watched() {
        let hub = SharedHub::default();
        let app = app(Matches::One(hub.clone()));
        assert!(!app.on_map("cp_process_f12"));
        publish(&hub, &server("cp_process_f12"));
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
}
