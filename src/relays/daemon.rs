//! The rooms' relays in a process of their own (`--relay-daemon SOCKET`), so stv.tf can be deployed
//! without the TF2 clients in a room noticing: the relays keep their ports and connections, and a
//! room picked up by the next stv.tf finds its relay again, port and password the same.
//!
//! stv.tf talks to it over a unix socket (`--relay-socket`, on a volume both see), a JSON line per
//! request and per answer:
//!
//! - `{"op": "start", "room", "want", "was", "known"}`: the room's relay, started if it has none
//!   or its own stopped (on the port and password it `was` on, if it can); its page line, null
//!   when every port is taken. A relay it has already plays on where it is, unless the room's
//!   place is `known`.
//! - `{"op": "follow", "room", "want"}`: where the room is now.
//! - `{"op": "status", "room"}`: TF2 clients in, whether it stopped, and its page line.
//! - `{"op": "keep", "room", "beams"}`: lines the TF2 clients in keep seeing, in place of those
//!   before, each `[x1, y1, z1, x2, y2, z2, r, g, b, seconds, width, speed]`.
//! - `{"op": "voice", "room", "voice"}`: voice from outside the match to send them as players'
//!   own, each `{user, steamid, rate, frames}` (frames in base64), in order.
//! - `{"op": "names", "room", "names"}`: the names they see for players and teams, in place of
//!   the game's (a [`stvtf::relay::Names`]).
//! - `{"op": "score", "room", "score"}`: what they see added to the teams' scores, BLU's then
//!   RED's.
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
//! instant. Its camera is `follow` (a player's user id) or `path` (the path's id, age and steps);
//! `fixed`, a spot as a stv.tf from before paths sends it, is still read. A relay nobody asked
//! about for [`LEASE`] (its room gone with an stv.tf that crashed) is stopped.
//!
//! stv.tf never waits on it: a thread per relay talks to it, and one that does not answer is left
//! alone for [`AWAY`].

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stvtf::relay::{Handover, Light, Names, Note, Want};

use crate::matches::sessions::logged;
use crate::relays::pool::{LocalRelay, Relays, Was};
use crate::report::fault;
use crate::sync::lock;

mod remote;
mod wire;

pub(crate) use self::remote::{Remote, RemoteRelay};
use self::wire::{BeamLine, MAX_VOICE_WAITING, Request, VoiceLine, WantLine, beam, beam_line};

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
    names: Names,
    score: [i64; 2],
}

impl Shows {
    fn of(want: Want) -> Shows {
        Shows { want, beams: Vec::new(), notes: Vec::new(), light: None, names: Names::default(), score: [0, 0] }
    }
}

type Rooms = Arc<Mutex<HashMap<String, Leased>>>;

/// A room's relay with TF2 clients in, handed to the next process.
#[derive(Deserialize, Serialize)]
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
    #[serde(default)]
    names: Names,
    #[serde(default)]
    score: [i64; 2],
    clients: Handover,
}

/// What a relay process told to stop leaves the next.
#[derive(Deserialize, Serialize)]
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

/// Stops every relay and writes those with TF2 clients in to `file`. The relays hand over at the
/// same time.
fn hand_over(rooms: &Rooms, file: &Path) -> Result<usize> {
    let all = std::mem::take(&mut *lock(rooms));
    let handed: Vec<HandedRoom> = std::thread::scope(|s| {
        let asked: Vec<_> = all
            .iter()
            .filter(|(_, l)| l.relay.clients() > 0 && !l.relay.stopped())
            .map(|(room, l)| {
                s.spawn(move || {
                    let clients = l.relay.hand_over(HAND_OVER).filter(|h| h.clients() > 0)?;
                    let Shows { want, beams, notes, light, names, score } = l.shows.clone();
                    let want = WantLine::of(&want);
                    Some(HandedRoom { room: room.clone(), was: l.relay.was(), want, beams, notes, light, names, score, clients })
                })
            })
            .collect();
        asked.into_iter().filter_map(|t| t.join().ok().flatten()).collect()
    });
    let clients = handed.iter().map(|r| r.clients.clients()).sum();
    let part = file.with_extension("handover.part");
    // `at_ms` is now: each want's age counts from here.
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
        Err(e) => return fault!("the clients handed over do not read: {e}"),
    };
    let ago = Duration::from_millis(now_ms().saturating_sub(handed.at_ms));
    if ago > HANDED_FOR {
        return tracing::info!("clients handed over {} s ago are gone", ago.as_secs());
    }
    for HandedRoom { room, was, want, beams, notes, light, names, score, clients } in handed.rooms {
        let shows = Shows { want: want.aged(ago).want(), beams, notes, light, names, score };
        let started = relays.start_with(shows.want.clone(), Some(&was), Some(&clients));
        match started {
            Ok(Some(relay)) if relay.was() == was => {
                tracing::info!(
                    "relay of room {} taken over on port {}, {} clients, after {} ms",
                    logged(&room),
                    was.port,
                    clients.clients(),
                    ago.as_millis()
                );
                relay.keep(shows.beams.iter().map(beam).collect());
                relay.keep_notes(shows.notes.clone());
                relay.keep_light(shows.light);
                relay.keep_names(shows.names.clone());
                relay.keep_score(shows.score);
                lock(rooms).insert(room, Leased { relay, seen: Instant::now(), shows });
            }
            Ok(_) => fault!("relay of room {} not taken over: port {} is not free", logged(&room), was.port),
            Err(e) => fault!("relay of room {} not taken over: {e:#}", logged(&room)),
        }
    }
}

/// What the relay process on `socket` answers when asked for its TF2 clients: `{"clients": N}`,
/// or an error from one that does not know the question (and hands none over).
pub(crate) fn clients(socket: &Path) -> Result<Value> {
    let mut conn = UnixStream::connect(socket).with_context(|| format!("cannot reach the relays at {}", socket.display()))?;
    conn.set_read_timeout(Some(ANSWER))?;
    writeln!(conn, "{}", json!({ "op": "clients" }))?;
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

/// Serves relays on `socket` until told to `stop`: then its TF2 clients are handed over (see
/// [`handed_at`]) and the process ends.
pub(crate) fn serve(socket: &Path, relays: &Arc<Relays>, lease: Duration, stop: &'static AtomicBool) -> Result<()> {
    // A socket file left by a process before would be in the way.
    let _ = std::fs::remove_file(socket);
    let rooms: Rooms = Arc::default();
    take_over(&rooms, relays, &handed_at(socket));
    let listener = UnixListener::bind(socket).with_context(|| format!("cannot listen on {}", socket.display()))?;
    tracing::info!("relays at {}", socket.display());
    let (leaving, file) = (rooms.clone(), handed_at(socket));
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(STOP_POLL);
        }
        match hand_over(&leaving, &file) {
            Ok(clients) => tracing::info!("relays stopping: {clients} clients handed over"),
            Err(e) => fault!("relays stopping: clients not handed over: {e:#}"),
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
                    tracing::info!("relay of room {} stopped", logged(room));
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
        let answer = match serde_json::from_str::<Value>(&line?).and_then(|v| Ok((Request::deserialize(&v)?, v))) {
            // In the trace of the stv.tf request that asked, tagged with its room.
            Ok((r, v)) => {
                let (op, room) = (v["op"].as_str().unwrap_or_default(), v["room"].as_str().unwrap_or_default());
                crate::report::continued(&format!("relay {op}"), v["trace"].as_str(), &[("room", room)], || handle(r, rooms, relays))
            }
            Err(e) => json!({ "error": format!("{e}") }),
        };
        writeln!(out, "{answer}")?;
    }
    Ok(())
}

/// Logs how starting a room's relay went: its port, which tells whose the relay's own log lines
/// are (never its password), or why there is none.
fn said_of(room: &str, started: &Result<Option<LocalRelay>>) {
    match started {
        Ok(Some(relay)) => tracing::info!("relay of room {} started on port {}", logged(room), relay.was().port),
        Ok(None) => fault!("no relay for room {}: every port is taken", logged(room)),
        Err(e) => fault!("no relay for room {}: {e:#}", logged(room)),
    }
}

/// Room `room`'s relay, started if it has none or its own stopped; its page line.
fn start(room: String, want: WantLine, known: bool, mut was: Option<Was>, rooms: &Rooms, relays: &Arc<Relays>) -> Value {
    let mut all = lock(rooms);
    if let Some(l) = all.get_mut(&room) {
        if !l.relay.stopped() {
            l.seen = Instant::now();
            if known {
                l.shows.want = want.want();
                l.relay.follow(l.shows.want.clone());
            }
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
        Request::Start { room, want, was, known } => start(room, want, known, was, rooms, relays),
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
        Request::Names { room, names } => in_room(rooms, &room, |l| {
            l.shows.names.clone_from(&names);
            l.relay.keep_names(names);
            json!({})
        }),
        Request::Score { room, score } => in_room(rooms, &room, |l| {
            l.shows.score = score;
            l.relay.keep_score(score);
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

#[cfg(test)]
mod tests;
