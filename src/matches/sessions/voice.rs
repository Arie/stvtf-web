//! A live match's voice from outside it (see `voice::feed`): taken in, and tended by a
//! thread of its own while there is any.

use super::session::Session;
use crate::voice::feed::Speaker;
use crate::voice::hold::Took;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

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
        if matches!(took, Took::Held { .. } | Took::PauseTalk) && !self.comms.tended.swap(true, Ordering::AcqRel) {
            let s = self.clone();
            std::thread::spawn(move || s.tend_voice());
        }
        Some(took)
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
