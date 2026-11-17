//! What is done every so often: saving, sweeping and stopping what nobody watches.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::app::{App, LINGER, Matches, ROOM_LINGER};

/// How often the sessions and rooms are reaped: one housekeeping round.
pub(super) const HOUSEKEEPING: Duration = Duration::from_millis(250);

/// Rounds between saves of the matches: five seconds.
pub(super) const PERSIST_ROUNDS: u64 = 20;

/// Rounds between saves of the rooms: a second.
pub(super) const ROOM_PERSIST_ROUNDS: u64 = 4;

/// Rounds between saves of the voice held.
pub(super) const VOICE_ROUNDS: u64 = (super::voice::hold::SAVED_EVERY.as_millis() / HOUSEKEEPING.as_millis()) as u64;

/// Rounds between sweeps of old files: five minutes.
pub(super) const SWEEP_ROUNDS: u64 = 1200;

/// The ids of the matches whose demos rooms can be relayed from: the sweep leaves them.
pub(super) fn live_ids(matches: &Matches) -> std::collections::HashSet<String> {
    let Matches::Many(sessions) = matches else { return std::collections::HashSet::new() };
    sessions.with_demos()
}

/// Stops idle matches and saves the running ones, and on Ctrl-C disconnects them all cleanly
/// before exiting.
pub(super) fn keep_house(app: Arc<App>, stop: &'static AtomicBool) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Matches::Many(sessions) = &app.matches else { return };
        for round in 0u64.. {
            if stop.load(Ordering::Relaxed) {
                // First, so what is saved is as it stops: the rooms play on meanwhile.
                sessions.finish_giving();
                // The feed up to now, and the voice held, for the process that takes over.
                sessions.persist();
                sessions.save_voice(true);
                app.rooms.persist();
                sessions.stop_all();
                return;
            }
            sessions.reap(LINGER);
            app.rooms.reap(ROOM_LINGER);
            // What a deploy's next container picks up, feed included.
            if round % PERSIST_ROUNDS == 0 {
                sessions.persist();
            }
            // Where each room is, to the second; relayed rooms the process before let go of,
            // and the voice it held.
            if round % ROOM_PERSIST_ROUNDS == 0 {
                app.rooms.persist();
                app.rooms.pick_up_relayed(sessions);
                sessions.adopt_voice();
            }
            if round % VOICE_ROUNDS == 0 {
                sessions.save_voice(false);
            }
            // Old files of rooms, demos and live matches.
            if round % SWEEP_ROUNDS == 0 {
                app.rooms.sweep(&live_ids(&app.matches));
            }
            std::thread::sleep(HOUSEKEEPING);
        }
    })
}
