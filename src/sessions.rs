//! Many matches in one process: a watcher per `SourceTV` server, started when someone asks for
//! it and stopped a while after its last viewer leaves. With a store, the matches outlive the
//! process: a deploy's new container picks them up under the same links.

use crate::cache::write_atomic;
use crate::cli::{DEFAULT_TV_PASSWORD, parse_connect};
use crate::hub::{SharedHub, Summary, subscribe, watch_into};
use crate::live_demos::LiveDemos;
use crate::recording::{Recording, Save, Sink};
use crate::relays::DemoOf;
use crate::{demostf, lock, token};
use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A match id: this many random bytes, as hex in links and file names.
const ID_BYTES: usize = 16;
/// How much of the id the log shows.
const LABEL_ID_LEN: usize = 8;

pub struct Session {
    pub id: String,
    pub hub: SharedHub,
    server: SocketAddr,
    password: String,
    /// Who started it (a Steam id from the start token), for the log; it limits nothing.
    owner: Option<String>,
    /// The site's reservation the server belongs to, 0 for none: the latest start link that said.
    reservation: AtomicU64,
    /// A fixed session (the demo): always there, outside the match limit.
    pinned: bool,
    /// The demos.tf demo it plays instead of watching `server`.
    demo: Option<u64>,
    stop: AtomicBool,
    /// Set when the watcher gave up (a wrong password, say) or was stopped.
    done: AtomicBool,
    viewers: AtomicUsize,
    /// For the log: the most viewers at once, and how many streams were opened in all.
    peak: AtomicUsize,
    views: AtomicUsize,
    started: Instant,
    idle_since: Mutex<Option<Instant>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// The hub's `changes` when the match was last saved. Held while it is saved and while it is
    /// ended, so a save that started before the match ended cannot write its file back after
    /// `end` deleted it.
    saving: Mutex<Option<u64>>,
    /// What a room on it plays, once known.
    source: Mutex<Option<Source>>,
    recording: OnceLock<Arc<Recording>>,
    /// A live match's recording, from when it started being watched.
    live: OnceLock<Arc<Recording>>,
    /// Whether `prepare` started.
    preparing: AtomicBool,
    /// A live match's demos, when there is a store to keep them in.
    demos: OnceLock<Arc<LiveDemos>>,
}

/// A recorded match: a saved file, or a demo's gzipped output.
enum Source {
    File(PathBuf),
    Output(Arc<Vec<u8>>),
}

impl Session {
    /// A session not watched yet; a pinned one is never idle, so never reaped.
    fn new(id: String, hub: SharedHub, server: SocketAddr, password: String, owner: Option<String>, pinned: bool) -> Self {
        Session {
            id,
            hub,
            server,
            password,
            owner,
            reservation: AtomicU64::new(0),
            pinned,
            demo: None,
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
            viewers: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            views: AtomicUsize::new(0),
            started: Instant::now(),
            idle_since: Mutex::new((!pinned).then(Instant::now)),
            thread: Mutex::new(None),
            saving: Mutex::new(None),
            source: Mutex::new(None),
            recording: OnceLock::new(),
            live: OnceLock::new(),
            preparing: AtomicBool::new(false),
            demos: OnceLock::new(),
        }
    }

    /// Whether it is a fixed match (`--demo`, `--demos`), there again in the next process.
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// The demos.tf demo it plays, if it plays one.
    pub fn demo_id(&self) -> Option<u64> {
        self.demo
    }

    /// A demo's gzipped stvtf output, once fetched.
    pub fn output(&self) -> Option<Arc<Vec<u8>>> {
        match &*lock(&self.source) {
            Some(Source::Output(gz)) => Some(gz.clone()),
            _ => None,
        }
    }

    /// Where the `.dem` files of what it plays are, by server tick, so rooms on it can be relayed:
    /// a live match's as it records them, or a demos.tf demo's, kept in `store`.
    pub(crate) fn demo_of(&self, store: Option<&Store>) -> Option<DemoOf> {
        if let Some(d) = self.demos.get() {
            let d = d.clone();
            return Some(Arc::new(move |tick| d.at(tick)));
        }
        let path = store?.dir.join(demo_dem(self.demo?));
        Some(Arc::new(move |_| path.exists().then(|| path.clone())))
    }

    /// Rooms play `file`, the recording this session loops.
    pub fn set_file(&self, file: PathBuf) {
        *lock(&self.source) = Some(Source::File(file));
    }

    /// Whether a room can be opened on it now.
    pub fn recordable(&self) -> bool {
        self.live.get().is_some() || self.recording.get().is_some() || lock(&self.source).is_some()
    }

    /// Its recording, if it is ready now: a live match's, or a demo's once read.
    pub fn ready(&self) -> Option<Arc<Recording>> {
        self.live.get().or_else(|| self.recording.get()).cloned()
    }

    /// Reads the room recording in the background, once.
    pub fn prepare(self: &Arc<Self>) {
        if self.recording.get().is_some() || !self.recordable() || self.preparing.swap(true, Ordering::Relaxed) {
            return;
        }
        let s = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = s.recording() {
                eprintln!("stvtf-web: match {}: preparing a room: {e:#}", s.label());
            }
        });
    }

    /// What a room on it plays: read once, then kept.
    pub fn recording(&self) -> Result<Arc<Recording>> {
        if let Some(r) = self.live.get().or_else(|| self.recording.get()) {
            return Ok(r.clone());
        }
        // One read at a time; a caller meanwhile waits and reuses it.
        let source = lock(&self.source);
        if let Some(r) = self.recording.get() {
            return Ok(r.clone());
        }
        let name = self.label();
        let read = match &*source {
            Some(Source::File(path)) => {
                let file = std::fs::File::open(path)?;
                if path.extension().is_some_and(|e| e == "gz") {
                    Recording::read(flate2::read::GzDecoder::new(file), &name)?
                } else {
                    Recording::read(file, &name)?
                }
            }
            Some(Source::Output(gz)) => Recording::read(flate2::read::GzDecoder::new(gz.as_slice()), &name)?,
            None if self.demo.is_some() => bail!("the demo is still being fetched"),
            None => bail!("a live match has no recording"),
        };
        Ok(self.recording.get_or_init(|| Arc::new(read)).clone())
    }

    pub fn reservation(&self) -> Option<u64> {
        Some(self.reservation.load(Ordering::Relaxed)).filter(|&r| r != 0)
    }

    fn link(&self, reservation: Option<u64>) {
        if let Some(r) = reservation {
            self.reservation.store(r, Ordering::Relaxed);
        }
    }

    /// Whether any browser has its event stream open.
    pub fn watched(&self) -> bool {
        self.viewers.load(Ordering::Relaxed) > 0
    }

    /// The start of the id (the whole id is the link) and the map.
    fn label(&self) -> String {
        let map = lock(&self.hub).map.clone();
        format!("{} {}", &self.id[..self.id.len().min(LABEL_ID_LEN)], map.as_deref().unwrap_or("-"))
    }

    /// Not the demo nor a demos.tf demo.
    fn watches_server(&self) -> bool {
        !self.pinned && self.demo.is_none()
    }

    /// Whether it watches a live match now: a room on it records it.
    pub fn is_live(&self) -> bool {
        self.watches_server() && !self.done.load(Ordering::Relaxed)
    }

    fn listed(&self) -> Listed {
        let h = lock(&self.hub);
        Listed {
            id: self.id.clone(),
            reservation: self.reservation(),
            demo: self.demo,
            pinned: self.pinned,
            owner: self.owner.clone(),
            server: self.watches_server().then_some(self.server),
            map: h.map.clone(),
            watching_for: self.started.elapsed().as_secs(),
            viewers: self.viewers.load(Ordering::Relaxed),
            peak: self.peak.load(Ordering::Relaxed),
            views: self.views.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            data_age: h.data_age().map(|d| stvtf::output::round1(d.as_secs_f64())),
            summary: h.summary(),
        }
    }

    fn ended(&self, why: &str) {
        eprintln!(
            "stvtf-web: match {} ended ({why}) after {} min, peak {} viewers, {} views",
            self.label(),
            self.started.elapsed().as_secs() / 60,
            self.peak.load(Ordering::Relaxed),
            self.views.load(Ordering::Relaxed),
        );
    }
}

/// A match as the site's list of matches shows it. Never the password.
#[derive(Debug, Serialize)]
pub struct Listed {
    pub id: String,
    pub reservation: Option<u64>,
    pub demo: Option<u64>,
    pub pinned: bool,
    pub owner: Option<String>,
    /// The server watched; none for a demo.
    pub server: Option<SocketAddr>,
    pub map: Option<String>,
    /// Seconds since this process started watching it.
    pub watching_for: u64,
    pub viewers: usize,
    pub peak: usize,
    pub views: usize,
    /// The watcher gave up or was stopped.
    pub done: bool,
    /// Seconds since the latest snapshot.
    pub data_age: Option<f64>,
    #[serde(flatten)]
    pub summary: Summary,
}

/// Counts a browser on a session's event stream until dropped.
pub struct Viewer(Arc<Session>);

impl Viewer {
    pub fn new(s: Arc<Session>) -> Self {
        let n = s.viewers.fetch_add(1, Ordering::Relaxed) + 1;
        s.peak.fetch_max(n, Ordering::Relaxed);
        s.views.fetch_add(1, Ordering::Relaxed);
        eprintln!("stvtf-web: match {} viewers {n}", s.label());
        *lock(&s.idle_since) = None;
        Viewer(s)
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let n = self.0.viewers.fetch_sub(1, Ordering::Relaxed) - 1;
        eprintln!("stvtf-web: match {} viewers {n}", self.0.label());
        if n == 0 {
            *lock(&self.0.idle_since) = Some(Instant::now());
        }
    }
}

pub struct Sessions {
    map: Mutex<HashMap<String, Arc<Session>>>,
    /// Refuse private and internal addresses: for a server that strangers can use.
    pub public: bool,
    pub max: usize,
    pub hz: f32,
    pub store: Option<Store>,
    /// Keep `.dem` files in the store for rooms' relays (`--relay-ports`): live matches recorded,
    /// demos.tf demos kept.
    pub demos: bool,
    /// This process's mark in the store, once it took the folder over.
    claim: String,
}

/// A file per match, so the containers of a deploy, briefly running side by side, never write
/// over each other's list. Encrypted with the token key when there is one.
///
/// The newest process owns the folder: it writes its mark to [`CURRENT`] on start, and from then
/// on the release it replaces saves only the matches it starts, for the new one to pick up when
/// their pages reconnect to it, and not the others, so it cannot bring back a match the new one
/// ended.
#[derive(Clone)]
pub struct Store {
    pub dir: PathBuf,
    pub keys: Option<Arc<token::Keys>>,
}

/// A running match as saved for the next process: what it takes to watch it again, and its feed.
#[derive(Debug, Serialize, Deserialize)]
struct SavedMatch {
    server: SocketAddr,
    password: String,
    owner: Option<String>,
    #[serde(default)]
    reservation: Option<u64>,
    #[serde(default)]
    map: Option<String>,
    /// The feed's history lines, as sent to pages.
    #[serde(default)]
    history: Vec<String>,
}

/// The file in the store holding the mark of the process it belongs to.
const CURRENT: &str = "current";

impl Store {
    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// Takes the folder over from whichever process had it.
    fn claim(&self, mark: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(write_atomic(&self.path(CURRENT), mark.as_bytes())?)
    }

    /// Whether a newer process took the folder over from the one with `mark`.
    fn taken_over(&self, mark: &str) -> bool {
        std::fs::read(self.path(CURRENT)).is_ok_and(|current| current != mark.as_bytes())
    }

    /// Saves a match, unless it was stopped (ended, or all of them stopped at exit), plays a demo
    /// (the next process could not pick up where it is), or its map and feed are as last saved.
    fn save(&self, s: &Session) -> Result<()> {
        let mut last_saved = lock(&s.saving);
        if s.stop.load(Ordering::Relaxed) || s.demo.is_some() {
            return Ok(());
        }
        let (changes, map, history) = {
            let h = lock(&s.hub);
            if *last_saved == Some(h.changes) {
                return Ok(());
            }
            (h.changes, h.map.clone(), h.history.iter().map(ToString::to_string).collect())
        };
        let saved = SavedMatch {
            server: s.server,
            password: s.password.clone(),
            owner: s.owner.clone(),
            reservation: s.reservation(),
            map,
            history,
        };
        let plain = serde_json::to_vec(&saved)?;
        std::fs::create_dir_all(&self.dir)?;
        let data = self.keys.as_ref().map_or_else(|| plain.clone(), |k| k.seal(&plain));
        write_atomic(&self.path(&s.id), &data)?;
        *last_saved = Some(changes);
        Ok(())
    }

    pub(crate) fn forget(&self, id: &str) {
        let _ = std::fs::remove_file(self.path(id));
    }

    /// Writes `plain` as `name`, sealed when there are keys.
    pub(crate) fn put(&self, name: &str, plain: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let data = self.keys.as_ref().map_or_else(|| plain.to_vec(), |k| k.seal(plain));
        Ok(write_atomic(&self.path(name), &data)?)
    }

    /// Adds `data` to the end of `name`, as it is: not sealed (a match's lines, which `SourceTV`
    /// shows anyone).
    pub(crate) fn append(&self, name: &str, data: &[u8]) -> Result<()> {
        use std::io::Write as _;
        std::fs::create_dir_all(&self.dir)?;
        Ok(std::fs::File::options().create(true).append(true).open(self.path(name))?.write_all(data)?)
    }

    /// `data` as all of `name`, as it is, as for `append`.
    pub(crate) fn replace(&self, name: &str, data: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(write_atomic(&self.path(name), data)?)
    }

    /// What `append` or `replace` wrote as `name`.
    pub(crate) fn read(&self, name: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.path(name))?)
    }

    /// Whether there is a `name`.
    pub(crate) fn has(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// Marks `name` as in use now, so a sweep of old files leaves it.
    pub(crate) fn touch(&self, name: &str) {
        let _ = std::fs::File::options().write(true).open(self.path(name)).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    }

    /// What `put` wrote as `name`.
    pub(crate) fn take(&self, name: &str) -> Result<Vec<u8>> {
        let data = std::fs::read(self.path(name))?;
        Ok(match &self.keys {
            Some(k) => k.unseal(&data)?,
            None => data,
        })
    }

    fn load(&self, data: &[u8]) -> Result<SavedMatch> {
        let plain = match &self.keys {
            Some(k) => k.unseal(data)?,
            None => data.to_vec(),
        };
        Ok(serde_json::from_slice(&plain)?)
    }
}

impl Sessions {
    pub fn new(public: bool, max: usize, hz: f32) -> Self {
        Sessions { map: Mutex::default(), public, max, hz, store: None, demos: false, claim: random_id() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        lock(&self.map)
    }

    /// The session `id`, or one the release this process replaces started after this one took
    /// over its folder, picked up as its page reconnects here.
    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        let mut map = self.lock();
        if let Some(s) = map.get(id) {
            return Some(s.clone());
        }
        let store = self.store.as_ref()?;
        if !is_id(id) || !store.path(id).exists() {
            return None;
        }
        self.pick_up(&mut map, store, id)
    }

    /// Every match, the oldest first.
    pub fn list(&self) -> Vec<Listed> {
        let mut all: Vec<Listed> = self.lock().values().map(|s| s.listed()).collect();
        all.sort_by(|a, b| b.watching_for.cmp(&a.watching_for).then_with(|| a.id.cmp(&b.id)));
        all
    }

    /// Whether any match, the demo included, is on `map`.
    pub fn on_map(&self, map: &str) -> bool {
        self.lock().values().any(|s| lock(&s.hub).map.as_deref() == Some(map))
    }

    /// The session for a connect string, joining one already watching that server. `owner`, who
    /// asked for it, is only logged: anyone may watch any number of matches. `reservation` links
    /// the match to the site's reservation for the list of matches.
    pub fn open(&self, line: &str, owner: Option<&str>, reservation: Option<u64>) -> Result<Arc<Session>> {
        let (target, password) = parse_connect(line).ok_or_else(|| anyhow!("no server address"))?;
        let password = password.unwrap_or_else(|| DEFAULT_TV_PASSWORD.into());
        // Connect to the address that was checked, not whatever the name resolves to later.
        let server = stvtf::watch::resolve(&target)?;
        if self.public && !is_public(server.ip()) {
            bail!("{target} is not a public address");
        }
        let mut map = self.lock();
        if let Some(s) =
            map.values().find(|s| s.watches_server() && s.server == server && s.password == password && !s.done.load(Ordering::Relaxed))
        {
            s.link(reservation);
            return Ok(join(s, owner));
        }
        self.check_room(&map)?;
        let s = self.start(&mut map, random_id(), server, password, owner.map(str::to_string), SharedHub::default());
        record(&s, self.store.as_ref(), false);
        s.link(reservation);
        eprintln!("stvtf-web: match {} started on {server}, owner {}", s.label(), owner.unwrap_or("-"));
        if let Some(store) = &self.store
            && let Err(e) = store.save(&s)
        {
            eprintln!("stvtf-web: saving match {}: {e:#}", s.id);
        }
        Ok(s)
    }

    /// The session playing demos.tf demo `demo`, joining one already playing it (where it is now)
    /// or starting it from the beginning; `owner` as for [`Sessions::open`]. It is not saved for
    /// the next process.
    pub fn open_demo(&self, demo: u64, owner: Option<&str>) -> Result<Arc<Session>> {
        let mut map = self.lock();
        if let Some(s) = map.values().find(|s| s.demo == Some(demo) && !s.done.load(Ordering::Relaxed)) {
            return Ok(join(s, owner));
        }
        self.check_room(&map)?;
        let nowhere = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
        let mut session = Session::new(demo_id(demo), SharedHub::default(), nowhere, String::new(), owner.map(str::to_string), false);
        session.demo = Some(demo);
        let s = Arc::new(session);
        let (player, hz, store) = (s.clone(), self.hz, self.store.clone().filter(|_| self.demos));
        let thread = std::thread::spawn(move || {
            let keep_demo = |dem: &[u8]| {
                if let Some(store) = &store
                    && let Err(e) = store.replace(&demo_dem(demo), dem)
                {
                    eprintln!("stvtf-web: keeping demo {demo}: {e:#}");
                }
            };
            if let Err(e) = demostf::play(&player.hub, demo, hz, &|| player.watched(), &player.stop, &keep_demo, &|gz| {
                *lock(&player.source) = Some(Source::Output(gz));
            }) {
                eprintln!("stvtf-web: match {} (demo {demo}) gave up: {e:#}", player.label());
            }
            player.done.store(true, Ordering::Relaxed);
        });
        *lock(&s.thread) = Some(thread);
        map.insert(s.id.clone(), s.clone());
        eprintln!("stvtf-web: match {} started on demo {demo}, owner {}", s.label(), owner.unwrap_or("-"));
        Ok(s)
    }

    /// Matches that count towards `max`.
    pub fn running(&self) -> usize {
        self.lock().values().filter(|s| !s.pinned).count()
    }

    /// Whether another match may start: there must be fewer than `max`.
    fn check_room(&self, map: &HashMap<String, Arc<Session>>) -> Result<()> {
        if map.values().filter(|s| !s.pinned).count() >= self.max {
            bail!("already watching {} matches, the most this server does", self.max);
        }
        Ok(())
    }

    /// Watches `server` as session `id`.
    fn start(
        &self,
        map: &mut HashMap<String, Arc<Session>>,
        id: String,
        server: SocketAddr,
        password: String,
        owner: Option<String>,
        hub: SharedHub,
    ) -> Arc<Session> {
        let s = Arc::new(Session::new(id, hub, server, password, owner, false));
        let demos = self.store.as_ref().filter(|_| self.demos).map(|store| LiveDemos::new(&store.dir, &s.id));
        if let Some(d) = &demos {
            let _ = s.demos.set(d.clone());
        }
        let (watcher, hz) = (s.clone(), self.hz);
        let thread = std::thread::spawn(move || {
            let demos = demos.map(|d| d as Arc<dyn stvtf::demo::Files>);
            if let Err(e) = watch_into(&watcher.hub, &server.to_string(), &watcher.password, hz, demos, &watcher.stop) {
                eprintln!("stvtf-web: match {} gave up: {e}", watcher.label());
            }
            watcher.done.store(true, Ordering::Relaxed);
        });
        *lock(&s.thread) = Some(thread);
        map.insert(s.id.clone(), s.clone());
        s
    }

    /// Takes the folder over and picks up the matches a previous process saved (the one a deploy
    /// replaces), each under its old link and with its feed so far. Returns how many.
    pub fn restore(&self) -> usize {
        let Some(store) = &self.store else { return 0 };
        if let Err(e) = store.claim(&self.claim) {
            eprintln!("stvtf-web: taking over {}: {e:#}", store.dir.display());
        }
        let Ok(entries) = std::fs::read_dir(&store.dir) else { return 0 };
        let mut map = self.lock();
        let mut restored = 0;
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if is_id(&id) && !map.contains_key(&id) && self.pick_up(&mut map, store, &id).is_some() {
                restored += 1;
            }
        }
        restored
    }

    /// Watches the match saved as `id` again, or drops its file if it cannot be read.
    fn pick_up(&self, map: &mut HashMap<String, Arc<Session>>, store: &Store, id: &str) -> Option<Arc<Session>> {
        let saved = std::fs::read(store.path(id)).map_err(anyhow::Error::from).and_then(|d| store.load(&d));
        let Ok(saved) = saved else {
            eprintln!("stvtf-web: dropping unreadable saved match {id}");
            store.forget(id);
            return None;
        };
        let hub = SharedHub::default();
        {
            let mut h = lock(&hub);
            h.map = saved.map;
            h.history = saved.history.into_iter().map(Into::into).collect();
        }
        let s = self.start(map, id.to_string(), saved.server, saved.password, saved.owner, hub);
        record(&s, Some(store), true);
        s.link(saved.reservation);
        Some(s)
    }

    /// Saves every running match, with its feed so far, unless a newer process took the store
    /// over: it has these matches already, apart from the ones it ended.
    pub fn persist(&self) {
        let Some(store) = &self.store else { return };
        if store.taken_over(&self.claim) {
            return;
        }
        let running: Vec<Arc<Session>> = self.lock().values().filter(|s| !s.pinned).cloned().collect();
        for s in running {
            if let Err(e) = store.save(&s) {
                eprintln!("stvtf-web: saving match {}: {e:#}", s.id);
            }
        }
    }

    /// A fixed session at `/w/<id>/` fed by `feed` (a recorded match on a loop, say). It is never
    /// reaped and nothing waits for its thread.
    pub fn pin(&self, id: &str, feed: impl FnOnce(&Session) + Send + 'static) {
        // It watches no server.
        let nowhere = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
        let s = Arc::new(Session::new(id.to_string(), SharedHub::default(), nowhere, String::new(), None, true));
        let session = s.clone();
        std::thread::spawn(move || feed(&session));
        self.lock().insert(id.to_string(), s);
    }

    /// Stops and forgets the sessions nobody has watched for `linger`. Their watchers disconnect
    /// on their own; nothing waits for them.
    pub fn reap(&self, linger: Duration) {
        let idle: Vec<Arc<Session>> = {
            let mut map = self.lock();
            let idle: Vec<String> = map
                .values()
                .filter(|s| !s.pinned)
                .filter(|s| lock(&s.idle_since).is_some_and(|t| t.elapsed() >= linger))
                .map(|s| s.id.clone())
                .collect();
            idle.iter().filter_map(|id| map.remove(id)).collect()
        };
        for s in idle {
            self.end(&s, "unwatched");
        }
    }

    /// Stops a session already taken out of the map, and forgets its saved file.
    fn end(&self, s: &Session, why: &str) {
        // Stopped once a save in progress is done, so no save writes the file again once forgotten.
        {
            let _saving = lock(&s.saving);
            s.stop.store(true, Ordering::Relaxed);
        }
        s.ended(why);
        if let Some(store) = &self.store {
            store.forget(&s.id);
            store.forget(&live_file(&s.id));
        }
        if let Some(d) = s.demos.get() {
            d.forget();
        }
    }

    /// Disconnects every watcher cleanly, so the servers free their `SourceTV` slots. Their saved
    /// files stay for the process that takes over.
    pub fn stop_all(&self) {
        let all: Vec<Arc<Session>> = self.lock().drain().map(|(_, s)| s).collect();
        for s in &all {
            s.stop.store(true, Ordering::Relaxed);
        }
        for s in all {
            if let Some(t) = lock(&s.thread).take() {
                let _ = t.join();
            }
        }
    }
}

/// A demos.tf demo's file in the store, kept while a match or room plays it.
pub(crate) fn demo_dem(demo: u64) -> String {
    format!("demo-{demo}.dem")
}

/// A live match's recording beside its file.
pub(crate) fn live_file(id: &str) -> String {
    format!("live-{id}.gz")
}

/// Chunks to the end of file `name`, or all of it anew; the first that fails says so.
pub(crate) fn to_file(store: &Store, name: &str) -> Sink {
    let (store, name, mut said) = (store.clone(), name.to_string(), false);
    Box::new(move |save: &Save| {
        let saved = match save {
            Save::Add(gz) => store.append(&name, gz),
            Save::All(gz) => store.replace(&name, gz),
        };
        if let Err(e) = saved
            && !std::mem::replace(&mut said, true)
        {
            eprintln!("stvtf-web: saving {name}: {e:#}");
        }
    })
}

/// How long the recorder waits for a line before it looks whether to stop.
const RECORD_POLL: Duration = Duration::from_millis(250);

/// Records live match `s` from now on, to its file when there is a store (`resume`: going on from
/// what the process before saved), until the match ends. The feed from before is not recorded
/// again as new.
fn record(s: &Arc<Session>, store: Option<&Store>, resume: bool) {
    let name = live_file(&s.id);
    let recording = Arc::new(match store {
        None => Recording::live(),
        Some(store) => {
            let saved = resume.then(|| store.read(&name).ok()).flatten();
            saved.and_then(|gz| Recording::resume(gz.as_slice(), to_file(store, &name)).ok()).unwrap_or_else(|| {
                store.forget(&name);
                Recording::live_to(to_file(store, &name))
            })
        }
    });
    let _ = s.live.set(recording.clone());
    let (lines, s) = (subscribe(&s.hub, true), s.clone());
    std::thread::spawn(move || {
        while s.is_live() && !s.stop.load(Ordering::Relaxed) {
            let json = match lines.recv_timeout(RECORD_POLL) {
                Ok(json) => json,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let Ok(mut stamped) = serde_json::from_str::<stvtf::output::Stamped>(&json) else { continue };
            if json.contains(r#""history":true"#) {
                continue;
            }
            stamped.time = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64));
            let _ = recording.push(&stamped, &json);
        }
        recording.end();
    });
}

/// Joins `s`, a match already running, for `owner`; logged, so the log shows everyone who asked
/// for a match and not only who started it.
fn join(s: &Arc<Session>, owner: Option<&str>) -> Arc<Session> {
    eprintln!("stvtf-web: match {} opened, owner {}", s.label(), owner.unwrap_or("-"));
    s.clone()
}

/// Demo `demo`'s match id, the same each time it is opened: its page's link lasts.
pub(crate) fn demo_id(demo: u64) -> String {
    format!("d{demo}")
}

/// The demo whose match id `id` is, by [`demo_id`].
pub(crate) fn id_demo(id: &str) -> Option<u64> {
    id.strip_prefix('d').filter(|n| n.bytes().all(|b| b.is_ascii_digit())).and_then(|n| n.parse().ok())
}

/// Whether `name` is a match id, as made by [`random_id`].
pub(crate) fn is_id(name: &str) -> bool {
    name.len() == 2 * ID_BYTES && name.chars().all(|c| c.is_ascii_hexdigit())
}

/// Random bits from the OS as hex; the link to a match is all it takes to watch it.
pub(crate) fn random_id() -> String {
    let mut id = [0u8; ID_BYTES];
    OsRng.fill_bytes(&mut id);
    id.iter().fold(String::with_capacity(2 * ID_BYTES), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// IPv4 ranges that are not on the internet and that `Ipv4Addr`'s own checks leave out: "this
/// network", carrier-grade NAT (Tailscale), IETF protocol assignments, benchmarking and reserved.
const NOT_PUBLIC_V4: [(Ipv4Addr, u32); 5] = [
    (Ipv4Addr::UNSPECIFIED, 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// Whether `ip` is in the network `net`/`prefix`.
fn in_network(ip: Ipv4Addr, (net, prefix): (Ipv4Addr, u32)) -> bool {
    ip.to_bits() >> (u32::BITS - prefix) == net.to_bits() >> (u32::BITS - prefix)
}

/// Not loopback, private, link-local, carrier-grade NAT (Tailscale), IETF protocol
/// assignments, benchmarking, multicast or reserved.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || NOT_PUBLIC_V4.into_iter().any(|net| in_network(v4, net)))
        }
        IpAddr::V6(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, script, server};

    #[test]
    fn only_public_addresses_pass() {
        let ok = |s: &str| is_public(s.parse().unwrap());
        assert!(ok("141.94.96.119"));
        assert!(ok("104.128.58.43"));
        for internal in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.12",
            "100.64.0.1",
            "100.100.100.100",
            "169.254.1.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "192.0.0.8",
            "198.18.0.1",
            "198.19.255.1",
            "::1",
        ] {
            assert!(!ok(internal), "{internal}");
        }
    }

    #[test]
    fn public_sessions_refuse_internal_servers() {
        let s = Sessions::new(true, 4, 20.0);
        let err = s.open("connect 127.0.0.1:27020; password tv", None, None).err().unwrap().to_string();
        assert!(err.contains("not a public address"), "{err}");
        assert!(s.open("10.0.0.5 tv", None, None).is_err());
    }

    #[test]
    fn a_user_may_watch_any_number_of_matches() {
        let s = Sessions::new(false, 8, 20.0);
        let a = s.open("192.0.2.1:27020 tv", Some("u1"), None).unwrap();
        let again = s.open("192.0.2.1:27020 tv", Some("u2"), None).unwrap();
        assert_eq!(a.id, again.id, "the same match is the same session");
        assert_eq!(again.owner.as_deref(), Some("u1"), "still the one who started it");
        let b = s.open("192.0.2.2:27020 tv", Some("u1"), None).unwrap();
        assert!(s.get(&a.id).is_some() && s.get(&b.id).is_some(), "starting another match stops none");
        s.stop_all();
    }

    #[test]
    fn the_list_says_what_is_watched_and_never_the_password() {
        let s = Sessions::new(false, 8, 20.0);
        s.pin("demo", |_| {});
        let m = s.open("connect 192.0.2.1:27020; password \"hunter2\"", Some("u1"), Some(9)).unwrap();
        let _viewer = Viewer::new(m.clone());
        let list = s.list();
        let listed = list.iter().find(|l| l.id == m.id).unwrap();
        assert_eq!((listed.reservation, listed.owner.as_deref(), listed.viewers, listed.peak), (Some(9), Some("u1"), 1, 1));
        assert_eq!(listed.server, Some("192.0.2.1:27020".parse().unwrap()));
        let demo = list.iter().find(|l| l.id == "demo").unwrap();
        assert!(demo.pinned && demo.server.is_none());
        assert!(!serde_json::to_string(&list).unwrap().contains("hunter2"));
        s.stop_all();
    }

    #[test]
    fn a_start_link_with_a_reservation_links_the_match() {
        let s = Sessions::new(false, 8, 20.0);
        let pasted = s.open("192.0.2.1:27020 tv", None, None).unwrap();
        assert_eq!(pasted.reservation(), None);
        s.open("192.0.2.1:27020 tv", None, Some(7)).unwrap();
        assert_eq!(pasted.reservation(), Some(7), "joining from the reservation links it");
        s.open("192.0.2.1:27020 tv", None, None).unwrap();
        assert_eq!(pasted.reservation(), Some(7), "a pasted link keeps the link");
        assert_eq!(s.open("192.0.2.2:27020 tv", None, Some(8)).unwrap().reservation(), Some(8));
        s.stop_all();
    }

    #[test]
    fn a_demo_plays_once_for_everyone_who_opens_it() {
        // Waiting for their turn to fetch, the demos never reach demos.tf.
        let fetching = lock(&demostf::FETCHING);
        let dir = TempDir::new("demo-sessions");
        let mut s = Sessions::new(false, 3, 20.0);
        s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
        let a = s.open_demo(1, Some("u1")).unwrap();
        assert_eq!(s.open_demo(1, Some("u2")).unwrap().id, a.id, "joined where it is");
        let server = s.open("192.0.2.1:27020 tv", None, None).unwrap();
        assert_ne!(server.id, a.id);
        s.open_demo(2, Some("u3")).unwrap();
        let full = s.open_demo(3, Some("u3")).err().unwrap().to_string();
        assert!(full.contains("already watching 3"), "demos count towards the limit: {full}");
        s.persist();
        assert!(!dir.join(&a.id).exists(), "the next process could not pick a demo up where it is");
        for m in s.lock().values() {
            m.stop.store(true, Ordering::Relaxed);
        }
        drop(fetching);
        s.stop_all();
    }

    #[test]
    fn a_session_knows_whether_it_is_watched() {
        let s = Sessions::new(false, 1, 20.0);
        s.pin("demo", |_| {});
        let demo = s.get("demo").unwrap();
        assert!(!demo.watched());
        let viewer = Viewer::new(demo.clone());
        assert!(demo.watched());
        drop(viewer);
        assert!(!demo.watched());
    }

    #[test]
    fn a_pinned_demo_can_be_recorded() {
        let dir = TempDir::new("recorded");
        let path = dir.join("demo.jsonl");
        std::fs::write(&path, "{\"type\":\"stalemate\",\"tick\":5}\n{\"type\":\"stalemate\",\"tick\":55}\n").unwrap();
        let s = Sessions::new(false, 1, 20.0);
        s.pin("demo", move |s| s.set_file(path));
        let demo = s.get("demo").unwrap();
        while !demo.recordable() {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(demo.recording().unwrap().length(), 750);
        assert!(Arc::ptr_eq(&demo.recording().unwrap(), &demo.recording().unwrap()), "read once");
    }

    #[test]
    fn a_demo_s_recording_is_prepared_in_the_background() {
        let dir = TempDir::new("prepared");
        let path = dir.join("demo.jsonl");
        std::fs::write(&path, "{\"type\":\"stalemate\",\"tick\":5}\n{\"type\":\"stalemate\",\"tick\":55}\n").unwrap();
        let s = Sessions::new(false, 1, 20.0);
        s.pin("demo", move |s| s.set_file(path));
        let demo = s.get("demo").unwrap();
        while !demo.recordable() {
            std::thread::sleep(Duration::from_millis(1));
        }
        demo.prepare();
        let started = Instant::now();
        while demo.recording.get().is_none() {
            assert!(started.elapsed() < Duration::from_secs(5), "prepared");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(Arc::ptr_eq(&demo.recording().unwrap(), demo.recording.get().unwrap()), "the room plays what was prepared");
    }

    #[test]
    fn a_live_match_is_recorded_from_the_start() {
        let s = Sessions::new(false, 1, 20.0);
        let m = s.open("192.0.2.1:27020 tv", None, None).unwrap();
        assert!(m.recordable());
        assert!(m.recording().unwrap().is_live());
        s.stop_all();
    }

    #[test]
    fn a_pinned_session_stays_and_does_not_count() {
        let s = Sessions::new(false, 1, 20.0);
        s.pin("demo", |_| {});
        s.reap(Duration::ZERO);
        assert!(s.get("demo").is_some());
        let m = s.open("192.0.2.1:27020 tv", None, None).unwrap();
        assert!(s.get(&m.id).is_some(), "the demo is outside the match limit");
        s.stop_all();
    }

    #[test]
    fn matches_survive_into_the_next_process() {
        let dir = TempDir::new("sessions");
        let keys = Arc::new(token::Keys::from_hex(&"42".repeat(32)).unwrap());
        let store = || Some(Store { dir: dir.to_path_buf(), keys: Some(keys.clone()) });
        let mut before = Sessions::new(false, 4, 20.0);
        before.store = store();
        let m = before.open("connect 192.0.2.1:27020; password \"hunter2\"", Some("u1"), Some(42)).unwrap();
        {
            let mut h = m.hub.lock().unwrap();
            h.history.push_back(r#"{"type":"kill","history":true}"#.into());
            h.changes += 1;
        }
        before.persist();
        let file = std::fs::read(dir.join(&m.id)).unwrap();
        assert!(!String::from_utf8_lossy(&file).contains("hunter2"), "saved encrypted");
        before.stop_all();

        let mut after = Sessions::new(false, 4, 20.0);
        after.store = store();
        assert_eq!(after.restore(), 1);
        let again = after.get(&m.id).expect("same link");
        assert_eq!((again.server, again.password.as_str(), again.owner.as_deref()), (m.server, "hunter2", Some("u1")));
        assert_eq!(again.hub.lock().unwrap().history.len(), 1, "feed so far");
        assert_eq!(again.reservation(), Some(42));
        after.reap(Duration::ZERO);
        assert!(!dir.join(&m.id).exists(), "a dropped match is forgotten");
        after.stop_all();
    }

    #[test]
    fn only_changed_matches_are_saved_again() {
        let dir = TempDir::new("sessions");
        let mut s = Sessions::new(false, 4, 20.0);
        s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
        let m = s.open("192.0.2.1:27020 tv", None, None).unwrap();
        std::fs::remove_file(dir.join(&m.id)).unwrap();
        s.persist();
        assert!(!dir.join(&m.id).exists(), "unchanged since it was saved");
        m.hub.lock().unwrap().changes += 1;
        s.persist();
        assert!(dir.join(&m.id).exists());
        s.stop_all();
    }

    #[test]
    fn an_ended_match_is_not_saved_again() {
        let dir = TempDir::new("sessions");
        let mut s = Sessions::new(false, 4, 20.0);
        s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
        let a = s.open("192.0.2.1:27020 tv", Some("u1"), None).unwrap();
        assert!(dir.join(&a.id).exists());
        // persist() took `a` from the list just before it was reaped.
        s.reap(Duration::ZERO);
        assert!(!dir.join(&a.id).exists(), "the ended match is forgotten");
        s.store.as_ref().unwrap().save(&a).unwrap();
        assert!(!dir.join(&a.id).exists(), "and stays forgotten");
        s.stop_all();
    }

    #[test]
    fn a_match_saved_by_the_running_release_is_picked_up() {
        let dir = TempDir::new("sessions");
        let (id, broken) = (random_id(), random_id());
        let saved = r#"{"server":"192.0.2.1:27020","password":"tv","owner":null,"map":"cp_badlands","history":["{\"type\":\"kill\"}"]}"#;
        std::fs::write(dir.join(&id), saved).unwrap();
        std::fs::write(dir.join(&broken), r#"{"server":"192.0.2.1:27020"}"#).unwrap();
        let mut s = Sessions::new(false, 4, 20.0);
        s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
        assert_eq!(s.restore(), 1);
        let m = s.get(&id).expect("same link");
        assert_eq!((m.password.as_str(), m.owner.as_deref()), ("tv", None));
        let hub = m.hub.lock().unwrap();
        assert_eq!(hub.map.as_deref(), Some("cp_badlands"));
        assert_eq!(hub.history.iter().map(ToString::to_string).collect::<Vec<_>>(), [r#"{"type":"kill"}"#]);
        drop(hub);
        assert!(!dir.join(&broken).exists(), "an unreadable one is dropped");
        s.stop_all();
    }

    /// A process starting on `dir`, as a deploy's new release does, and how many matches it took over.
    fn start_on(dir: &TempDir) -> (Sessions, usize) {
        let mut s = Sessions::new(false, 4, 20.0);
        s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
        let restored = s.restore();
        (s, restored)
    }

    #[test]
    fn a_match_started_by_the_old_release_is_picked_up_when_asked_for() {
        let dir = TempDir::new("sessions");
        let ((old, _), (new, _)) = (start_on(&dir), start_on(&dir));
        // Started after the new release read the folder, before the proxy switched to it.
        let m = old.open("192.0.2.1:27020 tv", Some("u1"), None).unwrap();
        let again = new.get(&m.id).expect("the page reconnecting to the new release finds it");
        assert_eq!((again.server, again.owner.as_deref()), (m.server, Some("u1")));
        assert!(new.get(&random_id()).is_none(), "a match nobody saved is still unknown");
        assert!(new.get("../current").is_none(), "only match ids are looked up");
        old.stop_all();
        new.stop_all();
    }

    #[test]
    fn a_release_taken_over_leaves_the_saved_matches_alone() {
        let dir = TempDir::new("sessions");
        let (old, _) = start_on(&dir);
        let m = old.open("192.0.2.1:27020 tv", Some("u1"), None).unwrap();
        let (new, restored) = start_on(&dir);
        assert_eq!(restored, 1);
        // Nobody watches it once the proxy switched: the new release ends it.
        new.reap(Duration::ZERO);
        assert!(!dir.join(&m.id).exists());
        // The old release still has it, with lines it got since its last save, and saves on SIGTERM.
        m.hub.lock().unwrap().changes += 1;
        old.persist();
        assert!(!dir.join(&m.id).exists(), "not brought back for the next deploy");
        old.stop_all();
        new.stop_all();
    }

    #[test]
    fn ids_are_long_and_differ() {
        let (a, b) = (random_id(), random_id());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn a_match_whose_watcher_gave_up_is_started_again() {
        let s = Sessions::new(false, 8, 20.0);
        script("192.0.2.200:27020", vec![server("cp_process_f12")], Some("wrong password"));
        let first = s.open("192.0.2.200:27020 tv", None, None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !first.done.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "the watcher should have given up");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(s.on_map("cp_process_f12"), "what it saw before giving up is kept");
        let second = s.open("192.0.2.200:27020 tv", None, None).unwrap();
        assert_ne!(first.id, second.id);
        s.stop_all();
    }

    #[test]
    fn a_match_is_idle_only_once_its_last_viewer_left() {
        let s = Sessions::new(false, 8, 20.0);
        let m = s.open("192.0.2.50:27020 tv", None, None).unwrap();
        let (a, b) = (Viewer::new(m.clone()), Viewer::new(m.clone()));
        s.reap(Duration::ZERO);
        assert!(s.get(&m.id).is_some(), "two are watching");
        drop(a);
        s.reap(Duration::ZERO);
        assert!(s.get(&m.id).is_some(), "one is still watching");
        drop(b);
        s.reap(Duration::from_hours(1));
        assert!(s.get(&m.id).is_some(), "it lingers");
        s.reap(Duration::ZERO);
        assert!(s.get(&m.id).is_none());
    }

    #[test]
    fn only_match_ids_are_looked_up_in_the_store() {
        let root = TempDir::new("storeids");
        let dir = root.join("store");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join(CURRENT), "mark").unwrap();
        std::fs::write(root.join("outside"), "x").unwrap();
        let mut s = Sessions::new(false, 8, 20.0);
        s.store = Some(Store { dir: dir.clone(), keys: None });
        for name in [CURRENT, "../outside"] {
            assert!(s.get(name).is_none(), "{name}");
        }
        // Anything picked up and found unreadable would have been deleted.
        assert_eq!(std::fs::read(dir.join(CURRENT)).unwrap(), b"mark");
        assert!(root.join("outside").exists());
        let unreadable = "0".repeat(2 * ID_BYTES);
        std::fs::write(dir.join(&unreadable), "not a match").unwrap();
        assert!(s.get(&unreadable).is_none());
        assert!(!dir.join(&unreadable).exists(), "a saved match that cannot be read is dropped");
    }

    #[test]
    fn a_match_id_is_exactly_as_long_as_they_are_made_and_hex() {
        assert!(is_id(&random_id()));
        assert_eq!(id_demo(&demo_id(1_531_482)), Some(1_531_482));
        assert_eq!(id_demo("d"), None);
        for bad in ["", "abc", &"a".repeat(31), &"a".repeat(33), &"g".repeat(32), &format!("../{}", "a".repeat(29))] {
            assert!(!is_id(bad), "{bad}");
        }
    }

    #[test]
    fn documentation_and_broadcast_addresses_are_not_public() {
        for ip in ["192.0.2.1", "198.51.100.1", "203.0.113.1", "255.255.255.255"] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn stopping_all_waits_for_every_watcher_to_finish() {
        let s = Sessions::new(false, 8, 20.0);
        let (a, b) = (s.open("192.0.2.60:27020 tv", None, None).unwrap(), s.open("192.0.2.61:27020 tv", None, None).unwrap());
        s.stop_all();
        assert!(a.done.load(Ordering::Relaxed) && b.done.load(Ordering::Relaxed));
        assert!(s.get(&a.id).is_none() && s.get(&b.id).is_none());
    }
}
