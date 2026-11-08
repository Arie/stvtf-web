//! The rooms' relays in a process of their own (`--relay-daemon SOCKET`), so stv.tf can be deployed
//! without the TF2 clients in a room noticing: the relays keep their ports and connections, and a
//! room picked up by the next stv.tf finds its relay again, port and password the same.
//!
//! stv.tf talks to it over a unix socket (`--relay-socket`, on a volume both see), a JSON line per
//! request and per answer:
//!
//! - `{"op": "start", "room", "want", "was"}`: the room's relay, started if it has none (on the
//!   port and password it `was` on, if it can); its page line, null when every port is taken.
//! - `{"op": "follow", "room", "want"}`: where the room is now.
//! - `{"op": "status", "room"}`: TF2 clients in, whether it stopped, and its page line.
//! - `{"op": "stop", "room"}`: the room ended.
//!
//! A `want` is a [`stvtf::relay::Want`] with `age_ms` (how long ago `tick` was wanted) for its
//! instant. A relay nobody asked about for [`LEASE`] (its room gone with an stv.tf that crashed)
//! is stopped.

use crate::lock;
use crate::relays::{LocalRelay, Relays, Was};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stvtf::relay::Want;

/// A relay not asked about for this long is stopped: longer than any deploy takes.
pub(crate) const LEASE: Duration = Duration::from_mins(2);
/// How long stv.tf waits for an answer.
const ANSWER: Duration = Duration::from_millis(500);
/// How often stv.tf asks how a relay is, at most.
const STATUS_EVERY: Duration = Duration::from_millis(500);

/// A [`Want`] on the wire.
#[derive(Serialize, Deserialize)]
pub(crate) struct WantLine {
    demo: PathBuf,
    tick: u32,
    age_ms: u64,
    paused: bool,
    speed: f64,
    from_start: bool,
    /// The player (user id) the camera follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    follow: Option<u16>,
}

impl WantLine {
    fn of(w: &Want) -> WantLine {
        WantLine {
            demo: w.demo.clone(),
            tick: w.tick,
            age_ms: w.at.elapsed().as_millis() as u64,
            paused: w.paused,
            speed: w.speed,
            from_start: w.from_start,
            follow: w.camera.map(|stvtf::relay::Camera::Follow { player }| player),
        }
    }

    fn want(self) -> Want {
        let at = Instant::now().checked_sub(Duration::from_millis(self.age_ms)).unwrap_or_else(Instant::now);
        Want {
            demo: self.demo,
            tick: self.tick,
            at,
            paused: self.paused,
            speed: self.speed,
            from_start: self.from_start,
            camera: self.follow.map(|player| stvtf::relay::Camera::Follow { player }),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Request {
    Start {
        room: String,
        want: WantLine,
        #[serde(default)]
        was: Option<Was>,
    },
    Follow {
        room: String,
        want: WantLine,
    },
    Status {
        room: String,
    },
    Stop {
        room: String,
    },
}

struct Leased {
    relay: LocalRelay,
    seen: Instant,
}

type Rooms = Arc<Mutex<HashMap<String, Leased>>>;

/// Serves relays on `socket` until the process ends.
pub(crate) fn serve(socket: &Path, relays: &Arc<Relays>, lease: Duration) -> Result<()> {
    // One left by a process before is in the way.
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket).with_context(|| format!("listening on {}", socket.display()))?;
    eprintln!("stvtf-web: relays at {}", socket.display());
    let rooms: Rooms = Arc::default();
    let reaped = rooms.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            lock(&reaped).retain(|room, l| {
                let keep = l.seen.elapsed() < lease && !l.relay.stopped();
                if !keep {
                    eprintln!("stvtf-web: relay of room {} stopped", &room[..8.min(room.len())]);
                }
                keep
            });
        }
    });
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let (rooms, relays) = (rooms.clone(), relays.clone());
        std::thread::spawn(move || {
            let _ = answer(conn, &rooms, &relays);
        });
    }
    Ok(())
}

/// Answers the requests on one connection.
fn answer(conn: UnixStream, rooms: &Rooms, relays: &Arc<Relays>) -> Result<()> {
    let mut out = conn.try_clone()?;
    for line in BufReader::new(conn).lines() {
        let answer = match serde_json::from_str::<Request>(&line?) {
            Ok(r) => handle(r, rooms, relays),
            Err(e) => json!({ "error": format!("{e}") }),
        };
        writeln!(out, "{answer}")?;
    }
    Ok(())
}

fn handle(r: Request, rooms: &Rooms, relays: &Arc<Relays>) -> Value {
    match r {
        Request::Start { room, want, was } => {
            let mut all = lock(rooms);
            if let Some(l) = all.get_mut(&room) {
                l.seen = Instant::now();
                l.relay.follow(want.want());
                return json!({ "line": l.relay.line() });
            }
            match relays.start(want.want(), was.as_ref()) {
                Ok(Some(relay)) => {
                    let line = relay.line();
                    all.insert(room, Leased { relay, seen: Instant::now() });
                    json!({ "line": line })
                }
                Ok(None) => json!({ "line": null }),
                Err(e) => json!({ "error": format!("{e:#}") }),
            }
        }
        Request::Follow { room, want } => match lock(rooms).get_mut(&room) {
            Some(l) => {
                l.seen = Instant::now();
                l.relay.follow(want.want());
                json!({})
            }
            None => json!({ "gone": true }),
        },
        Request::Status { room } => match lock(rooms).get_mut(&room) {
            Some(l) => {
                l.seen = Instant::now();
                json!({ "clients": l.relay.clients(), "stopped": l.relay.stopped(), "line": l.relay.line() })
            }
            None => json!({ "gone": true }),
        },
        Request::Stop { room } => {
            lock(rooms).remove(&room);
            json!({})
        }
    }
}

/// stv.tf's end: one connection, shared by its rooms, opened again when it breaks.
pub(crate) struct Remote {
    socket: PathBuf,
    conn: Mutex<Option<(UnixStream, BufReader<UnixStream>)>>,
}

impl Remote {
    pub(crate) fn new(socket: &Path) -> Arc<Remote> {
        Arc::new(Remote { socket: socket.to_owned(), conn: Mutex::new(None) })
    }

    /// One request and its answer; a broken connection is opened again once.
    fn call(&self, request: &Value) -> Result<Value> {
        let mut conn = lock(&self.conn);
        let mut tried = None;
        for _ in 0..2 {
            if conn.is_none() {
                let s = UnixStream::connect(&self.socket).with_context(|| format!("the relays at {}", self.socket.display()))?;
                s.set_read_timeout(Some(ANSWER))?;
                s.set_write_timeout(Some(ANSWER))?;
                *conn = Some((s.try_clone()?, BufReader::new(s)));
            }
            let Some((w, r)) = conn.as_mut() else { continue };
            let mut line = String::new();
            let sent = writeln!(w, "{request}").map_err(anyhow::Error::from).and_then(|()| Ok(r.read_line(&mut line)?));
            match sent {
                Ok(n) if n > 0 => {
                    let answer: Value = serde_json::from_str(&line)?;
                    if let Some(e) = answer.get("error").and_then(Value::as_str) {
                        bail!("{e}");
                    }
                    return Ok(answer);
                }
                Ok(_) => tried = Some(anyhow::anyhow!("the relays hung up")),
                Err(e) => tried = Some(e),
            }
            *conn = None;
        }
        Err(tried.unwrap_or_else(|| anyhow::anyhow!("no answer")))
    }

    /// Room `room`'s relay, started if it has none, playing as `want` says, on what it `was` on if
    /// it can; None when every port is taken.
    pub(crate) fn start(self: &Arc<Self>, room: &str, want: &Want, was: Option<&Was>) -> Result<Option<RemoteRelay>> {
        let answer = self.call(&json!({ "op": "start", "room": room, "want": WantLine::of(want), "was": was }))?;
        let line = answer.get("line").cloned().unwrap_or(Value::Null);
        if line.is_null() {
            return Ok(None);
        }
        let status = Status { clients: 0, stopped: false, line, asked: Instant::now() };
        Ok(Some(RemoteRelay { remote: self.clone(), room: room.to_string(), status: Mutex::new(status) }))
    }
}

struct Status {
    clients: usize,
    stopped: bool,
    line: Value,
    asked: Instant,
}

/// A room's relay in the relay process. Dropped, it keeps playing (stv.tf going away for a deploy
/// leaves it be): [`RemoteRelay::end`] stops it.
pub(crate) struct RemoteRelay {
    remote: Arc<Remote>,
    room: String,
    status: Mutex<Status>,
}

impl RemoteRelay {
    pub(crate) fn follow(&self, want: &Want) {
        let answer = self.remote.call(&json!({ "op": "follow", "room": self.room, "want": WantLine::of(want) }));
        if answer.is_ok_and(|a| a.get("gone").is_some()) {
            lock(&self.status).stopped = true;
        }
    }

    /// How it is, asked of the relay process at most every [`STATUS_EVERY`].
    fn status<T>(&self, read: impl FnOnce(&Status) -> T) -> T {
        let mut st = lock(&self.status);
        if st.asked.elapsed() >= STATUS_EVERY {
            st.asked = Instant::now();
            match self.remote.call(&json!({ "op": "status", "room": self.room })) {
                Ok(a) if a.get("gone").is_some() => st.stopped = true,
                Ok(a) => {
                    st.clients = a.get("clients").and_then(Value::as_u64).unwrap_or(0) as usize;
                    st.stopped = a.get("stopped").and_then(Value::as_bool).unwrap_or(false);
                    if let Some(line) = a.get("line") {
                        st.line = line.clone();
                    }
                }
                // The relay process is away (restarting): as it was.
                Err(_) => {}
            }
        }
        read(&st)
    }

    pub(crate) fn clients(&self) -> usize {
        self.status(|s| s.clients)
    }

    pub(crate) fn stopped(&self) -> bool {
        self.status(|s| s.stopped)
    }

    pub(crate) fn line(&self) -> Value {
        self.status(|s| s.line.clone())
    }

    /// The room ended: so does its relay.
    pub(crate) fn end(&self) {
        let _ = self.remote.call(&json!({ "op": "stop", "room": self.room }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn want() -> Want {
        Want { demo: "nowhere.dem".into(), tick: 0, at: Instant::now(), paused: false, speed: 1.0, from_start: false, camera: None }
    }

    #[test]
    fn a_room_finds_its_relay_again_and_ends_it() {
        let dir = TempDir::new("relay-daemon");
        let socket = dir.join("relay.sock");
        let port = std::net::UdpSocket::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port();
        let relays = Relays::new("127.0.0.1", [port]);
        let s = socket.clone();
        std::thread::spawn(move || serve(&s, &relays, Duration::from_secs(1)));
        let until = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < until, "no socket");
            std::thread::sleep(Duration::from_millis(10));
        }
        let remote = Remote::new(&socket);
        let a = remote.start("room-a", &want(), None).unwrap().unwrap();
        assert_eq!(a.line()["connect"], format!("127.0.0.1:{port}"));
        // The next stv.tf (another connection) finds the same relay, password and all.
        let next = Remote::new(&socket);
        let again = next.start("room-a", &want(), None).unwrap().unwrap();
        assert_eq!(again.line()["password"], a.line()["password"]);
        assert!(next.start("room-b", &want(), None).unwrap().is_none(), "the only port is taken");
        a.follow(&want());
        assert_eq!(a.clients(), 0);
        // Ended, its port is free for another room.
        again.end();
        let until = Instant::now() + Duration::from_secs(5);
        let b = loop {
            if let Some(b) = next.start("room-b", &want(), None).unwrap() {
                break b;
            }
            assert!(Instant::now() < until, "the port never came back");
            std::thread::sleep(Duration::from_millis(20));
        };
        // Started again with what it was on, as after a restart of the relay process.
        let was = Was::of_line(&b.line()).unwrap();
        b.end();
        let until = Instant::now() + Duration::from_secs(5);
        let b = loop {
            if let Some(b) = next.start("room-b2", &want(), Some(&was)).unwrap() {
                break b;
            }
            assert!(Instant::now() < until, "the port never came back");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(Was::of_line(&b.line()), Some(was));
        // Not asked about for a lease, it goes.
        drop(b);
        std::thread::sleep(Duration::from_millis(2500));
        assert!(next.start("room-c", &want(), None).unwrap().is_some(), "the lease ran out");
    }
}
