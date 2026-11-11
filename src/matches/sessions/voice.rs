//! A live match's voice from outside it (see `matches::voice_feed`): taken in, and tended by a
//! thread of its own while there is any.

use super::session::Session;
use crate::matches::voice_hold::Took;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How often the feed looks at the stream: a frame of voice.
const TEND_EVERY: Duration = Duration::from_millis(20);

impl Session {
    /// `who` (a `SteamID`) said `frames` (base64 Opus) just now, in a source `delay` seconds behind
    /// the game (see `VoiceFeed::said`). None for a match not watched live.
    pub fn voice_said(self: &Arc<Self>, who: &str, frames: Vec<String>, delay: Option<f64>) -> Option<Took> {
        if !self.is_live() {
            return None;
        }
        let took = self.voice.said(&self.hub, who, frames, delay, Instant::now());
        if matches!(took, Took::Held { .. } | Took::PauseTalk) && !self.voice_tended.swap(true, Ordering::AcqRel) {
            let s = self.clone();
            std::thread::spawn(move || {
                while s.is_live() && !s.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(TEND_EVERY);
                    let now = Instant::now();
                    s.voice.step(&s.hub, now);
                    if s.voice.idle(now) {
                        break;
                    }
                }
                s.voice_tended.store(false, Ordering::Release);
            });
        }
        Some(took)
    }
}
