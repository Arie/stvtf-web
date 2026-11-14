//! The rooms' relays in a process of their own (`--relay-daemon SOCKET`), so stv.tf can be deployed
//! without the TF2 clients in a room noticing: the relays keep their ports and connections, and a
//! room picked up by the next stv.tf finds its relay again, port and password the same.
//!
//! stv.tf talks to it over a unix socket (`--relay-socket`, on a volume both see), a JSON line per
//! request and per answer:
//!
//! - `{"op": "start", "room", "want", "was"}`: the room's relay, started if it has none or its
//!   own stopped (on the port and password it `was` on, if it can); its page line, null when
//!   every port is taken.
//! - `{"op": "follow", "room", "want"}`: where the room is now.
//! - `{"op": "status", "room"}`: TF2 clients in, whether it stopped, and its page line.
//! - `{"op": "keep", "room", "beams"}`: lines the TF2 clients in keep seeing, in place of those
//!   before, each `[x1, y1, z1, x2, y2, z2, r, g, b, seconds, width, speed]`.
//! - `{"op": "voice", "room", "voice"}`: voice from outside the match to send them as players'
//!   own, each `{user, steamid, rate, frames}` (frames in base64), in order.
//! - `{"op": "notes", "room", "notes"}`: text bubbles they keep seeing, in place of those before,
//!   each a [`stvtf::relay::Note`].
//! - `{"op": "light", "room", "light"}`: the light they keep seeing, in place of the one before, a
//!   [`stvtf::relay::Light`]; null puts it out.
//! - `{"op": "stop", "room"}`: the room ended.
//! - `{"op": "clients"}`: TF2 clients in, over all rooms.
//!
//! Told to stop (SIGTERM), it hands its TF2 clients to the process after it, in a file beside
//! the socket ([`handed_at`]): that one takes them in on the same ports and they play on, with
//! a pause of some seconds and no reconnect.
//!
//! A `want` is a [`stvtf::relay::Want`] with `age_ms` (how long ago `tick` was wanted) for its
//! instant. A relay nobody asked about for [`LEASE`] (its room gone with an stv.tf that crashed)
//! is stopped.
//!
//! stv.tf never waits on it: a thread per relay talks to it, and one that does not answer is left
//! alone for [`AWAY`].

use crate::lock;
use crate::matches::sessions::logged;
use crate::relays::pool::{LocalRelay, Relays, Was};
use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use stvtf::relay::{Beam, Handover, Light, Note, VoiceIn, Want};

/// A relay not asked about for this long is stopped: longer than any deploy takes.
pub(crate) const LEASE: Duration = Duration::from_mins(2);
/// How often relays whose lease ran out are stopped.
const REAP_EVERY: Duration = Duration::from_secs(1);
/// How long stv.tf waits for an answer.
const ANSWER: Duration = Duration::from_millis(500);
/// How often stv.tf asks how a relay is.
const STATUS_EVERY: Duration = Duration::from_millis(500);
/// How long a relay process that did not answer is left alone.
const AWAY: Duration = Duration::from_secs(2);
/// How long a relay gets to hand its clients over.
const HAND_OVER: Duration = Duration::from_secs(3);
/// Clients handed over longer ago are gone: TF2 gives a silent server 30 s.
const HANDED_FOR: Duration = Duration::from_secs(25);
/// How often the relay process looks whether it is told to stop.
const STOP_POLL: Duration = Duration::from_millis(50);

/// A [`Want`] on the wire.
#[derive(Clone, Serialize, Deserialize)]
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
    Keep {
        room: String,
        beams: Vec<BeamLine>,
    },
    Notes {
        room: String,
        notes: Vec<Note>,
    },
    Voice {
        room: String,
        voice: Vec<VoiceLine>,
    },
    Light {
        room: String,
        light: Option<Light>,
    },
    Stop {
        room: String,
    },
    Clients {},
}

/// A [`VoiceIn`] on the wire: its frames in base64.
#[derive(Serialize, Deserialize)]
struct VoiceLine {
    user: Option<u16>,
    steamid: String,
    rate: u16,
    frames: Vec<String>,
}

impl VoiceLine {
    fn of(v: &VoiceIn) -> Self {
        let b64 = base64::engine::general_purpose::STANDARD;
        VoiceLine { user: v.user, steamid: v.steamid.clone(), rate: v.rate, frames: v.frames.iter().map(|f| b64.encode(f)).collect() }
    }

    /// None when a frame is not base64.
    fn voice(&self) -> Option<VoiceIn> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let frames = self.frames.iter().map(|f| b64.decode(f).ok()).collect::<Option<_>>()?;
        Some(VoiceIn { user: self.user, steamid: self.steamid.clone(), rate: self.rate, frames })
    }
}

/// The most voice waiting for the relay process; past it the oldest goes.
const MAX_VOICE_WAITING: usize = 256;

/// A [`Beam`] on the wire.
type BeamLine = [f32; 12];

fn beam_line(b: &Beam) -> BeamLine {
    let [r, g, bl] = b.rgb.map(f32::from);
    [b.from[0], b.from[1], b.from[2], b.to[0], b.to[1], b.to[2], r, g, bl, b.life, b.width, f32::from(b.speed)]
}

fn beam(l: &BeamLine) -> Beam {
    let byte = |v: f32| v.clamp(0.0, 255.0) as u8;
    Beam {
        from: [l[0], l[1], l[2]],
        to: [l[3], l[4], l[5]],
        rgb: [byte(l[6]), byte(l[7]), byte(l[8])],
        life: l[9],
        width: l[10],
        speed: byte(l[11]),
    }
}

struct Leased {
    relay: LocalRelay,
    seen: Instant,
    /// What it was last told to play and show: handed over with its clients.
    shows: Shows,
}

/// What a relay plays and its clients keep seeing.
#[derive(Clone)]
struct Shows {
    want: Want,
    beams: Vec<BeamLine>,
    notes: Vec<Note>,
    light: Option<Light>,
}

impl Shows {
    fn of(want: Want) -> Shows {
        Shows { want, beams: Vec::new(), notes: Vec::new(), light: None }
    }
}

type Rooms = Arc<Mutex<HashMap<String, Leased>>>;

/// A room's relay with TF2 clients in, handed to the next process.
#[derive(Serialize, Deserialize)]
struct HandedRoom {
    room: String,
    was: Was,
    want: WantLine,
    #[serde(default)]
    beams: Vec<BeamLine>,
    #[serde(default)]
    notes: Vec<Note>,
    #[serde(default)]
    light: Option<Light>,
    clients: Handover,
}

/// What a relay process told to stop leaves the next.
#[derive(Serialize, Deserialize)]
struct Handed {
    /// When, in milliseconds since 1970.
    at_ms: u64,
    rooms: Vec<HandedRoom>,
}

/// Where the relay process on `socket` leaves its clients for the next.
pub(crate) fn handed_at(socket: &Path) -> PathBuf {
    socket.with_extension("handover")
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Stops every relay with TF2 clients in and writes them to `file`, each relay at once.
fn hand_over(rooms: &Rooms, file: &Path) -> Result<usize> {
    let all = std::mem::take(&mut *lock(rooms));
    let handed: Vec<HandedRoom> = std::thread::scope(|s| {
        let asked: Vec<_> = all
            .iter()
            .filter(|(_, l)| l.relay.clients() > 0 && !l.relay.stopped())
            .map(|(room, l)| {
                s.spawn(move || {
                    let clients = l.relay.hand_over(HAND_OVER).filter(|h| h.clients() > 0)?;
                    let Shows { want, beams, notes, light } = l.shows.clone();
                    Some(HandedRoom { room: room.clone(), was: l.relay.was(), want: WantLine::of(&want), beams, notes, light, clients })
                })
            })
            .collect();
        asked.into_iter().filter_map(|t| t.join().ok().flatten()).collect()
    });
    let clients = handed.iter().map(|r| r.clients.clients()).sum();
    let part = file.with_extension("handover.part");
    // A want's age is as of now.
    std::fs::write(&part, serde_json::to_vec(&Handed { at_ms: now_ms(), rooms: handed })?)?;
    std::fs::rename(&part, file)?;
    Ok(clients)
}

/// Takes in what the process before left in `file`, if it is not too long ago: each room's relay
/// on its port again, its clients in.
fn take_over(rooms: &Rooms, relays: &Arc<Relays>, file: &Path) {
    let Ok(bytes) = std::fs::read(file) else { return };
    let _ = std::fs::remove_file(file);
    let handed: Handed = match serde_json::from_slice(&bytes) {
        Ok(h) => h,
        Err(e) => return log!("the clients handed over do not read: {e}"),
    };
    let ago = Duration::from_millis(now_ms().saturating_sub(handed.at_ms));
    if ago > HANDED_FOR {
        return log!("clients handed over {} s ago are gone", ago.as_secs());
    }
    for HandedRoom { room, was, mut want, beams, notes, light, clients } in handed.rooms {
        want.age_ms += ago.as_millis() as u64;
        let shows = Shows { want: want.want(), beams, notes, light };
        let started = relays.start_with(shows.want.clone(), Some(&was), Some(&clients));
        match started {
            Ok(Some(relay)) if relay.was() == was => {
                log!(
                    "relay of room {} taken over on port {}, {} clients, after {} ms",
                    logged(&room),
                    was.port,
                    clients.clients(),
                    ago.as_millis()
                );
                relay.keep(shows.beams.iter().map(beam).collect());
                relay.keep_notes(shows.notes.clone());
                relay.keep_light(shows.light);
                lock(rooms).insert(room, Leased { relay, seen: Instant::now(), shows });
            }
            Ok(_) => log!("relay of room {} not taken over: port {} is not free", logged(&room), was.port),
            Err(e) => log!("relay of room {} not taken over: {e:#}", logged(&room)),
        }
    }
}

/// What the relay process on `socket` answers when asked for its TF2 clients: `{"clients": N}`,
/// or an error from one that does not know the question (and hands none over).
pub(crate) fn clients(socket: &Path) -> Result<Value> {
    let mut conn = UnixStream::connect(socket).with_context(|| format!("the relays at {}", socket.display()))?;
    conn.set_read_timeout(Some(ANSWER))?;
    writeln!(conn, "{}", json!({ "op": "clients" }))?;
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

/// Serves relays on `socket` until told to `stop`: then its TF2 clients are handed over (see
/// [`handed_at`]) and the process ends.
pub(crate) fn serve(socket: &Path, relays: &Arc<Relays>, lease: Duration, stop: &'static AtomicBool) -> Result<()> {
    // One left by a process before is in the way.
    let _ = std::fs::remove_file(socket);
    let rooms: Rooms = Arc::default();
    take_over(&rooms, relays, &handed_at(socket));
    let listener = UnixListener::bind(socket).with_context(|| format!("listening on {}", socket.display()))?;
    log!("relays at {}", socket.display());
    let (leaving, file) = (rooms.clone(), handed_at(socket));
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(STOP_POLL);
        }
        match hand_over(&leaving, &file) {
            Ok(clients) => log!("relays stopping: {clients} clients handed over"),
            Err(e) => log!("relays stopping: clients not handed over: {e:#}"),
        }
        std::process::exit(0);
    });
    let reaped = rooms.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(REAP_EVERY);
            lock(&reaped).retain(|room, l| {
                let keep = l.seen.elapsed() < lease && !l.relay.stopped();
                if !keep {
                    log!("relay of room {} stopped", logged(room));
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

/// Says how starting a room's relay went: its port, to tell whose the relay's own notes are
/// (not its password), or why there is none.
fn said_of(room: &str, started: &Result<Option<LocalRelay>>) {
    match started {
        Ok(Some(relay)) => log!("relay of room {} started on port {}", logged(room), relay.was().port),
        Ok(None) => log!("no relay for room {}: every port is taken", logged(room)),
        Err(e) => log!("no relay for room {}: {e:#}", logged(room)),
    }
}

/// Room `room`'s relay, started if it has none or its own stopped; its page line.
fn start(room: String, want: WantLine, mut was: Option<Was>, rooms: &Rooms, relays: &Arc<Relays>) -> Value {
    let mut all = lock(rooms);
    if let Some(l) = all.get_mut(&room) {
        if !l.relay.stopped() {
            l.seen = Instant::now();
            l.shows.want = want.want();
            l.relay.follow(l.shows.want.clone());
            return json!({ "line": l.relay.line() });
        }
        // Stopped: a new one on its port.
        was = was.or_else(|| Some(l.relay.was()));
        all.remove(&room);
    }
    let want = want.want();
    let started = relays.start(want.clone(), was.as_ref());
    said_of(&room, &started);
    match started {
        Ok(Some(relay)) => {
            let line = relay.line();
            all.insert(room, Leased { relay, seen: Instant::now(), shows: Shows::of(want) });
            json!({ "line": line })
        }
        Ok(None) => json!({ "line": null }),
        Err(e) => json!({ "error": format!("{e:#}") }),
    }
}

fn handle(r: Request, rooms: &Rooms, relays: &Arc<Relays>) -> Value {
    match r {
        Request::Start { room, want, was } => start(room, want, was, rooms, relays),
        Request::Follow { room, want } => in_room(rooms, &room, |l| {
            l.seen = Instant::now();
            l.shows.want = want.want();
            l.relay.follow(l.shows.want.clone());
            json!({})
        }),
        Request::Status { room } => in_room(rooms, &room, |l| {
            l.seen = Instant::now();
            json!({ "clients": l.relay.clients(), "stopped": l.relay.stopped(), "line": l.relay.line() })
        }),
        Request::Keep { room, beams } => in_room(rooms, &room, |l| {
            l.relay.keep(beams.iter().map(beam).collect());
            l.shows.beams = beams;
            json!({})
        }),
        Request::Notes { room, notes } => in_room(rooms, &room, |l| {
            l.shows.notes.clone_from(&notes);
            l.relay.keep_notes(notes);
            json!({})
        }),
        Request::Voice { room, voice } => in_room(rooms, &room, |l| {
            for v in voice.iter().filter_map(VoiceLine::voice) {
                l.relay.voice(v);
            }
            json!({})
        }),
        Request::Light { room, light } => in_room(rooms, &room, |l| {
            l.shows.light = light;
            l.relay.keep_light(light);
            json!({})
        }),
        Request::Stop { room } => {
            lock(rooms).remove(&room);
            json!({})
        }
        Request::Clients {} => json!({ "clients": lock(rooms).values().map(|l| l.relay.clients()).sum::<usize>() }),
    }
}

/// `f`'s answer for relay `room`, or that it is gone.
fn in_room(rooms: &Rooms, room: &str, f: impl FnOnce(&mut Leased) -> Value) -> Value {
    lock(rooms).get_mut(room).map_or_else(|| json!({ "gone": true }), f)
}

/// stv.tf's end: one connection, shared by its rooms, opened again when it breaks.
pub(crate) struct Remote {
    socket: PathBuf,
    conn: Mutex<Option<(UnixStream, BufReader<UnixStream>)>>,
    /// Not asked until then: it did not answer.
    away: Mutex<Option<Instant>>,
}

impl Remote {
    pub(crate) fn new(socket: &Path) -> Arc<Remote> {
        Arc::new(Remote { socket: socket.to_owned(), conn: Mutex::new(None), away: Mutex::new(None) })
    }

    /// One request and its answer; after one with no answer, calls fail at once for [`AWAY`].
    fn call(&self, request: &Value) -> Result<Value> {
        let mut conn = lock(&self.conn);
        if lock(&self.away).is_some_and(|until| Instant::now() < until) {
            bail!("the relays at {} did not answer", self.socket.display());
        }
        let answer = match self.exchange(&mut conn, request) {
            Ok(a) => a,
            Err(e) => {
                *lock(&self.away) = Some(Instant::now() + AWAY);
                return Err(e);
            }
        };
        if let Some(e) = answer.get("error").and_then(Value::as_str) {
            bail!("{e}");
        }
        Ok(answer)
    }

    /// A broken connection is opened again once.
    fn exchange(&self, conn: &mut Option<(UnixStream, BufReader<UnixStream>)>, request: &Value) -> Result<Value> {
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
                Ok(n) if n > 0 => return Ok(serde_json::from_str(&line)?),
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
        let Some(line) = answer.get("line").filter(|l| !l.is_null()).cloned() else { return Ok(None) };
        let shared = Arc::new(Shared {
            remote: self.clone(),
            room: room.to_string(),
            inner: Mutex::new(Inner {
                clients: 0,
                stopped: false,
                line,
                want: None,
                kept: None,
                notes: None,
                voice: VecDeque::new(),
                notes_given: Vec::new(),
                light: None,
                light_given: false,
                light_due: false,
                done: None,
            }),
            wake: Condvar::new(),
        });
        let tending = shared.clone();
        std::thread::spawn(move || tending.tend());
        Ok(Some(RemoteRelay(shared)))
    }
}

/// How a relay's thread stops.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Done {
    /// The relay plays on, for the next stv.tf.
    LetGo,
    /// So does the relay: the room ended.
    End,
}

struct Inner {
    clients: usize,
    stopped: bool,
    line: Value,
    /// Not sent yet.
    want: Option<Want>,
    /// The kept beams to hand over, the latest only.
    kept: Option<Vec<Beam>>,
    /// The notes to hand over, the latest only.
    notes: Option<Vec<Note>>,
    /// Voice to hand over, all of it, in order.
    voice: VecDeque<VoiceLine>,
    /// The notes last given to hand over: the marker gives them with its every piece.
    notes_given: Vec<Note>,
    /// The light last given to hand over (the marker gives it with its every piece), whether one
    /// ever was, and whether it is still to hand over.
    light: Option<Light>,
    light_given: bool,
    light_due: bool,
    done: Option<Done>,
}

struct Shared {
    remote: Arc<Remote>,
    room: String,
    inner: Mutex<Inner>,
    wake: Condvar,
}

impl Shared {
    /// Sends where the room is, and asks how the relay is, until let go of or ended.
    fn tend(&self) {
        let mut asked = Instant::now();
        loop {
            let (want, kept, notes, voice, light, done) = {
                let mut i = lock(&self.inner);
                while i.want.is_none() && i.kept.is_none() && i.notes.is_none() && i.voice.is_empty() && !i.light_due && i.done.is_none() {
                    let left = STATUS_EVERY.saturating_sub(asked.elapsed());
                    if left.is_zero() {
                        break;
                    }
                    i = self.wake.wait_timeout(i, left).unwrap_or_else(PoisonError::into_inner).0;
                }
                let voice: Vec<VoiceLine> = i.voice.drain(..).collect();
                (i.want.take(), i.kept.take(), i.notes.take(), voice, std::mem::take(&mut i.light_due).then_some(i.light), i.done)
            };
            match done {
                Some(Done::End) => {
                    let _ = self.remote.call(&json!({ "op": "stop", "room": self.room }));
                    return;
                }
                Some(Done::LetGo) => return,
                None => {}
            }
            if let Some(want) = want {
                let answer = self.remote.call(&json!({ "op": "follow", "room": self.room, "want": WantLine::of(&want) }));
                if answer.is_ok_and(|a| a.get("gone").is_some()) {
                    lock(&self.inner).stopped = true;
                }
            }
            if let Some(kept) = kept {
                let kept: Vec<BeamLine> = kept.iter().map(beam_line).collect();
                let _ = self.remote.call(&json!({ "op": "keep", "room": self.room, "beams": kept }));
            }
            if let Some(notes) = notes {
                let _ = self.remote.call(&json!({ "op": "notes", "room": self.room, "notes": notes }));
            }
            if !voice.is_empty() {
                let _ = self.remote.call(&json!({ "op": "voice", "room": self.room, "voice": voice }));
            }
            if let Some(light) = light {
                let _ = self.remote.call(&json!({ "op": "light", "room": self.room, "light": light }));
            }
            if asked.elapsed() >= STATUS_EVERY {
                asked = Instant::now();
                self.ask_status();
            }
        }
    }

    /// Asks the relay process how the relay is.
    fn ask_status(&self) {
        let answer = self.remote.call(&json!({ "op": "status", "room": self.room }));
        let mut i = lock(&self.inner);
        match answer {
            Ok(a) if a.get("gone").is_some() => i.stopped = true,
            Ok(a) => {
                i.clients = a.get("clients").and_then(Value::as_u64).unwrap_or(0) as usize;
                i.stopped = a.get("stopped").and_then(Value::as_bool).unwrap_or(false);
                if let Some(line) = a.get("line") {
                    i.line = line.clone();
                }
            }
            // The relay process is away (restarting): as it was.
            Err(_) => {}
        }
    }

    fn finish(&self, done: Done) {
        lock(&self.inner).done.get_or_insert(done);
        self.wake.notify_all();
    }
}

/// A room's relay in the relay process, tended by a thread of its own. Dropped, it plays on (for
/// the next stv.tf); [`RemoteRelay::end`] stops it.
pub(crate) struct RemoteRelay(Arc<Shared>);

impl RemoteRelay {
    /// Where the room is now; only the latest is sent.
    pub(crate) fn follow(&self, want: &Want) {
        lock(&self.0.inner).want = Some(want.clone());
        self.0.wake.notify_all();
    }

    /// Beams the TF2 clients in keep seeing, in place of those before; only the latest is sent.
    pub(crate) fn keep(&self, beams: Vec<Beam>) {
        lock(&self.0.inner).kept = Some(beams);
        self.0.wake.notify_all();
    }

    /// The light the TF2 clients in keep seeing, in place of the one before; only the latest is
    /// sent, and not the same one again.
    pub(crate) fn keep_light(&self, light: Option<Light>) {
        let mut i = lock(&self.0.inner);
        if i.light_given && i.light == light {
            return;
        }
        i.light = light;
        i.light_given = true;
        i.light_due = true;
        drop(i);
        self.0.wake.notify_all();
    }

    /// Notes the TF2 clients in keep seeing, in place of those before; only the latest is sent,
    /// and not the same ones again.
    pub(crate) fn keep_notes(&self, notes: Vec<Note>) {
        let mut i = lock(&self.0.inner);
        if i.notes_given == notes {
            return;
        }
        i.notes_given.clone_from(&notes);
        i.notes = Some(notes);
        drop(i);
        self.0.wake.notify_all();
    }

    /// Voice from outside the match for the TF2 clients in; all of it goes, in order.
    pub(crate) fn voice(&self, v: &VoiceIn) {
        let mut i = lock(&self.0.inner);
        if i.voice.len() >= MAX_VOICE_WAITING {
            i.voice.pop_front();
        }
        i.voice.push_back(VoiceLine::of(v));
        drop(i);
        self.0.wake.notify_all();
    }

    /// As last asked.
    pub(crate) fn clients(&self) -> usize {
        lock(&self.0.inner).clients
    }

    pub(crate) fn stopped(&self) -> bool {
        lock(&self.0.inner).stopped
    }

    pub(crate) fn line(&self) -> Value {
        lock(&self.0.inner).line.clone()
    }

    /// The room ended: so does its relay.
    pub(crate) fn end(&self) {
        self.0.finish(Done::End);
    }
}

impl Drop for RemoteRelay {
    fn drop(&mut self) {
        self.0.finish(Done::LetGo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relays::pool::tests::want;
    use crate::testing::{TempDir, free_port};

    #[test]
    fn a_beam_goes_to_the_relay_process_as_it_was() {
        let b = Beam { from: [1.5, -2.0, 3.0], to: [4.0, 5.0, -6.25], rgb: [255, 128, 0], life: 4.0, width: 3.0, speed: 20 };
        let line: BeamLine = serde_json::from_value(serde_json::to_value(beam_line(&b)).unwrap()).unwrap();
        assert_eq!(beam(&line), b);
    }

    #[test]
    fn the_light_goes_to_the_relay_process_as_it_was_and_none_puts_it_out() {
        let light = Light { at: [1.0, 2.0, 3.0], rgb: [255, 128, 0], radius: 500.0, exponent: 5 };
        let line = json!({ "op": "light", "room": "r", "light": light });
        let Ok(Request::Light { room, light: got }) = serde_json::from_value(line) else { panic!("not a light request") };
        assert_eq!((room.as_str(), got), ("r", Some(light)));
        let out = json!({ "op": "light", "room": "r", "light": null });
        assert!(matches!(serde_json::from_value(out), Ok(Request::Light { light: None, .. })));
    }

    #[test]
    fn notes_go_to_the_relay_process_as_they_were() {
        let note = Note { id: 3, text: "lobby".into(), at: [1.0, 2.0, 3.0], player: Some(7) };
        let line = json!({ "op": "notes", "room": "r", "notes": [note.clone()] });
        let Ok(Request::Notes { room, notes }) = serde_json::from_value(line) else { panic!("not a notes request") };
        assert_eq!((room.as_str(), notes), ("r", vec![note]));
    }

    #[test]
    fn voice_goes_to_the_relay_process_as_it_was() {
        let v = VoiceIn { user: Some(7), steamid: "[U:1:5]".into(), rate: 24000, frames: vec![vec![0x68, 1, 2], vec![3]] };
        let line = json!({ "op": "voice", "room": "r", "voice": [VoiceLine::of(&v)] });
        let Ok(Request::Voice { room, voice }) = serde_json::from_value(line) else { panic!("not a voice request") };
        assert_eq!((room.as_str(), voice.iter().filter_map(VoiceLine::voice).collect::<Vec<_>>()), ("r", vec![v]));
    }

    /// A relay process with one port, serving on a socket in `dir` once this returns.
    fn serving(dir: &TempDir, lease: Duration) -> (PathBuf, u16) {
        static NEVER: AtomicBool = AtomicBool::new(false);
        let socket = dir.join("relay.sock");
        let port = free_port();
        let relays = Relays::new("127.0.0.1", [port]);
        let s = socket.clone();
        std::thread::spawn(move || serve(&s, &relays, lease, &NEVER));
        let until = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < until, "no socket");
            std::thread::sleep(Duration::from_millis(10));
        }
        (socket, port)
    }

    /// Room `room`'s relay, once a port is free again.
    fn start_when_free(remote: &Arc<Remote>, room: &str, was: Option<&Was>) -> RemoteRelay {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(r) = remote.start(room, &want(), was).unwrap() {
                return r;
            }
            assert!(Instant::now() < until, "the port never came back");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_relay_process_that_does_not_answer_is_left_alone_a_while() {
        let dir = TempDir::new("relay-away");
        let socket = dir.join("relay.sock");
        // Never answers.
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let held: Vec<UnixStream> = listener.incoming().flatten().collect();
            drop(held);
        });
        let remote = Remote::new(&socket);
        let first = Instant::now();
        assert!(remote.start("r", &want(), None).is_err());
        assert!(first.elapsed() >= ANSWER, "waited for it once");
        let again = Instant::now();
        assert!(remote.start("r", &want(), None).is_err());
        assert!(again.elapsed() < Duration::from_millis(50), "not again: {:?}", again.elapsed());
    }

    #[test]
    fn nothing_waits_on_a_relay_in_the_relay_process() {
        let dir = TempDir::new("relay-nowait");
        let (socket, _) = serving(&dir, LEASE);
        let remote = Remote::new(&socket);
        let r = remote.start("room", &want(), None).unwrap().unwrap();
        // As a hung relay process would.
        let held = lock(&remote.conn);
        let t = Instant::now();
        for _ in 0..1000 {
            r.follow(&want());
            let _ = (r.clients(), r.stopped(), r.line());
        }
        assert!(t.elapsed() < Duration::from_millis(200), "{:?}", t.elapsed());
        drop(held);
        r.end();
    }

    #[test]
    fn the_relay_process_says_how_many_tf2_clients_it_has() {
        let dir = TempDir::new("stvtf-relay-clients");
        let (socket, _) = serving(&dir, LEASE);
        assert_eq!(clients(&socket).unwrap(), json!({ "clients": 0 }));
        let _relay = start_when_free(&Remote::new(&socket), "r", None);
        assert_eq!(clients(&socket).unwrap(), json!({ "clients": 0 }));
    }

    /// What a process told to stop left: a room's relay comes back on its port and password,
    /// showing what it showed. Too long ago, or not readable, it is left.
    #[test]
    fn a_relay_handed_over_is_taken_in_on_its_port_by_the_next_process() {
        let dir = TempDir::new("stvtf-relay-handed");
        let port = free_port();
        let was = Was { port, password: "abc123".into() };
        let handed = |at_ms| Handed {
            at_ms,
            rooms: vec![HandedRoom {
                room: "r".into(),
                was: was.clone(),
                want: WantLine::of(&want()),
                beams: vec![[0.0; 12]],
                notes: Vec::new(),
                light: None,
                clients: Handover::default(),
            }],
        };
        let file = dir.join("relay.handover");
        let relays = Relays::new("127.0.0.1", [port]);
        for (left, taken) in [
            (serde_json::to_vec(&handed(now_ms())).unwrap(), true),
            (serde_json::to_vec(&handed(now_ms() - 60_000)).unwrap(), false),
            (b"{".to_vec(), false),
        ] {
            std::fs::write(&file, left).unwrap();
            let rooms: Rooms = Arc::default();
            take_over(&rooms, &relays, &file);
            assert!(!file.exists(), "read once");
            let all = lock(&rooms);
            assert_eq!(all.get("r").map(|l| (l.relay.was(), l.shows.beams.len())), taken.then(|| (was.clone(), 1)));
        }
    }

    #[test]
    fn a_relay_process_told_to_stop_leaves_nothing_of_rooms_without_tf2_clients() {
        let dir = TempDir::new("stvtf-relay-none");
        let relays = Relays::new("127.0.0.1", [free_port()]);
        let rooms: Rooms = Arc::default();
        let relay = relays.start(want(), None).unwrap().unwrap();
        lock(&rooms).insert("r".into(), Leased { relay, seen: Instant::now(), shows: Shows::of(want()) });
        let file = dir.join("relay.handover");
        assert_eq!(hand_over(&rooms, &file).unwrap(), 0);
        let left: Handed = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert!(left.rooms.is_empty() && lock(&rooms).is_empty());
    }

    #[test]
    fn a_room_finds_its_relay_again_and_ends_it() {
        let dir = TempDir::new("relay-daemon");
        let (socket, port) = serving(&dir, Duration::from_secs(1));
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
        let b = start_when_free(&next, "room-b", None);
        // Started again with what it was on, as after a restart of the relay process.
        let was = Was::of_line(&b.line()).unwrap();
        b.end();
        let b = start_when_free(&next, "room-b2", Some(&was));
        assert_eq!(Was::of_line(&b.line()), Some(was));
        // Not asked about for a lease, it goes.
        drop(b);
        std::thread::sleep(Duration::from_millis(2500));
        assert!(next.start("room-c", &want(), None).unwrap().is_some(), "the lease ran out");
    }
}
