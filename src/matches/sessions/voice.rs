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
        let took = self.voice.take_in(&self.hub, who, frames, delay, Instant::now());
        self.keep_spoken();
        if matches!(took, Took::Held { .. } | Took::PauseTalk) && !self.voice_tended.swap(true, Ordering::AcqRel) {
            let s = self.clone();
            std::thread::spawn(move || {
                loop {
                    while s.is_live() && !s.stop.load(Ordering::Relaxed) {
                        std::thread::sleep(TEND_EVERY);
                        let now = Instant::now();
                        s.voice.step(&s.hub, now);
                        s.keep_spoken();
                        if s.voice.idle(now) {
                            break;
                        }
                    }
                    s.voice_tended.store(false, Ordering::Release);
                    // Voice that came just as this was done would wait for the next: tended on.
                    let done = !s.is_live() || s.stop.load(Ordering::Relaxed) || s.voice.idle(Instant::now());
                    if done || s.voice_tended.swap(true, Ordering::AcqRel) {
                        break;
                    }
                }
            });
        }
        Some(took)
    }

    /// What players said goes beside the demo being recorded, to download with the comms in it.
    fn keep_spoken(&self) {
        let spoken = self.voice.take_spoken();
        if let Some(demos) = self.demos.get() {
            for s in &spoken {
                demos.append_voice(s);
            }
        }
    }
}
