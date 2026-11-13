//! One match: what it watches or plays, who watches it, and its recording for rooms.

use super::store::{Store, dem_beside, demo_dem, live_file, to_file};
use crate::lock;
use crate::matches::hub::{SharedHub, Summary, subscribe_all};
use crate::matches::live_demos::LiveDemos;
use crate::matches::recording::Recording;
use crate::matches::replay;
use crate::relays::pool::DemoOf;
use crate::voice::feed::VoiceFeed;
use crate::voice::mumble::MumbleJob;
use anyhow::{Result, bail};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// In play: at least this many players on the teams…
pub(super) const IN_PLAY_PLAYERS: usize = 2;
/// …and data no older than this.
pub(super) const IN_PLAY_DATA: Duration = Duration::from_secs(60);
/// How much of the id the log shows.
pub(super) const LABEL_ID_LEN: usize = 8;

pub struct Session {
    pub id: String,
    pub hub: SharedHub,
    pub(super) server: SocketAddr,
    pub(super) password: String,
    /// Who started it (a Steam id from the start token), for the log; it limits nothing.
    pub(super) owner: Option<String>,
    /// The site's reservation the server belongs to, 0 for none: the latest start link that said.
    reservation: AtomicU64,
    /// A fixed session (the demo): always there, outside the match limit.
    pub(super) pinned: bool,
    /// The demos.tf demo it plays instead of watching `server`.
    pub(super) demo: Option<u64>,
    pub(super) stop: AtomicBool,
    /// Set when the watcher gave up (a wrong password, say) or was stopped.
    pub(super) done: AtomicBool,
    viewers: AtomicUsize,
    /// For the log: the most viewers at once, and how many streams were opened in all.
    peak: AtomicUsize,
    views: AtomicUsize,
    started: Instant,
    pub(super) idle_since: Mutex<Option<Instant>>,
    pub(super) thread: Mutex<Option<JoinHandle<()>>>,
    /// The hub's `changes` when the match was last saved. Held while it is saved and while it is
    /// ended, so a save that started before the match ended cannot write its file back after
    /// `end` deleted it.
    pub(super) saving: Mutex<Option<u64>>,
    /// What a room on it plays, once known.
    pub(super) source: Mutex<Option<Source>>,
    pub(super) recording: OnceLock<Arc<Recording>>,
    /// A live match's recording, from when it started being watched.
    live: OnceLock<Arc<Recording>>,
    /// Whether `prepare` started.
    preparing: AtomicBool,
    /// A live match's demos, when there is a store to keep them in.
    pub(super) demos: OnceLock<Arc<LiveDemos>>,
    /// Who started it, for the limit per person: a Steam id or an address.
    pub(super) by: OnceLock<String>,
    /// Voice from outside the match (a team's Discord), and whether a thread tends it.
    pub(crate) voice: VoiceFeed,
    pub(super) voice_tended: AtomicBool,
    /// What a Discord channel's voice comes in with: given to whoever started the match, and to
    /// the site's streamers, to pass on with the bot's command.
    pub(super) voice_invite: OnceLock<String>,
    /// The Mumble channel the voice bot is wanted in for it, if any.
    pub(crate) mumble: Mutex<Vec<MumbleJob>>,
    /// How it goes in each Discord channel the voice bot is in for it, and when it last said.
    pub(crate) discord: Mutex<std::collections::BTreeMap<String, (String, Instant)>>,
}

/// A recorded match: a saved file, or a demo's gzipped output.
pub(super) enum Source {
    File(PathBuf),
    Output(Arc<Vec<u8>>),
}

/// How long a voice invite is, in hex digits.
const INVITE_LEN: usize = 16;

impl Session {
    /// A session not watched yet; a pinned one is never idle, so never reaped.
    pub(super) fn new(id: String, hub: SharedHub, server: SocketAddr, password: String, owner: Option<String>, pinned: bool) -> Self {
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
            by: OnceLock::new(),
            voice: VoiceFeed::default(),
            voice_tended: AtomicBool::new(false),
            voice_invite: OnceLock::new(),
            mumble: Mutex::default(),
            discord: Mutex::default(),
        }
    }

    /// The match's voice invite: a secret made once, kept with the match.
    pub(crate) fn voice_invite(&self) -> &str {
        self.voice_invite.get_or_init(|| super::random_id()[..INVITE_LEN].to_string())
    }

    /// Whether the visitor known as `key` (see `web::visitor::Who`) started it.
    pub(crate) fn started_by(&self, key: &str) -> bool {
        self.owner.as_deref().is_some_and(|o| crate::rooms::people::steam_key(o) == key)
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

    /// Where the `.dem` files of what it plays are, by map load and server tick, so rooms on it can be relayed:
    /// a live match's as it records them, a demos.tf demo's, kept in `store`, or a fixed match's,
    /// beside its recording under the same name (`demo.dem` for `demo.jsonl.gz`).
    pub(crate) fn demo_of(&self, store: Option<&Store>) -> Option<DemoOf> {
        if let Some(d) = self.demos.get() {
            let d = d.clone();
            return Some(Arc::new(move |map, again, tick| d.at(map, again, tick)));
        }
        let path = match &*lock(&self.source) {
            Some(Source::File(file)) => dem_beside(file),
            _ => store?.dir.join(demo_dem(self.demo?)),
        };
        Some(Arc::new(move |_, _, _| path.exists().then(|| path.clone())))
    }

    /// Rooms play `file`, the recording this session loops.
    pub fn set_file(&self, file: PathBuf) {
        *lock(&self.source) = Some(Source::File(file));
    }

    /// Whether a room can be opened on it now.
    pub fn recordable(&self) -> bool {
        self.live.get().is_some() || self.recording.get().is_some() || lock(&self.source).is_some()
    }

    /// Whether its watcher gave up or was stopped: a demo that could not be fetched never has
    /// anything to play.
    pub fn gave_up(&self) -> bool {
        self.done.load(Ordering::Relaxed)
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
                log!("match {}: preparing a room: {e:#}", s.label());
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
            Some(Source::File(path)) => Recording::read(replay::open(path)?, &name)?,
            Some(Source::Output(gz)) => Recording::read(flate2::read::GzDecoder::new(gz.as_slice()), &name)?,
            None if self.demo.is_some() => bail!("the demo is still being fetched"),
            None => bail!("a live match has no recording"),
        };
        Ok(self.recording.get_or_init(|| Arc::new(read)).clone())
    }

    pub fn reservation(&self) -> Option<u64> {
        Some(self.reservation.load(Ordering::Relaxed)).filter(|&r| r != 0)
    }

    pub(super) fn link(&self, reservation: Option<u64>) {
        if let Some(r) = reservation {
            self.reservation.store(r, Ordering::Relaxed);
        }
    }

    /// Whether a live match is being played: players on the teams, and data coming in. Nobody
    /// watching such a match does not end it (see [`super::Sessions::reap`]).
    pub(super) fn in_play(&self) -> bool {
        if self.pinned || self.demo.is_some() {
            return false;
        }
        let h = lock(&self.hub);
        let fresh = h.data_age().is_some_and(|age| age < IN_PLAY_DATA);
        let players = h
            .summary()
            .snapshot
            .map_or(0, |s| s.players.iter().filter(|p| matches!(p.team, stvtf::output::Team::Red | stvtf::output::Team::Blue)).count());
        fresh && players >= IN_PLAY_PLAYERS
    }

    /// Whether any browser has its event stream open.
    pub fn watched(&self) -> bool {
        self.viewers.load(Ordering::Relaxed) > 0
    }

    /// The start of the id (the whole id is the link) and the map.
    pub(super) fn label(&self) -> String {
        let map = lock(&self.hub).map.clone();
        format!("{} {}", &self.id[..self.id.len().min(LABEL_ID_LEN)], map.as_deref().unwrap_or("-"))
    }

    /// Not the demo nor a demos.tf demo.
    pub(crate) fn watches_server(&self) -> bool {
        !self.pinned && self.demo.is_none()
    }

    /// Whether it watches a live match now: a room on it records it.
    pub fn is_live(&self) -> bool {
        self.watches_server() && !self.done.load(Ordering::Relaxed)
    }

    pub(super) fn listed(&self) -> Listed {
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

    pub(super) fn ended(&self, why: &str) {
        log!(
            "match {} ended ({why}) after {} min, peak {} viewers, {} views",
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
        log!("match {} viewers {n}", s.label());
        *lock(&s.idle_since) = None;
        Viewer(s)
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let n = self.0.viewers.fetch_sub(1, Ordering::Relaxed) - 1;
        log!("match {} viewers {n}", self.0.label());
        if n == 0 {
            *lock(&self.0.idle_since) = Some(Instant::now());
        }
    }
}

/// How long the recorder waits for a line before it looks whether to stop.
pub(super) const RECORD_POLL: Duration = Duration::from_millis(250);

/// Records live match `s` from now on, to its file when there is a store (`resume`: going on from
/// what the process before saved), until the match ends. The feed from before is not recorded
/// again as new.
pub(super) fn record(s: &Arc<Session>, store: Option<&Store>, resume: bool) {
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
    // Everything: detail, voice, and what is only kept.
    let (lines, s) = (subscribe_all(&s.hub), s.clone());
    std::thread::spawn(move || {
        while s.is_live() && !s.stop.load(Ordering::Relaxed) {
            let json = match lines.recv_timeout(RECORD_POLL) {
                Ok(json) => json,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if json.contains(r#""history":true"#) {
                continue;
            }
            let Ok(mut stamped) = serde_json::from_str::<stvtf::output::Stamped>(&json) else { continue };
            stamped.time = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64));
            if let (stvtf::Line::Snapshot(snap), Some(demos)) = (&stamped.line, s.demos.get()) {
                demos.reached(snap.tick);
            }
            let _ = recording.push(&stamped, &json);
        }
        recording.end();
    });
}

/// Joins `s`, a match already running, for `owner`; logged, so the log shows everyone who asked
/// for a match and not only who started it.
pub(super) fn join(s: &Arc<Session>, owner: Option<&str>) -> Arc<Session> {
    log!("match {} opened, owner {}", s.label(), owner.unwrap_or("-"));
    s.clone()
}
