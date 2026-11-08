//! Rooms as `SourceTV` addresses: each room someone is in gets a port from the range
//! (`--relay-ports`) and a password, and plays its demo there as the room plays (see
//! `stvtf::relay::Playback`). TF2 clients connect with `connect HOST:PORT; password PW`.

use crate::lock;
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use stvtf::relay::{Playback, Relay, RelayConfig, Want};

/// TF2 clients a room takes at once.
const SLOTS: usize = 24;

/// The ports rooms may take, and the address clients are told.
pub(crate) struct Relays {
    host: String,
    free: Mutex<BTreeSet<u16>>,
}

impl Relays {
    pub(crate) fn new(host: &str, ports: RangeInclusive<u16>) -> Arc<Relays> {
        Arc::new(Relays { host: host.to_string(), free: Mutex::new(ports.collect()) })
    }

    /// A relay on a free port, playing as `want` says until dropped; None when every port is taken.
    pub(crate) fn start(self: &Arc<Self>, want: Want) -> Result<Option<RoomRelay>> {
        let Some(port) = lock(&self.free).pop_first() else { return Ok(None) };
        let config =
            RelayConfig { password: stvtf::relay::random_password(), slots: SLOTS, ..RelayConfig::new(&format!("0.0.0.0:{port}")) };
        let relay = match Relay::bind(&config) {
            Ok(r) => Arc::new(r),
            Err(e) => {
                lock(&self.free).insert(port);
                return Err(e).with_context(|| format!("relay port {port}"));
            }
        };
        let (playback, stop) = (Arc::new(Playback::new(want)), Arc::new(AtomicBool::new(false)));
        let (r, p, s, pool) = (relay.clone(), playback.clone(), stop.clone(), self.clone());
        std::thread::spawn(move || {
            if let Err(e) = r.play(&p, &s, &mut |_| {}) {
                eprintln!("stvtf-web: relay on port {port}: {e:#}");
            }
            // Free once the socket is closed, so the next room can bind it.
            drop(r);
            lock(&pool.free).insert(port);
        });
        Ok(Some(RoomRelay { pool: self.clone(), port, password: config.password, relay, playback, stop }))
    }
}

/// A room's relay; dropping it stops it, which frees its port.
pub(crate) struct RoomRelay {
    pool: Arc<Relays>,
    port: u16,
    password: String,
    relay: Arc<Relay>,
    playback: Arc<Playback>,
    stop: Arc<AtomicBool>,
}

impl RoomRelay {
    /// Where the room is now.
    pub(crate) fn follow(&self, want: Want) {
        self.playback.set(want);
    }

    /// TF2 clients in.
    pub(crate) fn clients(&self) -> usize {
        self.relay.clients()
    }

    /// What pages show: the console line to connect, and why it cannot play, if it cannot.
    pub(crate) fn line(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "relay",
            "connect": format!("{}:{}", self.pool.host, self.port),
            "password": self.password,
            "problem": self.playback.problem(),
        })
    }
}

impl Drop for RoomRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Which demo file holds a server tick of what a room plays.
pub(crate) type DemoOf = Arc<dyn Fn(u32) -> Option<PathBuf> + Send + Sync>;

/// A room's relays, and where its demos are.
#[derive(Clone)]
pub(crate) struct Offer {
    pub(crate) relays: Arc<Relays>,
    pub(crate) demo_of: DemoOf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn want() -> Want {
        Want { demo: "nowhere.dem".into(), tick: 0, at: Instant::now(), paused: false, speed: 1.0, from_start: false }
    }

    #[test]
    fn rooms_take_ports_from_the_range_and_give_them_back() {
        let relays = Relays::new("stv.example", 47_901..=47_902);
        let a = relays.start(want()).unwrap().unwrap();
        let b = relays.start(want()).unwrap().unwrap();
        assert!(relays.start(want()).unwrap().is_none(), "all taken");
        assert_eq!(a.line()["connect"], "stv.example:47901");
        assert_ne!(a.password, b.password);
        drop(a);
        let until = Instant::now() + std::time::Duration::from_secs(5);
        let c = loop {
            if let Some(c) = relays.start(want()).unwrap() {
                break c;
            }
            assert!(Instant::now() < until, "the port never came back");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(c.port, 47_901);
    }
}
