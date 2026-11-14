//! A live match's voice from outside it (see `voice::feed`): taken in, and tended by a
//! thread of its own while there is any.
//!
//! What is held is saved in the store, a file per match and process (`held-<match>-<process>`):
//! every `SAVED_EVERY` where it changed, and as the process stops. The process the store
//! belongs to takes the others' files into its own matches and drops them: the release a deploy
//! replaces saves until it stops, and a process that crashed leaves what it last saved.

use super::session::Session;
use super::store::{Store, age};
use super::{Sessions, is_id};
use crate::lock;
use crate::voice::feed::{Adopt, MAX_DELAY, Save, SavedVoice, Speaker};
use crate::voice::hold::Took;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// What the names of the files of held voice start with.
const HELD_PREFIX: &str = "held-";
/// What a file being taken ends with: a save meanwhile makes a new one, taken next.
const TAKING: &str = ".taking";
/// Nothing in a file saved this long ago is still due: the longest delay, and some.
const HELD_STALE: Duration = Duration::from_secs(MAX_DELAY as u64 + 60);

/// The file of what the process with mark `process` holds of match `id`.
fn held_file(id: &str, process: &str) -> String {
    format!("{HELD_PREFIX}{id}-{process}")
}

/// The match and the process a file of held voice is of.
fn held_of(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix(HELD_PREFIX)?.split_once('-').filter(|(id, process)| is_id(id) && is_id(process))
}

impl Sessions {
    /// Saves what each live match holds of voice, where that changed; `last`, as this process
    /// stops: all of it, and they take no more. A release taken over saves too: voice comes in
    /// until the proxy switches.
    pub fn save_voice(&self, last: bool) {
        let Some(store) = &self.store else { return };
        for s in self.live() {
            self.save_voice_of(store, &s, last);
        }
    }

    fn save_voice_of(&self, store: &Store, s: &Session, last: bool) {
        let name = held_file(&s.id, &self.claim);
        let saved = match s.comms.feed.save(&s.hub, last) {
            Save::Same => return,
            Save::Nothing => return store.forget(&name),
            Save::Held(saved) => saved,
        };
        // An ended match's file is forgotten: not written again.
        let _saving = lock(&s.saving);
        if s.stop.load(Ordering::Relaxed) {
            return;
        }
        if let Err(e) = serde_json::to_vec(&saved).map_err(Into::into).and_then(|json| store.put(&name, &json)) {
            log!("saving match {}'s held voice: {e:#}", s.id);
        }
    }

    /// Takes the voice other processes saved into this one's matches, once each has a stream,
    /// and drops their files, with those of no match and those saved long ago. Nothing once a
    /// newer process took the store over: they are that one's.
    pub fn adopt_voice(&self) {
        let Some(store) = &self.store else { return };
        if store.taken_over(&self.claim) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&store.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(HELD_PREFIX) {
                continue;
            }
            if age(&entry).is_ok_and(|a| a > HELD_STALE) {
                store.forget(&name);
            } else if let Some((id, from)) = held_of(&name).filter(|(_, from)| *from != self.claim) {
                self.adopt_voice_in(store, &name, id, from);
            }
        }
    }

    /// Takes file `name`, what process `from` holds of match `id`.
    fn adopt_voice_in(&self, store: &Store, name: &str, id: &str, from: &str) {
        let Some(s) = self.lock().get(id).cloned() else {
            // Not picked up yet (see `Sessions::get`), unless it ended.
            if !store.path(id).exists() {
                store.forget(name);
            }
            return;
        };
        if !s.has_stream() {
            return;
        }
        // Under a name of its own first: what `from` saves meanwhile is not dropped unread.
        let taking = format!("{name}{TAKING}");
        if std::fs::rename(store.path(name), store.path(&taking)).is_err() {
            return;
        }
        match store.take(&taking).and_then(|json| Ok(serde_json::from_slice::<SavedVoice>(&json)?)) {
            Ok(saved) => {
                if s.adopt_voice(from, saved) == Adopt::Taken {
                    // Saved as this process's before the file goes.
                    self.save_voice_of(store, &s, false);
                }
            }
            Err(e) => log!("dropping match {}'s unreadable held voice: {e:#}", s.id),
        }
        store.forget(&taking);
    }

    /// Forgets what this process saved of match `id`'s held voice.
    pub(super) fn forget_voice(&self, store: &Store, id: &str) {
        store.forget(&held_file(id, &self.claim));
    }
}

/// How often the feed looks at the stream: three frames of voice, so a line carries a few and a
/// player's queue (100 ms ahead) rides it out.
const TEND_EVERY: Duration = Duration::from_millis(60);

impl Session {
    /// `who` said `frames` (base64 Opus) just now, in a source `delay` seconds behind
    /// the game (see `VoiceFeed::take_in`). None for a match not watched live.
    pub fn take_in_voice(self: &Arc<Self>, who: &Speaker, frames: Vec<String>, delay: Option<f64>) -> Option<Took> {
        if !self.is_live() {
            return None;
        }
        let took = self.comms.feed.take_in(&self.hub, who, frames, delay, Instant::now());
        self.keep_spoken();
        if matches!(took, Took::Held { .. } | Took::PauseTalk) {
            self.tend();
        }
        Some(took)
    }

    /// Takes the voice process `from` held of this match, as it saved it.
    fn adopt_voice(self: &Arc<Self>, from: &str, saved: SavedVoice) -> Adopt {
        let adopted = self.comms.feed.adopt(&self.hub, from, saved, Instant::now());
        if adopted == Adopt::Taken {
            self.tend();
        }
        adopted
    }

    /// Whether its stream has a tick to hold voice against.
    pub(super) fn has_stream(&self) -> bool {
        lock(&self.hub).voice_clock().tick.is_some()
    }

    /// A thread tends the feed from now on, unless one does.
    fn tend(self: &Arc<Self>) {
        if !self.comms.tended.swap(true, Ordering::AcqRel) {
            let s = self.clone();
            std::thread::spawn(move || s.tend_voice());
        }
    }

    /// Tends the feed while it holds anything, the match is live and not stopped: what is due goes
    /// out. Voice that comes just as it is done is tended on, not left for the next.
    fn tend_voice(&self) {
        let going = || self.is_live() && !self.stop.load(Ordering::Relaxed);
        loop {
            while going() {
                std::thread::sleep(TEND_EVERY);
                let now = Instant::now();
                self.comms.feed.step(&self.hub, now);
                self.keep_spoken();
                if self.comms.feed.idle(now) {
                    break;
                }
            }
            self.comms.tended.store(false, Ordering::Release);
            if !going() || self.comms.feed.idle(Instant::now()) || self.comms.tended.swap(true, Ordering::AcqRel) {
                return;
            }
        }
    }

    /// What players said goes beside the demo being recorded, to download with the comms in it.
    fn keep_spoken(&self) {
        let spoken = self.comms.feed.take_spoken();
        if let Some(demos) = self.demos.get() {
            for s in &spoken {
                demos.append_voice(s);
            }
        }
    }
}
