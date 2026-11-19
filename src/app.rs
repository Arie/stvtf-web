//! The app every request and thread shares: its matches, rooms, voice and settings.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::sync::lock;
use crate::matches::hub::SharedHub;
use crate::matches::sessions::Sessions;
use crate::web::MAX_STREAMS_PER_CLIENT;

/// How long a match nobody watches keeps its `SourceTV` connection.
pub(super) const LINGER: Duration = Duration::from_secs(180);

/// A room ends this long after its last viewer left: a reload does not end it.
pub(super) const ROOM_LINGER: Duration = Duration::from_secs(60);

/// Whether `s` is a name safe to use in a path: letters, digits and the bytes in `extra`.
pub(super) fn is_name(s: &str, extra: &[u8]) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

/// What the server watches: one match at `/`, or any number at `/w/<id>/`.
pub(super) enum Matches {
    One(SharedHub),
    Many(Sessions),
}

pub(super) struct App {
    pub(super) matches: Matches,
    pub(super) cache: PathBuf,
    /// The backpack pictures and `items.json`.
    pub(super) backpack: PathBuf,
    /// One build of each map at a time; a map being built does not hold up the others.
    pub(super) maps: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(super) overviews: Mutex<()>,
    /// Where a proxy serves us, like `/live`; empty at the root.
    pub(super) base: String,
    /// When set, matches start only from tokens a site in front of us made.
    pub(super) keys: Option<Arc<super::web::token::Keys>>,
    /// The site in front: where people sign in.
    pub(super) home: Option<String>,
    /// The leagues' names for a room's players.
    pub(super) leagues: super::leagues::Leagues,
    /// The cameras casters placed on maps.
    pub(super) cameras: super::cameras::Cameras,
    /// Open event streams by client address.
    pub(super) streams: Arc<Mutex<HashMap<IpAddr, usize>>>,
    pub(super) rooms: super::rooms::registry::Rooms,
    /// Team voice comms.
    pub(super) voice: super::voice::VoiceSetup,
}

/// Counts an event stream against its client's limit until dropped.
pub(super) struct Stream(Arc<Mutex<HashMap<IpAddr, usize>>>, IpAddr);

impl Drop for Stream {
    fn drop(&mut self) {
        let mut streams = lock(&self.0);
        if let Some(n) = streams.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                streams.remove(&self.1);
            }
        }
    }
}

impl App {
    /// Whether a match being watched, or a room, is on `map`: only those maps are fetched and
    /// built on request.
    pub(super) fn on_map(&self, map: &str) -> bool {
        let watched = match &self.matches {
            Matches::One(hub) => lock(hub).map.as_deref() == Some(map),
            Matches::Many(sessions) => sessions.on_map(map),
        };
        watched || self.rooms.on_map(map)
    }

    /// The lock around building map `name`.
    pub(super) fn map_lock(&self, name: &str) -> Arc<Mutex<()>> {
        lock(&self.maps).entry(name.to_string()).or_default().clone()
    }

    /// A slot for one more event stream from `client`, or None when it has too many open.
    pub(super) fn stream_slot(&self, client: IpAddr) -> Option<Stream> {
        let mut streams = lock(&self.streams);
        let n = streams.entry(client).or_default();
        if *n >= MAX_STREAMS_PER_CLIENT {
            return None;
        }
        *n += 1;
        Some(Stream(self.streams.clone(), client))
    }
}
