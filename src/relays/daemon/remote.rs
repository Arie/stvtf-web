//! The relay process as stv.tf sees it: a relay there for each room, each talked to from a thread
//! of its own.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use stvtf::relay::{Beam, Light, Names, Note, VoiceIn, Want};

use super::{ANSWER, AWAY, BeamLine, MAX_VOICE_WAITING, STATUS_EVERY, VoiceLine, WantLine, beam_line};
use crate::relays::pool::Was;
use crate::sync::lock;

/// stv.tf's end: one connection, shared by its rooms, opened again when it breaks.
pub(crate) struct Remote {
    pub(super) socket: PathBuf,
    pub(super) conn: Mutex<Option<(UnixStream, BufReader<UnixStream>)>>,
    /// Not asked until then: it did not answer.
    pub(super) away: Mutex<Option<Instant>>,
}

impl Remote {
    pub(crate) fn new(socket: &Path) -> Arc<Remote> {
        Arc::new(Remote { socket: socket.to_owned(), conn: Mutex::new(None), away: Mutex::new(None) })
    }

    /// One request and its answer, in the trace of the request being answered.
    pub(super) fn call(&self, request: &Value) -> Result<Value> {
        self.call_in(request, crate::report::trace_header())
    }

    /// One request and its answer, in `trace`; after one with no answer, calls fail at once for [`AWAY`].
    pub(super) fn call_in(&self, request: &Value, trace: Option<String>) -> Result<Value> {
        let mut traced = None;
        let request = match trace {
            Some(t) => traced.insert({
                let mut r = request.clone();
                r["trace"] = t.into();
                r
            }),
            None => request,
        };
        let mut conn = lock(&self.conn);
        if lock(&self.away).is_some_and(|until| Instant::now() < until) {
            bail!("cannot hear from the relays at {}", self.socket.display());
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
    pub(super) fn exchange(&self, conn: &mut Option<(UnixStream, BufReader<UnixStream>)>, request: &Value) -> Result<Value> {
        let mut tried = None;
        for _ in 0..2 {
            if conn.is_none() {
                let s =
                    UnixStream::connect(&self.socket).with_context(|| format!("cannot reach the relays at {}", self.socket.display()))?;
                s.set_read_timeout(Some(ANSWER))?;
                s.set_write_timeout(Some(ANSWER))?;
                *conn = Some((s.try_clone()?, BufReader::new(s)));
            }
            let Some((w, r)) = conn.as_mut() else { continue };
            let mut line = String::new();
            let sent = writeln!(w, "{request}").map_err(anyhow::Error::from).and_then(|()| Ok(r.read_line(&mut line)?));
            match sent {
                Ok(n) if n > 0 => return Ok(serde_json::from_str(&line)?),
                Ok(_) => tried = Some(anyhow::anyhow!("cannot hear from the relays: they hung up")),
                Err(e) => tried = Some(e),
            }
            *conn = None;
        }
        Err(tried.unwrap_or_else(|| anyhow::anyhow!("cannot get an answer")))
    }

    /// Room `room`'s relay, started if it has none, playing as `want` says, on what it `was` on if
    /// it can; None when every port is taken. A relay it has already plays on where it is, unless
    /// the room's place is `known`.
    pub(crate) fn start(self: &Arc<Self>, room: &str, want: &Want, was: Option<&Was>, known: bool) -> Result<Option<RemoteRelay>> {
        let answer = self.call(&json!({ "op": "start", "room": room, "want": WantLine::of(want), "was": was, "known": known }))?;
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
                names: None,
                score: None,
                voice: VecDeque::new(),
                notes_given: Vec::new(),
                light: None,
                light_given: false,
                light_due: false,
                done: None,
                trace: None,
            }),
            wake: Condvar::new(),
        });
        let tending = shared.clone();
        std::thread::spawn(move || tending.tend());
        Ok(Some(RemoteRelay(shared)))
    }
}

/// How a relay's thread stops.
#[derive(Copy, Clone, Eq, PartialEq)]
pub(super) enum Done {
    /// The relay plays on, for the next stv.tf.
    LetGo,
    /// The relay stops too: the room ended.
    End,
}

pub(super) struct Inner {
    pub(super) clients: usize,
    pub(super) stopped: bool,
    pub(super) line: Value,
    /// Where the room is, not sent yet.
    pub(super) want: Option<Want>,
    /// The kept beams to hand over, the latest only.
    pub(super) kept: Option<Vec<Beam>>,
    /// The notes to hand over, the latest only.
    pub(super) notes: Option<Vec<Note>>,
    /// The names to hand over, the latest only.
    pub(super) names: Option<Names>,
    /// What is added to the scores, to hand over, the latest only.
    pub(super) score: Option<[i64; 2]>,
    /// Voice to hand over, all of it, in order.
    pub(super) voice: VecDeque<VoiceLine>,
    /// The notes last given, so that the same ones are not sent again: the marker gives them with
    /// every piece it draws.
    pub(super) notes_given: Vec<Note>,
    /// The light last given (the marker gives it with every piece it draws), whether one ever was,
    /// and whether it is still to send.
    pub(super) light: Option<Light>,
    pub(super) light_given: bool,
    pub(super) light_due: bool,
    pub(super) done: Option<Done>,
    /// The trace of the last request that gave it something to send.
    pub(super) trace: Option<String>,
}

pub(super) struct Shared {
    pub(super) remote: Arc<Remote>,
    pub(super) room: String,
    pub(super) inner: Mutex<Inner>,
    pub(super) wake: Condvar,
}

impl Shared {
    /// Hands the relay what its clients keep seeing, of what changed: beams, notes, names and
    /// what is added to the scores.
    pub(super) fn hand_kept<R>(
        &self,
        call: &impl Fn(&Value) -> R,
        kept: Option<Vec<Beam>>,
        notes: Option<Vec<Note>>,
        (names, score): (Option<Names>, Option<[i64; 2]>),
    ) {
        if let Some(kept) = kept {
            let kept: Vec<BeamLine> = kept.iter().map(beam_line).collect();
            call(&json!({ "op": "keep", "room": self.room, "beams": kept }));
        }
        if let Some(notes) = notes {
            call(&json!({ "op": "notes", "room": self.room, "notes": notes }));
        }
        if let Some(names) = names {
            call(&json!({ "op": "names", "room": self.room, "names": names }));
        }
        if let Some(score) = score {
            call(&json!({ "op": "score", "room": self.room, "score": score }));
        }
    }

    /// Sends where the room is, and asks how the relay is, until let go of or ended.
    pub(super) fn tend(&self) {
        let mut asked = Instant::now();
        loop {
            let (want, kept, notes, names, score, voice, light, done, trace) = {
                let mut i = lock(&self.inner);
                while i.want.is_none()
                    && i.kept.is_none()
                    && i.notes.is_none()
                    && i.names.is_none()
                    && i.score.is_none()
                    && i.voice.is_empty()
                    && !i.light_due
                    && i.done.is_none()
                {
                    let left = STATUS_EVERY.saturating_sub(asked.elapsed());
                    if left.is_zero() {
                        break;
                    }
                    i = self.wake.wait_timeout(i, left).unwrap_or_else(PoisonError::into_inner).0;
                }
                let voice: Vec<VoiceLine> = i.voice.drain(..).collect();
                (
                    i.want.take(),
                    i.kept.take(),
                    i.notes.take(),
                    i.names.take(),
                    i.score.take(),
                    voice,
                    std::mem::take(&mut i.light_due).then_some(i.light),
                    i.done,
                    i.trace.take(),
                )
            };
            let call = |request: &Value| self.remote.call_in(request, trace.clone());
            match done {
                Some(Done::End) => {
                    let _ = call(&json!({ "op": "stop", "room": self.room }));
                    return;
                }
                Some(Done::LetGo) => return,
                None => {}
            }
            if let Some(want) = want {
                let answer = call(&json!({ "op": "follow", "room": self.room, "want": WantLine::of(&want) }));
                if answer.is_ok_and(|a| a.get("gone").is_some()) {
                    lock(&self.inner).stopped = true;
                }
            }
            self.hand_kept(&call, kept, notes, (names, score));
            if !voice.is_empty() {
                let _ = call(&json!({ "op": "voice", "room": self.room, "voice": voice }));
            }
            if let Some(light) = light {
                let _ = call(&json!({ "op": "light", "room": self.room, "light": light }));
            }
            if asked.elapsed() >= STATUS_EVERY {
                asked = Instant::now();
                self.ask_status();
            }
        }
    }

    /// Asks the relay process how the relay is.
    pub(super) fn ask_status(&self) {
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
            // The relay process is away (restarting): what was last known stands.
            Err(_) => {}
        }
    }

    pub(super) fn finish(&self, done: Done) {
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
        let mut i = lock(&self.0.inner);
        i.want = Some(want.clone());
        traced(&mut i);
        drop(i);
        self.0.wake.notify_all();
    }

    /// Beams the TF2 clients in keep seeing, in place of those before; only the latest is sent.
    pub(crate) fn keep(&self, beams: Vec<Beam>) {
        let mut i = lock(&self.0.inner);
        i.kept = Some(beams);
        traced(&mut i);
        drop(i);
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
        traced(&mut i);
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
        traced(&mut i);
        drop(i);
        self.0.wake.notify_all();
    }

    /// The names the TF2 clients in see for players and teams; only the latest is sent.
    pub(crate) fn keep_names(&self, names: Names) {
        let mut i = lock(&self.0.inner);
        i.names = Some(names);
        traced(&mut i);
        drop(i);
        self.0.wake.notify_all();
    }

    /// What the TF2 clients in see added to the teams' scores; only the latest is sent.
    pub(crate) fn keep_score(&self, add: [i64; 2]) {
        let mut i = lock(&self.0.inner);
        i.score = Some(add);
        traced(&mut i);
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
        traced(&mut i);
        drop(i);
        self.0.wake.notify_all();
    }

    /// TF2 clients in, as of the last status asked.
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

/// What is to send goes in the trace of the request that gave it, if one did.
pub(super) fn traced(i: &mut Inner) {
    if let Some(t) = crate::report::trace_header() {
        i.trace = Some(t);
    }
}

impl Drop for RemoteRelay {
    fn drop(&mut self) {
        self.0.finish(Done::LetGo);
    }
}
