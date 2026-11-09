//! Rooms as `SourceTV` addresses: each room someone is in gets a port from the range
//! (`--relay-ports`) and a password, and plays its demo there as the room plays (see
//! `stvtf::relay::Playback`). TF2 clients connect with `connect HOST:PORT; password PW`. The
//! relays run in this process, or in one of their own that outlives deploys (see `daemon`).

use crate::lock;
use crate::relays::daemon::{Remote, RemoteRelay};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use stvtf::relay::{Playback, Relay, RelayConfig, Want};

/// TF2 clients a room takes at once.
const SLOTS: usize = 24;
/// The longest password a relay is started again with.
const MAX_PASSWORD: usize = 32;

/// A relay's port and password: started again on them, TF2's `retry` gets back in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Was {
    pub(crate) port: u16,
    pub(crate) password: String,
}

impl Was {
    /// From a relay's page line.
    pub(crate) fn of_line(line: &serde_json::Value) -> Option<Was> {
        let port = u16::try_from(line.get("port")?.as_u64()?).ok()?;
        Some(Was { port, password: line.get("password")?.as_str()?.to_string() })
    }

    /// Safe in a console line.
    fn password_ok(&self) -> bool {
        (1..=MAX_PASSWORD).contains(&self.password.len()) && self.password.bytes().all(|b| b.is_ascii_alphanumeric())
    }
}

/// The ports rooms may take, and the address clients are told.
pub(crate) struct Relays {
    host: String,
    /// The host's IPv4 address, for `steam://connect` links, which take no names; looked up once.
    ip: Option<String>,
    /// Taken from the front; one that would not bind goes to the back.
    free: Mutex<VecDeque<u16>>,
}

/// A relay's socket with its port: the port goes back to the pool only once the socket is closed,
/// which is when the last holder (the room, or the thread playing) lets go.
struct Bound {
    relay: Option<Relay>,
    port: u16,
    pool: Arc<Relays>,
}

impl Drop for Bound {
    fn drop(&mut self) {
        drop(self.relay.take());
        lock(&self.pool.free).push_back(self.port);
    }
}

impl Bound {
    fn relay(&self) -> &Relay {
        self.relay.as_ref().expect("a relay until dropped")
    }
}

impl Relays {
    pub(crate) fn new(host: &str, ports: impl IntoIterator<Item = u16>) -> Arc<Relays> {
        use std::net::ToSocketAddrs;
        let ip = (host, 0).to_socket_addrs().ok().and_then(|mut a| a.find(std::net::SocketAddr::is_ipv4)).map(|a| a.ip().to_string());
        if ip.is_none() {
            eprintln!("stvtf-web: --relay-host {host} has no IPv4 address: rooms get no Open in TF2 link");
        }
        Arc::new(Relays { host: host.to_string(), ip, free: Mutex::new(ports.into_iter().collect()) })
    }

    /// A relay on a free port, playing as `want` says until dropped; None when every port is taken.
    /// On what it `was` on, if that port is free.
    pub(crate) fn start(self: &Arc<Self>, want: Want, was: Option<&Was>) -> Result<Option<LocalRelay>> {
        let port = {
            let mut free = lock(&self.free);
            match was.and_then(|w| free.iter().position(|&p| p == w.port)) {
                Some(i) => free.remove(i),
                None => free.pop_front(),
            }
        };
        let Some(port) = port else { return Ok(None) };
        let password = was.filter(|w| w.password_ok()).map_or_else(stvtf::relay::random_password, |w| w.password.clone());
        let config = RelayConfig { password, slots: SLOTS, ..RelayConfig::new(&format!("0.0.0.0:{port}")) };
        let relay = match Relay::bind(&config) {
            Ok(r) => r,
            Err(e) => {
                // Something else has it: the others first, next time.
                lock(&self.free).push_back(port);
                return Err(e).with_context(|| format!("relay port {port}"));
            }
        };
        let bound = Arc::new(Bound { relay: Some(relay), port, pool: self.clone() });
        let (playback, stop) = (Arc::new(Playback::new(want)), Arc::new(AtomicBool::new(false)));
        let (b, p, s) = (bound.clone(), playback.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            if let Err(e) = b.relay().play(&p, &s, &mut |_| {}) {
                eprintln!("stvtf-web: relay on port {port}: {e:#}");
            }
        });
        Ok(Some(LocalRelay { bound: Some(bound), password: config.password, playback, stop, thread }))
    }
}

/// A relay in this process; dropping it stops it, and its port is free again once the thread is
/// done.
pub(crate) struct LocalRelay {
    bound: Option<Arc<Bound>>,
    password: String,
    playback: Arc<Playback>,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl LocalRelay {
    fn bound(&self) -> &Bound {
        self.bound.as_ref().expect("bound until dropped")
    }

    /// Where the room is now.
    pub(crate) fn follow(&self, want: Want) {
        self.playback.set(want);
    }

    /// TF2 clients in.
    pub(crate) fn clients(&self) -> usize {
        self.bound().relay().clients()
    }

    /// Whether it stopped on its own (it could not even start), so the room should start another.
    pub(crate) fn stopped(&self) -> bool {
        self.thread.is_finished()
    }

    pub(crate) fn was(&self) -> Was {
        Was { port: self.bound().port, password: self.password.clone() }
    }

    /// What pages show: the console line to connect, and why it cannot play, if it cannot.
    pub(crate) fn line(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "relay",
            "connect": format!("{}:{}", self.bound().pool.host, self.bound().port),
            "port": self.bound().port,
            // For the steam:// link, which takes an address and no name.
            "address": self.bound().pool.ip.as_ref().map(|ip| format!("{ip}:{}", self.bound().port)),
            "password": self.password,
            "problem": self.playback.problem(),
        })
    }
}

impl Drop for LocalRelay {
    fn drop(&mut self) {
        // Let go of the socket first: the thread holds the last of it, and frees the port.
        drop(self.bound.take());
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Where rooms' relays run.
pub(crate) enum Pool {
    /// In this process, on these ports.
    Local(Arc<Relays>),
    /// In the relay process (`--relay-socket`).
    Remote(Arc<Remote>),
}

impl Pool {
    /// Room `room`'s relay, playing as `want` says, on what it `was` on if it can; None when
    /// every port is taken. In the relay process, the one it had if it had one.
    pub(crate) fn start(&self, room: &str, want: Want, was: Option<&Was>) -> Result<Option<RoomRelay>> {
        Ok(match self {
            Pool::Local(r) => r.start(want, was)?.map(RoomRelay::Local),
            Pool::Remote(r) => r.start(room, &want, was)?.map(RoomRelay::Remote),
        })
    }
}

/// A room's relay, wherever it runs.
pub(crate) enum RoomRelay {
    Local(LocalRelay),
    Remote(RemoteRelay),
}

impl RoomRelay {
    /// Where the room is now.
    pub(crate) fn follow(&self, want: Want) {
        match self {
            RoomRelay::Local(r) => r.follow(want),
            RoomRelay::Remote(r) => r.follow(&want),
        }
    }

    /// TF2 clients in.
    pub(crate) fn clients(&self) -> usize {
        match self {
            RoomRelay::Local(r) => r.clients(),
            RoomRelay::Remote(r) => r.clients(),
        }
    }

    /// Whether it stopped on its own, so the room should start another.
    pub(crate) fn stopped(&self) -> bool {
        match self {
            RoomRelay::Local(r) => r.stopped(),
            RoomRelay::Remote(r) => r.stopped(),
        }
    }

    /// What pages show (see [`LocalRelay::line`]).
    pub(crate) fn line(&self) -> serde_json::Value {
        match self {
            RoomRelay::Local(r) => r.line(),
            RoomRelay::Remote(r) => r.line(),
        }
    }

    pub(crate) fn was(&self) -> Option<Was> {
        Was::of_line(&self.line())
    }

    /// The room ended: so does its relay. (Only dropped, one in the relay process plays on, for the
    /// next stv.tf to find.)
    pub(crate) fn end(self) {
        if let RoomRelay::Remote(r) = &self {
            r.end();
        }
    }
}

/// Which demo file holds a server tick of what a room plays.
pub(crate) type DemoOf = Arc<dyn Fn(u32) -> Option<PathBuf> + Send + Sync>;

/// A room's relays, and where its demos are.
#[derive(Clone)]
pub(crate) struct Offer {
    pub(crate) relays: Arc<Pool>,
    pub(crate) demo_of: DemoOf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Two UDP ports nothing has just now.
    fn free_ports() -> [u16; 2] {
        let a = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let b = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        [a.local_addr().unwrap().port(), b.local_addr().unwrap().port()]
    }

    #[test]
    fn a_port_something_else_has_goes_to_the_back() {
        let taken = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let [free, _] = free_ports();
        let relays = Relays::new("h", [taken.local_addr().unwrap().port(), free]);
        assert!(relays.start(want(), None).is_err());
        let r = relays.start(want(), None).unwrap().unwrap();
        assert_eq!(r.bound().port, free, "the free one, not the taken one again");
    }

    fn want() -> Want {
        Want { demo: "nowhere.dem".into(), tick: 0, at: Instant::now(), paused: false, speed: 1.0, from_start: false, camera: None }
    }

    #[test]
    fn rooms_take_ports_from_the_range_and_give_them_back() {
        let ports = free_ports();
        let relays = Relays::new("127.0.0.1", ports);
        let a = relays.start(want(), None).unwrap().unwrap();
        let b = relays.start(want(), None).unwrap().unwrap();
        assert!(relays.start(want(), None).unwrap().is_none(), "all taken");
        assert_eq!(a.line()["connect"], format!("127.0.0.1:{}", ports[0]));
        assert_eq!(a.line()["address"], format!("127.0.0.1:{}", ports[0]));
        assert_ne!(a.password, b.password);
        drop(a);
        let until = Instant::now() + std::time::Duration::from_secs(5);
        let c = loop {
            if let Some(c) = relays.start(want(), None).unwrap() {
                break c;
            }
            assert!(Instant::now() < until, "the port never came back");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(c.bound().port, ports[0]);
        drop(b);
    }

    #[test]
    fn a_relay_started_again_gets_the_port_and_password_it_had() {
        let ports = free_ports();
        let relays = Relays::new("127.0.0.1", ports);
        let was = Was { port: ports[1], password: "again2".into() };
        let r = relays.start(want(), Some(&was)).unwrap().unwrap();
        assert_eq!(Was::of_line(&r.line()), Some(was));
        let odd = Was { port: ports[0], password: "a\";quit".into() };
        assert_ne!(relays.start(want(), Some(&odd)).unwrap().unwrap().password, odd.password, "not in a console line");
    }
}
