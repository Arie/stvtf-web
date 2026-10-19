//! Many matches in one process: a watcher per `SourceTV` server, started when someone asks for
//! it and stopped a while after its last viewer leaves.

use crate::{SharedHub, parse_connect, publish};
use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use stvmap::watch::{self, Watch};

pub struct Session {
    pub id: String,
    pub hub: SharedHub,
    server: SocketAddr,
    password: String,
    /// Who started it, for the one-match-per-user limit; None is unlimited.
    owner: Option<String>,
    /// A fixed session (the demo): always there, outside the match limit.
    pinned: bool,
    stop: AtomicBool,
    /// Set when the watcher gave up (a wrong password, say) or was stopped.
    done: AtomicBool,
    viewers: AtomicUsize,
    idle_since: Mutex<Option<Instant>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Session {
    /// Whether any browser has its event stream open.
    pub fn watched(&self) -> bool {
        self.viewers.load(Ordering::Relaxed) > 0
    }
}

/// Counts a browser on a session's event stream until dropped.
pub struct Viewer(Arc<Session>);

impl Viewer {
    pub fn new(s: Arc<Session>) -> Self {
        s.viewers.fetch_add(1, Ordering::Relaxed);
        *s.idle_since.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Viewer(s)
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        if self.0.viewers.fetch_sub(1, Ordering::Relaxed) == 1 {
            *self.0.idle_since.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        }
    }
}

pub struct Sessions {
    map: Mutex<HashMap<String, Arc<Session>>>,
    /// Refuse private and internal addresses: for a server that strangers can use.
    pub public: bool,
    pub max: usize,
    pub hz: f32,
}

impl Sessions {
    pub fn new(public: bool, max: usize, hz: f32) -> Self {
        Sessions { map: Mutex::default(), public, max, hz }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.lock().get(id).cloned()
    }

    /// The session for a connect string, joining one already watching that server. An `owner`
    /// watches one match at a time: starting another stops the one they started before.
    pub fn open(&self, line: &str, owner: Option<&str>) -> Result<Arc<Session>> {
        let (target, password) = parse_connect(line).ok_or_else(|| anyhow!("no server address"))?;
        let password = password.unwrap_or_else(|| "tv".into());
        // Connect to the address that was checked, not whatever the name resolves to later.
        let server = target
            .to_socket_addrs()
            .map_err(|e| anyhow!("{target}: {e}"))?
            .find(SocketAddr::is_ipv4)
            .ok_or_else(|| anyhow!("no IPv4 address for {target}"))?;
        if self.public && !is_public(server.ip()) {
            bail!("{target} is not a public address");
        }
        let mut map = self.lock();
        if let Some(s) = map.values().find(|s| !s.pinned && s.server == server && s.password == password && !s.done.load(Ordering::Relaxed))
        {
            return Ok(s.clone());
        }
        if let Some(o) = owner {
            let previous: Vec<String> = map.values().filter(|s| s.owner.as_deref() == Some(o)).map(|s| s.id.clone()).collect();
            for id in previous {
                if let Some(p) = map.remove(&id) {
                    p.stop.store(true, Ordering::Relaxed);
                }
            }
        }
        if map.values().filter(|s| !s.pinned).count() >= self.max {
            bail!("already watching {} matches, the most this server does", self.max);
        }
        let s = Arc::new(Session {
            id: random_id(),
            hub: SharedHub::default(),
            server,
            password,
            owner: owner.map(str::to_string),
            pinned: false,
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
            viewers: AtomicUsize::new(0),
            idle_since: Mutex::new(Some(Instant::now())),
            thread: Mutex::new(None),
        });
        let (watcher, hz) = (s.clone(), self.hz);
        let thread = std::thread::spawn(move || {
            let mut w = Watch::new(&watcher.server.to_string(), &watcher.password);
            w.snapshot_hz = hz;
            if let Err(e) = watch::watch(&w, &watcher.stop, &mut |v| publish(&watcher.hub, &v)) {
                publish(&watcher.hub, &serde_json::json!({"type": "status", "state": "gave_up", "reason": e.to_string()}));
            }
            watcher.done.store(true, Ordering::Relaxed);
        });
        *s.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread);
        map.insert(s.id.clone(), s.clone());
        Ok(s)
    }

    /// A fixed session at `/w/<id>/` fed by `feed` (a recorded match on a loop, say). It is never
    /// reaped and nothing waits for its thread.
    pub fn pin(&self, id: &str, feed: impl FnOnce(&Session) + Send + 'static) {
        let s = Arc::new(Session {
            id: id.to_string(),
            hub: SharedHub::default(),
            server: SocketAddr::from(([0, 0, 0, 0], 0)),
            password: String::new(),
            owner: None,
            pinned: true,
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
            viewers: AtomicUsize::new(0),
            idle_since: Mutex::new(None),
            thread: Mutex::new(None),
        });
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
                .filter(|s| s.idle_since.lock().unwrap_or_else(PoisonError::into_inner).is_some_and(|t| t.elapsed() >= linger))
                .map(|s| s.id.clone())
                .collect();
            idle.iter().filter_map(|id| map.remove(id)).collect()
        };
        for s in idle {
            s.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Disconnects every watcher cleanly, so the servers free their `SourceTV` slots.
    pub fn stop_all(&self) {
        let all: Vec<Arc<Session>> = self.lock().drain().map(|(_, s)| s).collect();
        for s in &all {
            s.stop.store(true, Ordering::Relaxed);
        }
        for s in all {
            if let Some(t) = s.thread.lock().unwrap_or_else(PoisonError::into_inner).take() {
                let _ = t.join();
            }
        }
    }
}

/// 128 random bits as hex; the link to a match is all it takes to watch it.
fn random_id() -> String {
    let r = || RandomState::new().hash_one(Instant::now());
    format!("{:016x}{:016x}", r(), r())
}

/// Not loopback, private, link-local, carrier-grade NAT (Tailscale), multicast or reserved.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ids_are_long_and_differ() {
        let (a, b) = (random_id(), random_id());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
