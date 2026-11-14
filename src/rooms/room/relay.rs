//! A room's relay to TF2 clients: started while someone is on a page, told where the room is.

use super::play::State;
use super::{Room, Saved};
use crate::lock;
use crate::matches::hub::announce;
use crate::matches::sessions::logged;
use crate::relays::pool::{Offer, RoomRelay};
use base64::Engine;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use stvtf::Line;
use stvtf::relay::VoiceIn;

/// A relay that could not start is tried again after this long.
pub(super) const RELAY_RETRY: Duration = Duration::from_secs(5);

/// A relay is told again when off by more than this (it jumps at a second).
pub(super) const RELAY_DRIFT: Duration = Duration::from_millis(250);

/// Whether a relay told `sent` is where `now` is without being told again: same course, within
/// [`RELAY_DRIFT`], on a server ticking every `interval` seconds.
pub(super) fn on_course(sent: &stvtf::relay::Want, now: &stvtf::relay::Want, interval: f64) -> bool {
    let same = sent.demo == now.demo
        && sent.paused == now.paused
        // One of a few set values.
        && sent.speed.to_bits() == now.speed.to_bits()
        && sent.from_start == now.from_start
        && sent.camera == now.camera;
    let gone = if sent.paused { 0.0 } else { now.at.saturating_duration_since(sent.at).as_secs_f64() * sent.speed };
    let expected = f64::from(sent.tick) + gone / interval.max(1e-3);
    same && (f64::from(now.tick) - expected).abs() * interval <= RELAY_DRIFT.as_secs_f64()
}

impl Room {
    /// The room can be relayed to TF2 clients, from the demos `offer` says.
    pub(crate) fn offer_relay(&self, offer: Offer) {
        let _ = self.offer.set(offer);
        if self.wants_relay() {
            self.ensure_relay();
        }
    }

    /// Someone is on a page, or it had a relay (TF2 clients may be in it).
    pub(super) fn wants_relay(&self) -> bool {
        lock(&self.people).listed() > 0 || lock(&self.relay.was).is_some()
    }

    /// Whether `saved` had a relay.
    pub(crate) fn relayed(saved: &Saved) -> bool {
        saved.relay.is_some()
    }

    /// Starts its relay if it can have one and has none. Takes the state's lock and the relay's
    /// one at a time: the player holds the state's while it points the relay.
    pub(super) fn ensure_relay(&self) {
        let Some(offer) = self.offer.get() else { return };
        if lock(&self.relay.current).is_some() {
            return;
        }
        if lock(&self.relay.failed).as_ref().is_some_and(|(t, _)| t.elapsed() < RELAY_RETRY) {
            return;
        }
        // A recording picked up is not at the match's latest until its file is taken: the relay
        // is, and plays on.
        let (want, known) = {
            let st = lock(&self.state);
            (self.want(&st, offer, true), st.tick.is_some() && !self.recording.is_catching_up())
        };
        let Some(want) = want else { return };
        let was = lock(&self.relay.was).clone();
        match offer.relays.start(&self.id, want.clone(), was.as_ref(), known) {
            Ok(Some(r)) => {
                *lock(&self.relay.failed) = None;
                let mut relay = lock(&self.relay.current);
                // Another page started one meanwhile: this one goes.
                if relay.is_none() {
                    if let Some(was) = r.was() {
                        *lock(&self.relay.was) = Some(was);
                    }
                    // A stand-in is not where the relay is: the room's own place goes once known.
                    *lock(&self.relay.sent) = known.then_some(want);
                    *relay = Some(r);
                    // A new relay has no marker yet.
                    self.marker.dirty.store(true, Ordering::Relaxed);
                }
            }
            Ok(None) => {}
            Err(e) => {
                let why = format!("{e:#}");
                let mut failed = lock(&self.relay.failed);
                if failed.as_ref().is_none_or(|(_, before)| *before != why) {
                    log!("room {}: {why}", logged(&self.id));
                }
                *failed = Some((Instant::now(), why));
            }
        }
        self.tell_relay();
    }

    /// Where its relay should be, as playback is in `st`: at the tick last played, or the first of
    /// the demo before any (`any`).
    pub(super) fn want(&self, st: &State, offer: &Offer, any: bool) -> Option<stvtf::relay::Want> {
        let tick = st.tick.or(any.then_some(0))?;
        let part = self.part(st);
        Some(stvtf::relay::Want {
            demo: (offer.demo_of)(part.load, tick)?,
            tick,
            at: Instant::now(),
            paused: st.paused,
            speed: st.speed,
            from_start: false,
            camera: lock(&self.camera).map(|player| stvtf::relay::Camera::Follow { player }),
        })
    }

    /// Points the relay at where playback is, when it changed course: the relay plays on by itself.
    pub(super) fn steer_relay(&self, st: &State) {
        let Some(offer) = self.offer.get() else { return };
        let interval = lock(&self.hub).tick_interval();
        let relay = lock(&self.relay.current);
        let Some(r) = &*relay else { return };
        if self.recording.is_catching_up() {
            return;
        }
        let Some(want) = self.want(st, offer, false) else { return };
        let mut sent = lock(&self.relay.sent);
        if sent.as_ref().is_some_and(|s| on_course(s, &want, interval)) {
            return;
        }
        r.follow(want.clone());
        *sent = Some(want);
    }

    pub(super) fn relay_clients(&self) -> usize {
        lock(&self.relay.current).as_ref().map_or(0, RoomRelay::clients)
    }

    /// Tells the pages how to connect in TF2, when that changed.
    pub(super) fn tell_relay(&self) {
        let line = lock(&self.relay.current).as_ref().map_or_else(|| serde_json::json!({ "type": "relay" }), RoomRelay::line);
        let line = line.to_string();
        let mut said = lock(&self.relay.said);
        if said.as_deref() != Some(line.as_str()) {
            announce(&self.hub, "relay", &line.clone().into());
            *said = Some(line);
        }
    }

    /// Looks after its relay, as the rooms are looked after: one that stopped (its demo cannot be
    /// read, say) goes, one wanted and missing (the demo not written yet, every port taken) is
    /// tried again, and the pages hear why it cannot play as soon as that is known.
    pub(crate) fn tend_relay(&self) {
        let stopped = lock(&self.relay.current).as_ref().is_some_and(RoomRelay::stopped);
        if stopped && let Some(r) = lock(&self.relay.current).take() {
            r.end();
            *lock(&self.relay.sent) = None;
        }
        let missing = lock(&self.relay.current).is_none();
        if missing && self.wants_relay() {
            self.ensure_relay();
        }
        self.tell_relay();
        self.keep_marker_if_changed();
    }
}

/// What a relay must send of `line` as the room plays it: voice from outside the match (the
/// demo it plays has the match's own), unless said in a pause. Frames that are not base64 are
/// left out.
pub(super) fn relay_voice(line: &Line) -> Option<VoiceIn> {
    let Line::Voice(v) = line else { return None };
    if !v.injected || v.pause {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let frames: Vec<Vec<u8>> = v.frames.iter().filter_map(|f| b64.decode(f).ok()).collect();
    (!frames.is_empty()).then(|| VoiceIn { user: v.player.map(|p| p.0), steamid: v.steamid.clone(), rate: v.rate, frames })
}

impl Room {
    /// Gives the relay what it must send of a line being played.
    pub(super) fn relay_line(&self, line: &Line) {
        let Some(v) = relay_voice(line) else { return };
        #[cfg(test)]
        lock(&self.relayed).push(v.clone());
        if let Some(r) = &*lock(&self.relay.current) {
            r.voice(v);
        }
    }
}
