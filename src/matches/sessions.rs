//! Many matches in one process: a watcher per `SourceTV` server, started when someone asks for
//! it and stopped a while after its last viewer leaves. With a store, the matches outlive the
//! process: a deploy's new container picks them up under the same links.

use crate::cli::{DEFAULT_TV_PASSWORD, parse_connect};
use crate::lock;
use crate::matches::demostf;
use crate::matches::hub::{SharedHub, watch_into};
use crate::matches::live_demos::LiveDemos;
use anyhow::{Result, anyhow, bail};
use record::record;
use session::{Source, join};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
/// A live match in play stays watched with nobody watching it for this long, at most.
const UNWATCHED_PLAY: Duration = Duration::from_hours(6);
/// A match nobody has watched for this long makes room for a new one when there are as many as
/// there may be (one just started has no viewer yet either).
pub(crate) const MAKES_ROOM: Duration = Duration::from_mins(3);
/// Matches one person may have started at once (see [`Sessions::open_as`]).
pub(crate) const MATCHES_PER_PERSON: usize = 4;
/// The server of a match that watches none: the demo, a demos.tf demo.
const NOWHERE: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));

mod address;
mod ids;
mod record;
mod session;
mod store;
mod voice;

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
    /// The matches watched live now.
    pub(crate) fn live(&self) -> Vec<Arc<Session>> {
        lock(&self.map).values().filter(|s| s.is_live()).cloned().collect()
    }

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
        self.open_as(None, line, owner, reservation)
    }

    /// [`Sessions::open`], a new match counting against `by` (none: a streamer, the console).
    pub fn open_as(&self, by: Option<&str>, line: &str, owner: Option<&str>, reservation: Option<u64>) -> Result<Arc<Session>> {
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
        self.check_person(&mut map, by)?;
        self.check_room(&mut map)?;
        let s = self.start(&mut map, random_id(), server, password, owner.map(str::to_string), SharedHub::default());
        if let Some(by) = by {
            let _ = s.by.set(by.to_string());
        }
        record(&s, self.store.as_ref(), false);
        s.link(reservation);
        log!("match {} started on {server}, owner {}", s.label(), owner.unwrap_or("-"));
        if let Some(store) = &self.store
            && let Err(e) = store.save(&s)
        {
            log!("saving match {}: {e:#}", s.id);
        }
        Ok(s)
    }

    /// The session playing demos.tf demo `demo`, joining one already playing it (where it is now)
    /// or starting it from the beginning; `owner` as for [`Sessions::open`], a new one counting
    /// against `by` as for [`Sessions::open_as`]. It is not saved for the next process.
    pub fn open_demo_as(&self, by: Option<&str>, demo: u64, owner: Option<&str>) -> Result<Arc<Session>> {
        let mut map = self.lock();
        if let Some(s) = map.values().find(|s| s.demo == Some(demo) && !s.done.load(Ordering::Relaxed)) {
            return Ok(join(s, owner));
        }
        self.check_person(&mut map, by)?;
        self.check_room(&mut map)?;
        let mut session = Session::new(demo_id(demo), SharedHub::default(), NOWHERE, String::new(), owner.map(str::to_string), false);
        session.demo = Some(demo);
        if let Some(by) = by {
            let _ = session.by.set(by.to_string());
        }
        let s = Arc::new(session);
        let kept = self.store.clone().map(|store| demostf::Kept { store, dem: self.demos });
        let (player, hz) = (s.clone(), self.hz);
        let thread = std::thread::spawn(move || {
            if let Err(e) = demostf::play(&player.hub, demo, hz, &|| player.watched(), &player.stop, kept.as_ref(), &|gz| {
                *lock(&player.source) = Some(Source::Output(gz));
            }) {
                log!("match {} (demo {demo}) gave up: {e:#}", player.label());
            }
            player.done.store(true, Ordering::Relaxed);
        });
        *lock(&s.thread) = Some(thread);
        map.insert(s.id.clone(), s.clone());
        log!("match {} started on demo {demo}, owner {}", s.label(), owner.unwrap_or("-"));
        Ok(s)
    }

    /// Matches that count towards `max`.
    pub fn running(&self) -> usize {
        self.lock().values().filter(|s| !s.pinned).count()
    }

    /// Whether another match may start: there must be fewer than `max`. When there are not, the
    /// one nobody has watched the longest, for [`MAKES_ROOM`] at least (kept on for its players,
    /// see [`Sessions::reap`]), makes room.
    fn check_room(&self, map: &mut HashMap<String, Arc<Session>>) -> Result<()> {
        if map.values().filter(|s| !s.pinned).count() < self.max {
            return Ok(());
        }
        let Some(s) = longest_unwatched(map.values().filter(|s| !s.pinned)).and_then(|id| map.remove(&id)) else {
            bail!("already watching {} matches, the most this server does", self.max);
        };
        self.end(&s, "making room");
        Ok(())
    }

    /// Whether `by` may start another match: fewer than [`MATCHES_PER_PERSON`] of theirs, or one
    /// nobody has watched for [`MAKES_ROOM`], which ends.
    fn check_person(&self, map: &mut HashMap<String, Arc<Session>>, by: Option<&str>) -> Result<()> {
        let Some(by) = by else { return Ok(()) };
        let theirs: Vec<&Arc<Session>> =
            map.values().filter(|s| s.by.get().is_some_and(|b| b == by) && !s.done.load(Ordering::Relaxed)).collect();
        if theirs.len() < MATCHES_PER_PERSON {
            return Ok(());
        }
        let Some(s) = longest_unwatched(theirs).and_then(|id| map.remove(&id)) else {
            bail!("You are watching {MATCHES_PER_PERSON} matches already: close one first.");
        };
        self.end(&s, "making room for its starter's next");
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
                log!("match {} gave up: {e}", watcher.label());
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
            log!("taking over {}: {e:#}", store.dir.display());
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
        let Ok(saved) = store.load(id) else {
            log!("dropping unreadable saved match {id}");
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
        if let Some(by) = saved.by {
            let _ = s.by.set(by);
        }
        if let Some(invite) = saved.voice_invite {
            let _ = s.voice_invite.set(invite);
        }
        *lock(&s.mumble) = saved.mumble;
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
                log!("saving match {}: {e:#}", s.id);
            }
        }
    }

    /// A fixed session at `/w/<id>/` fed by `feed` (a recorded match on a loop, say). It is never
    /// reaped and nothing waits for its thread.
    pub fn pin(&self, id: &str, feed: impl FnOnce(&Session) + Send + 'static) {
        let s = Arc::new(Session::new(id.to_string(), SharedHub::default(), NOWHERE, String::new(), None, true));
        let session = s.clone();
        std::thread::spawn(move || feed(&session));
        self.lock().insert(id.to_string(), s);
    }

    /// Stops and forgets the sessions nobody has watched for `linger`, but for a live match still
    /// in play (see [`Session::in_play`]), which goes on being watched (and recorded, for those
    /// who come back to it) for up to [`UNWATCHED_PLAY`]. Their watchers disconnect on their own;
    /// nothing waits for them.
    pub fn reap(&self, linger: Duration) {
        let idle: Vec<Arc<Session>> = {
            let mut map = self.lock();
            let idle: Vec<String> = map
                .values()
                .filter(|s| !s.pinned)
                .filter(|s| {
                    lock(&s.idle_since).is_some_and(|t| {
                        let idle = t.elapsed();
                        idle >= linger && (idle >= UNWATCHED_PLAY || !s.in_play())
                    })
                })
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

/// The id of the one of `sessions` nobody has watched the longest, for [`MAKES_ROOM`] at least.
fn longest_unwatched<'a>(sessions: impl IntoIterator<Item = &'a Arc<Session>>) -> Option<String> {
    sessions
        .into_iter()
        .filter_map(|s| lock(&s.idle_since).filter(|t| t.elapsed() >= MAKES_ROOM).map(|t| (t, s.id.clone())))
        .min_by_key(|(t, _)| *t)
        .map(|(_, id)| id)
}

#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod tests;
pub(crate) use address::is_public;
pub(crate) use ids::{demo_id, id_demo, is_id, logged, random_id};
pub(crate) use session::{Listed, Session, Viewer};
pub(crate) use store::{DEMO_PREFIX, LIVE_PREFIX, Store, demo_dem, demo_output, live_file};
