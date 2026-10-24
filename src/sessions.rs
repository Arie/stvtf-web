//! Many matches in one process: a watcher per `SourceTV` server, started when someone asks for
//! it and stopped a while after its last viewer leaves. With a store, the matches outlive the
//! process: a deploy's new container picks them up under the same links.

use crate::cache::write_atomic;
use crate::cli::{DEFAULT_TV_PASSWORD, parse_connect};
use crate::hub::{SharedHub, watch_into};
use crate::{lock, token};
use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
    /// Who started it, for the one-match-per-user limit; None is unlimited.
    owner: Mutex<Option<String>>,
    /// Someone besides its owner opened it: starting another match releases it from the owner's
    /// limit rather than stopping it under the others watching.
    shared: AtomicBool,
    /// A fixed session (the demo): always there, outside the match limit.
    pinned: bool,
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
}

impl Session {
    /// A session not watched yet; a pinned one is never idle, so never reaped.
    fn new(id: String, hub: SharedHub, server: SocketAddr, password: String, owner: Option<String>, pinned: bool) -> Self {
        Session {
            id,
            hub,
            server,
            password,
            owner: Mutex::new(owner),
            shared: AtomicBool::new(false),
            pinned,
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
            viewers: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            views: AtomicUsize::new(0),
            started: Instant::now(),
            idle_since: Mutex::new((!pinned).then(Instant::now)),
            thread: Mutex::new(None),
            saving: Mutex::new(None),
        }
    }

    fn owner(&self) -> Option<String> {
        lock(&self.owner).clone()
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
}

/// A file per match, so the containers of a deploy, briefly running side by side, never write
/// over each other's list. Encrypted with the token key when there is one.
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
    shared: bool,
    #[serde(default)]
    map: Option<String>,
    /// The feed's history lines, as sent to pages.
    #[serde(default)]
    history: Vec<String>,
}

impl Store {
    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// Saves a match, unless it was stopped (ended, or all of them stopped at exit) or its map
    /// and feed are as last saved.
    fn save(&self, s: &Session) -> Result<()> {
        let mut last_saved = lock(&s.saving);
        if s.stop.load(Ordering::Relaxed) {
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
            owner: s.owner(),
            shared: s.shared.load(Ordering::Relaxed),
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

    fn forget(&self, id: &str) {
        let _ = std::fs::remove_file(self.path(id));
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
        Sessions { map: Mutex::default(), public, max, hz, store: None }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        lock(&self.map)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.lock().get(id).cloned()
    }

    /// Whether any match, the demo included, is on `map`.
    pub fn on_map(&self, map: &str) -> bool {
        self.lock().values().any(|s| lock(&s.hub).map.as_deref() == Some(map))
    }

    /// The session for a connect string, joining one already watching that server. An `owner`
    /// watches one match at a time: starting another stops the one they started before, or, if
    /// someone else opened that one too, leaves it to them.
    pub fn open(&self, line: &str, owner: Option<&str>) -> Result<Arc<Session>> {
        let (target, password) = parse_connect(line).ok_or_else(|| anyhow!("no server address"))?;
        let password = password.unwrap_or_else(|| DEFAULT_TV_PASSWORD.into());
        // Connect to the address that was checked, not whatever the name resolves to later.
        let server = stvtf::watch::resolve(&target)?;
        if self.public && !is_public(server.ip()) {
            bail!("{target} is not a public address");
        }
        let mut map = self.lock();
        if let Some(s) = map.values().find(|s| !s.pinned && s.server == server && s.password == password && !s.done.load(Ordering::Relaxed))
        {
            if s.owner().as_deref() != owner {
                s.shared.store(true, Ordering::Relaxed);
            }
            return Ok(s.clone());
        }
        if let Some(o) = owner {
            let previous: Vec<String> = map.values().filter(|s| s.owner().as_deref() == Some(o)).map(|s| s.id.clone()).collect();
            for id in previous {
                if map[&id].shared.load(Ordering::Relaxed) {
                    let p = &map[&id];
                    *lock(&p.owner) = None;
                    // Saved again with no owner, though its feed has not changed.
                    *lock(&p.saving) = None;
                    eprintln!("stvtf-web: match {} released by owner {o}", p.label());
                } else if let Some(p) = map.remove(&id) {
                    self.end(&p, "replaced");
                }
            }
        }
        if map.values().filter(|s| !s.pinned).count() >= self.max {
            bail!("already watching {} matches, the most this server does", self.max);
        }
        let s = self.start(&mut map, random_id(), server, password, owner.map(str::to_string), SharedHub::default());
        eprintln!("stvtf-web: match {} started on {server}, owner {}", s.label(), owner.unwrap_or("-"));
        if let Some(store) = &self.store
            && let Err(e) = store.save(&s)
        {
            eprintln!("stvtf-web: saving match {}: {e:#}", s.id);
        }
        Ok(s)
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
        let (watcher, hz) = (s.clone(), self.hz);
        let thread = std::thread::spawn(move || {
            if let Err(e) = watch_into(&watcher.hub, &server.to_string(), &watcher.password, hz, &watcher.stop) {
                eprintln!("stvtf-web: match {} gave up: {e}", watcher.label());
            }
            watcher.done.store(true, Ordering::Relaxed);
        });
        *lock(&s.thread) = Some(thread);
        map.insert(s.id.clone(), s.clone());
        s
    }

    /// Picks up the matches a previous process saved (the one a deploy replaces), each under its
    /// old link and with its feed so far. Returns how many.
    pub fn restore(&self) -> usize {
        let Some(store) = &self.store else { return 0 };
        let Ok(entries) = std::fs::read_dir(&store.dir) else { return 0 };
        let mut map = self.lock();
        let mut restored = 0;
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.len() != 2 * ID_BYTES || !id.chars().all(|c| c.is_ascii_hexdigit()) || map.contains_key(&id) {
                continue;
            }
            let saved = std::fs::read(entry.path()).map_err(anyhow::Error::from).and_then(|d| store.load(&d));
            let Ok(saved) = saved else {
                eprintln!("stvtf-web: dropping unreadable saved match {id}");
                store.forget(&id);
                continue;
            };
            let hub = SharedHub::default();
            {
                let mut h = lock(&hub);
                h.map = saved.map;
                h.history = saved.history.into_iter().map(Into::into).collect();
            }
            let s = self.start(&mut map, id, saved.server, saved.password, saved.owner, hub);
            s.shared.store(saved.shared, Ordering::Relaxed);
            restored += 1;
        }
        restored
    }

    /// Saves every running match, with its feed so far.
    pub fn persist(&self) {
        let Some(store) = &self.store else { return };
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

/// Random bits from the OS as hex; the link to a match is all it takes to watch it.
fn random_id() -> String {
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
    use crate::testing::TempDir;

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
        let err = s.open("connect 127.0.0.1:27020; password tv", None).err().unwrap().to_string();
        assert!(err.contains("not a public address"), "{err}");
        assert!(s.open("10.0.0.5 tv", None).is_err());
    }

    #[test]
    fn a_user_watches_one_match_at_a_time() {
        // Unreachable but well-formed servers: the watchers just keep retrying until stopped.
        let s = Sessions::new(false, 8, 20.0);
        let a = s.open("192.0.2.1:27020 tv", Some("u1")).unwrap();
        let again = s.open("192.0.2.1:27020 tv", Some("u1")).unwrap();
        assert_eq!(a.id, again.id, "the same match is the same session");
        let b = s.open("192.0.2.2:27020 tv", Some("u1")).unwrap();
        assert!(s.get(&a.id).is_none(), "starting another match stops the first");
        let other = s.open("192.0.2.3:27020 tv", Some("u2")).unwrap();
        let unlimited = s.open("192.0.2.4:27020 tv", None).unwrap();
        assert!(s.get(&b.id).is_some() && s.get(&other.id).is_some() && s.get(&unlimited.id).is_some());
        s.stop_all();
    }

    #[test]
    fn a_match_someone_else_opened_is_left_to_them() {
        let s = Sessions::new(false, 8, 20.0);
        let a = s.open("192.0.2.1:27020 tv", Some("u1")).unwrap();
        // A caster joins u1's match and shares the link.
        assert_eq!(s.open("192.0.2.1:27020 tv", None).unwrap().id, a.id);
        let b = s.open("192.0.2.2:27020 tv", Some("u1")).unwrap();
        assert!(s.get(&a.id).is_some(), "still watched for the caster");
        assert_eq!(a.owner(), None, "and no longer u1's");
        s.open("192.0.2.3:27020 tv", Some("u1")).unwrap();
        assert!(s.get(&b.id).is_none(), "a match only u1 opened is replaced");
        assert!(s.get(&a.id).is_some());
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
    fn a_pinned_session_stays_and_does_not_count() {
        let s = Sessions::new(false, 1, 20.0);
        s.pin("demo", |_| {});
        s.reap(Duration::ZERO);
        assert!(s.get("demo").is_some());
        let m = s.open("192.0.2.1:27020 tv", None).unwrap();
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
        let m = before.open("connect 192.0.2.1:27020; password \"hunter2\"", Some("u1")).unwrap();
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
        assert_eq!((again.server, again.password.as_str(), again.owner().as_deref()), (m.server, "hunter2", Some("u1")));
        assert_eq!(again.hub.lock().unwrap().history.len(), 1, "feed so far");
        after.reap(Duration::ZERO);
        assert!(!dir.join(&m.id).exists(), "a dropped match is forgotten");
        after.stop_all();
    }

    #[test]
    fn only_changed_matches_are_saved_again() {
        let dir = TempDir::new("sessions");
        let mut s = Sessions::new(false, 4, 20.0);
        s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
        let m = s.open("192.0.2.1:27020 tv", None).unwrap();
        std::fs::remove_file(dir.join(&m.id)).unwrap();
        s.persist();
        assert!(!dir.join(&m.id).exists(), "unchanged since it was saved");
        m.hub.lock().unwrap().changes += 1;
        s.persist();
        assert!(dir.join(&m.id).exists());
        s.stop_all();
    }

    #[test]
    fn a_replaced_match_is_not_saved_again() {
        let dir = TempDir::new("sessions");
        let mut s = Sessions::new(false, 4, 20.0);
        s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
        let a = s.open("192.0.2.1:27020 tv", Some("u1")).unwrap();
        assert!(dir.join(&a.id).exists());
        // persist() took `a` from the list just before u1 started another match.
        s.open("192.0.2.2:27020 tv", Some("u1")).unwrap();
        assert!(!dir.join(&a.id).exists(), "the replaced match is forgotten");
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
        assert_eq!((m.password.as_str(), m.owner().as_deref()), ("tv", None));
        let hub = m.hub.lock().unwrap();
        assert_eq!(hub.map.as_deref(), Some("cp_badlands"));
        assert_eq!(hub.history.iter().map(ToString::to_string).collect::<Vec<_>>(), [r#"{"type":"kill"}"#]);
        drop(hub);
        assert!(!dir.join(&broken).exists(), "an unreadable one is dropped");
        s.stop_all();
    }

    #[test]
    fn ids_are_long_and_differ() {
        let (a, b) = (random_id(), random_id());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
