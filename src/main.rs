//! Live top-down web viewer for a TF2 match, fed by stvtf.
//!
//! Given a server, one `SourceTV` connection fanned out to every browser over Server-Sent
//! Events; without one, a page to start watching any server, each match at its own link.
//! Map geometry comes from the map's BSP on fastdl.

#[macro_use]
mod log;
mod cli;
mod maps;
mod matches;
mod relays;
mod rooms;
mod voice;
mod web;

use crate::cli::{DEFAULT_TV_PASSWORD, Options, USAGE, parse_connect};
use crate::maps::cache::{demostf_maps, map_geometry_gzip, overview, warm_maps};
use crate::matches::hub::{SharedHub, watch_into};
use crate::matches::replay::replay;
use crate::matches::sessions::Sessions;
use crate::web::{MAX_STREAMS_PER_CLIENT, app_version, serve};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use stvtf::watch;

/// How long a match nobody watches keeps its `SourceTV` connection.
const LINGER: Duration = Duration::from_secs(180);
/// A room ends this long after its last viewer left: a reload does not end it.
const ROOM_LINGER: Duration = Duration::from_secs(60);

/// Open connections, browsers on `/events` included; more are closed straight away.
const MAX_CONNECTIONS: usize = 512;
/// The exit status for a command line that does not say what to watch.
const EXIT_USAGE: i32 = 2;

/// Whether `s` is a name safe to use in a path: letters, digits and the bytes in `extra`.
fn is_name(s: &str, extra: &[u8]) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

/// Locks `m` even when a thread panicked while holding it: nothing here leaves its data
/// half-changed, so one failed request must not fail every later one.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How often the sessions and rooms are reaped: one housekeeping round.
const HOUSEKEEPING: Duration = Duration::from_millis(250);
/// Rounds between saves of the matches: five seconds.
const PERSIST_ROUNDS: u64 = 20;
/// Rounds between saves of the rooms: a second.
const ROOM_PERSIST_ROUNDS: u64 = 4;
/// Rounds between sweeps of old files: five minutes.
const SWEEP_ROUNDS: u64 = 1200;

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
    /// One build of each map at a time; a map being built does not hold up the others.
    maps: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    overviews: Mutex<()>,
    /// Where a proxy serves us, like `/live`; empty at the root.
    base: String,
    /// When set, matches start only from tokens a site in front of us made.
    keys: Option<Arc<web::token::Keys>>,
    /// Where `/` sends visitors when matches start only from the site's links.
    home: Option<String>,
    /// Open event streams by client address.
    streams: Mutex<HashMap<IpAddr, usize>>,
    rooms: rooms::registry::Rooms,
    /// The key the voice bot posts with (`STVTF_VOICE_KEY`); without it no voice is taken in.
    voice_key: Option<String>,
    /// Discord users and the Steam accounts linked to them, for voice from Discord.
    voice_links: voice::links::Links,
    /// Where the voice bot is added to a Discord server (`STVTF_VOICE_BOT`), for the invite.
    voice_bot: Option<String>,
    /// The bell the voice bot waits on for Mumble channels asked for.
    mumble_changes: voice::mumble::Changes,
    /// Whether a Mumble server may be on this machine or its network (`STVTF_MUMBLE_ANYWHERE=1`):
    /// at a LAN.
    mumble_anywhere: bool,
    /// Whom people in voice channels were said to be, from one match to the next.
    voice_who: voice::known::WhoHistory,
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

    /// Whether a match being watched, or a room, is on `map`: only those maps are fetched and
    /// built on request.
    fn on_map(&self, map: &str) -> bool {
        let watched = match &self.matches {
            Matches::One(hub) => lock(hub).map.as_deref() == Some(map),
            Matches::Many(sessions) => sessions.on_map(map),
        };
        watched || self.rooms.on_map(map)
    }

    /// The lock around building map `name`.
    fn map_lock(&self, name: &str) -> Arc<Mutex<()>> {
        lock(&self.maps).entry(name.to_string()).or_default().clone()
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

    /// A redirect to `path` under `base`.
    fn redirect(&self, s: &mut TcpStream, status: &str, path: &str) -> std::io::Result<()> {
        web::redirect(s, status, &format!("{}{path}", self.base))
    }
}

/// Fetches demos.tf's overviews and builds the most played maps, so their first viewer does not
/// wait.
fn warm(app: &App) {
    let known = {
        let _one_at_a_time = lock(&app.overviews);
        demostf_maps(&app.cache)
    };
    for map in known {
        let _one_at_a_time = lock(&app.overviews);
        if let Err(e) = overview(&app.cache, &format!("{map}.webp")) {
            log!("overview of {map}: {e}");
        }
    }
    for name in warm_maps() {
        let map_lock = app.map_lock(name);
        let _one_at_a_time = lock(&map_lock);
        // Gzipped too: browsers take that, and the JSON is made on the way.
        if let Err(e) = map_geometry_gzip(&app.cache, name, true) {
            log!("{name}: {e}");
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

/// Any number of matches: the hub's options, the matches a previous process left, the demos.
fn many(options: &Options, keys: Option<&Arc<web::token::Keys>>) -> Result<Sessions> {
    let mut sessions = Sessions::new(options.public, options.max_matches, options.hz);
    if let Some(dir) = &options.sessions {
        sessions.store = Some(matches::sessions::Store { dir: dir.clone(), keys: keys.cloned() });
        // Before matches are picked up, so they record as the new ones do.
        sessions.demos = options.relays.is_some() || options.relay_socket.is_some();
        let restored = sessions.restore();
        if restored > 0 {
            log!("picked up {restored} running matches");
        }
    }
    if let Some(file) = &options.demo {
        pin_demo(&sessions, "demo", file.clone());
    }
    if let Some(dir) = &options.demos {
        for (id, file) in demo_files(dir)? {
            pin_demo(&sessions, &id, file);
        }
    }
    Ok(sessions)
}

/// A fixed match at `/w/<id>/` playing a saved `stvtf` output on a loop while someone watches.
fn pin_demo(sessions: &Sessions, id: &str, file: PathBuf) {
    let name = id.to_string();
    sessions.pin(id, move |s| {
        s.set_file(file.clone());
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
        if is_name(id, b"_-") {
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
        match sessions.open(&line, None, None) {
            Ok(m) => eprintln!("watching: {url}/w/{}/  (overlay: {url}/w/{}/overlay)", m.id, m.id),
            Err(e) => eprintln!("{e:#}"),
        }
    }
}

/// What the options ask to watch: a saved match, one server, or any number of them.
fn matches(options: &Options, keys: Option<&Arc<web::token::Keys>>, stop: &'static AtomicBool) -> Result<Matches> {
    let hub = SharedHub::default();
    let fed = hub.clone();
    if let Some(file) = options.replay.clone() {
        std::thread::spawn(move || {
            if let Err(e) = replay(&fed, &file, &|| true) {
                eprintln!("replay: {e}");
            }
        });
    } else if let Some(connect) = &options.connect {
        let Some((target, password)) = parse_connect(connect) else {
            eprintln!("{USAGE}");
            std::process::exit(EXIT_USAGE);
        };
        let password = options.password.clone().or(password).unwrap_or_else(|| DEFAULT_TV_PASSWORD.to_string());
        let hz = options.hz;
        std::thread::spawn(move || {
            if let Err(e) = watch_into(&fed, &target, &password, hz, None, stop) {
                log!("{e}");
            }
            if stop.load(Ordering::Relaxed) {
                std::process::exit(0);
            }
        });
    } else {
        return Ok(Matches::Many(many(options, keys)?));
    }
    Ok(Matches::One(hub))
}

/// The ids of the matches whose demos rooms can be relayed from: the sweep leaves them.
fn live_ids(matches: &Matches) -> std::collections::HashSet<String> {
    let Matches::Many(sessions) = matches else { return std::collections::HashSet::new() };
    sessions.with_demos()
}

/// Stops idle matches and saves the running ones, and on Ctrl-C disconnects them all cleanly
/// before exiting.
fn keep_house(app: Arc<App>, stop: &'static AtomicBool) {
    std::thread::spawn(move || {
        let Matches::Many(sessions) = &app.matches else { return };
        for round in 0u64.. {
            if stop.load(Ordering::Relaxed) {
                // The feed up to now, for the process that takes over.
                sessions.persist();
                app.rooms.persist();
                sessions.stop_all();
                std::process::exit(0);
            }
            sessions.reap(LINGER);
            app.rooms.reap(ROOM_LINGER);
            // What a deploy's next container picks up, feed included.
            if round % PERSIST_ROUNDS == 0 {
                sessions.persist();
            }
            // Where each room is, to the second; relayed rooms the process before let go of.
            if round % ROOM_PERSIST_ROUNDS == 0 {
                app.rooms.persist();
                app.rooms.pick_up_relayed(sessions);
            }
            // Old files of rooms, demos and live matches.
            if round % SWEEP_ROUNDS == 0 {
                app.rooms.sweep(&live_ids(&app.matches));
            }
            std::thread::sleep(HOUSEKEEPING);
        }
    });
}

/// Serves each connection on its own thread, up to `MAX_CONNECTIONS` at once.
fn accept(listen: &str, app: &Arc<App>, open: &'static AtomicUsize) -> Result<()> {
    let listener = TcpListener::bind(listen)?;
    log!("http://{listen}/");
    for conn in listener.incoming() {
        let Ok(s) = conn else { continue };
        let Some(open) = Connection::open(open) else { continue };
        let app = app.clone();
        std::thread::spawn(move || {
            let _open = open;
            let _ = serve(s, &app);
        });
    }
    Ok(())
}

/// Who is who in a team's voice channel: kept in the store, the same as the stv.tf that holds
/// it for all regions (`STVTF_VOICE_HOME`; none on that one itself), and with the sites asked
/// who a Discord user is (`STVTF_LINK_SITES`, each as `--home`; else the one this belongs to),
/// where voice is taken in and the sites share a key.
fn who_is_who(options: &Options, keys: Option<&Arc<web::token::Keys>>, voice_key: Option<&str>) -> voice::known::WhoHistory {
    let home = std::env::var("STVTF_VOICE_HOME").ok().filter(|u| !u.is_empty());
    let sites = std::env::var("STVTF_LINK_SITES").ok().filter(|l| !l.is_empty()).or_else(|| options.home.clone()).unwrap_or_default();
    let sites: Vec<_> = match (keys, voice_key) {
        (Some(keys), Some(_)) => {
            sites.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| web::voice_known::site(s, keys.clone())).collect()
        }
        _ => Vec::new(),
    };
    voice::known::WhoHistory::new(options.sessions.as_deref(), home.as_deref().zip(voice_key), sites)
}

/// What is said is kept in a file too (`--logs`), the relays' notes with it: one for each
/// process. A folder that cannot be written is not worth not starting for: stderr has it all.
fn keep_log(options: &Options) {
    if let Some(logs) = &options.logs
        && let Err(e) = log::keep_in(logs, if options.relay_daemon.is_some() { "relay" } else { "web" })
    {
        log!("no log kept in {}: {e}", logs.display());
    }
    stvtf::relay::notes_to(log::line);
}

fn main() -> Result<()> {
    static STOP: AtomicBool = AtomicBool::new(false);
    static OPEN: AtomicUsize = AtomicUsize::new(0);
    let Some(options) = Options::parse(std::env::args().skip(1))? else {
        println!("{USAGE}");
        return Ok(());
    };
    watch::stop_on_ctrl_c(&STOP)?;
    keep_log(&options);
    // The relay process: only the relays (the rest of the options are the image's for the web).
    if let (Some(socket), Some((host, ports))) = (&options.relay_daemon, &options.relays) {
        log!("relays on {host}, ports {}-{}", ports.start(), ports.end());
        return relays::daemon::serve(socket, &relays::pool::Relays::new(host, ports.clone()), relays::daemon::LEASE);
    }
    let keys = std::env::var("STVTF_TOKEN_KEY").ok().map(|k| web::token::Keys::from_hex(&k).map(Arc::new)).transpose()?;
    let console_prompt = keys.is_none() && std::io::IsTerminal::is_terminal(&std::io::stdin());
    let matches = matches(&options, keys.as_ref(), &STOP)?;
    let store = if let Matches::Many(sessions) = &matches { sessions.store.clone() } else { None };
    let relays = match (&options.relays, &options.relay_socket) {
        (Some(_), _) | (_, Some(_)) if store.is_none() => bail!("relays need --sessions"),
        (Some((host, ports)), _) => Some(Arc::new(relays::pool::Pool::Local(relays::pool::Relays::new(host, ports.clone())))),
        (None, Some(socket)) => Some(Arc::new(relays::pool::Pool::Remote(relays::daemon::Remote::new(socket)))),
        (None, None) => None,
    };
    let voice_key = std::env::var("STVTF_VOICE_KEY").ok().filter(|k| !k.is_empty());
    let voice_who = who_is_who(&options, keys.as_ref(), voice_key.as_deref());
    let app = Arc::new(App {
        matches,
        cache: options.cache.clone(),
        backpack: options.backpack.clone(),
        maps: Mutex::default(),
        overviews: Mutex::new(()),
        base: options.base_path.clone(),
        keys,
        home: options.home.clone(),
        streams: Mutex::default(),
        rooms: rooms::registry::Rooms::new(store, relays).with_maps(options.cache.clone()),
        voice_key: voice_key.clone(),
        voice_links: voice::links::Links::from_env(options.sessions.as_deref()),
        voice_bot: std::env::var("STVTF_VOICE_BOT").ok().filter(|u| !u.is_empty()),
        mumble_changes: voice::mumble::Changes::default(),
        mumble_anywhere: std::env::var("STVTF_MUMBLE_ANYWHERE").is_ok_and(|v| v == "1"),
        voice_who,
    });
    app.voice_who.keep_up();
    app.rooms.sweep(&live_ids(&app.matches));
    if let Matches::Many(_) = app.matches {
        keep_house(app.clone(), &STOP);
        if !options.no_warm {
            let warming = app.clone();
            std::thread::spawn(move || warm(&warming));
        }
        if console_prompt {
            let (console, url) = (app.clone(), format!("http://{}{}", options.listen, app.base));
            std::thread::spawn(move || prompt(&console, &url));
        }
    }
    accept(&options.listen, &app, &OPEN)
}

#[cfg(test)]
mod testing;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matches::hub::publish;
    use crate::testing::{TempDir, app, server};

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
        let dir = TempDir::new("demos");
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

    #[test]
    fn a_server_of_many_matches_starts_with_its_demos_and_the_saved_matches() {
        let (demos, store) = (TempDir::new("manydemos"), TempDir::new("manystore"));
        std::fs::write(demos.join("rd.jsonl"), "").unwrap();
        std::fs::write(demos.join("notes.txt"), "").unwrap();
        let demo = demos.join("rd.jsonl");
        let args = ["--demos", demos.to_str().unwrap(), "--demo", demo.to_str().unwrap(), "--sessions", store.to_str().unwrap()];
        let options = Options::parse(args.map(String::from)).unwrap().unwrap();

        let before = many(&options, None).unwrap();
        testing::script("192.0.2.77:27020", vec![server("cp_restored")], None);
        let running = before.open("192.0.2.77:27020 tv", None, None).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !before.on_map("cp_restored") {
            assert!(std::time::Instant::now() < deadline, "the watcher should have said its map");
            std::thread::sleep(Duration::from_millis(1));
        }
        before.persist();
        before.stop_all();

        let after = many(&options, None).unwrap();
        // Before anyone asks for it by id, which would pick it up anyway.
        assert!(after.on_map("cp_restored"), "picked up");
        assert!(after.get(&running.id).is_some());
        assert!(after.get("rd").is_some() && after.get("demo").is_some(), "the demos are there");
        assert!(after.get("notes").is_none());
        after.stop_all();
    }

    #[test]
    fn a_missing_demo_folder_stops_the_server_from_starting() {
        let dir = TempDir::new("nodemos");
        let missing = dir.join("nothing");
        let options = Options::parse(["--demos".to_string(), missing.to_str().unwrap().to_string()]).unwrap().unwrap();
        assert!(many(&options, None).is_err());
    }
}
