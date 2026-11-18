//! The score a room's admins say a game stands at, where the game's own is off (a first half
//! played before, a restart): kept as what to add to the game's, for that game only. A game is a
//! map from one reset of its score to the next: the next map and a restart start from the game's
//! own again. Every page is told what to add where the room is now, and so is its relay, for
//! those watching in TF2.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use stvtf::output::Snapshot;

use super::Room;
use super::play::State;
use crate::matches::hub::announce;
use crate::matches::recording::Mark;
use crate::sync::lock;

/// The highest score an admin gives.
const MAX_SCORE: i64 = 99;
/// A snapshot's score may trail a round won by this much play time (ms): lower than the win's
/// only after it is a reset.
const TRAILS_MS: u64 = 5000;
/// How often the game the room is in is looked up while its score stays as it is.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// The most games with a score of their own.
const MAX_GAMES: usize = 32;

/// The score a team should show now, as an admin posts it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScoreEdit {
    pub(crate) team: stvtf::output::Team,
    pub(crate) score: i64,
}

/// What to add to a game's score: the game by its map's number and the play time of the round
/// won before it began (0 for the map's first).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(crate) struct Added {
    load: usize,
    game: u64,
    blue: i64,
    red: i64,
}

#[derive(Default)]
pub(super) struct Scores {
    pub(super) added: Vec<Added>,
    /// The game's own score at the last snapshot played, blue's then red's, and the game it is of.
    own: [i64; 2],
    game: (usize, u64),
    /// What the pages were last told to add.
    told: [i64; 2],
    looked: Option<Instant>,
}

/// The game at play time `at` of a map whose rounds won were `wins` (play time, blue and red
/// after it), the score there being `own`: the play time of the last round of the game before.
fn game_at(wins: &[(u64, [u16; 2])], at: u64, own: [i64; 2]) -> u64 {
    let total = |s: [u16; 2]| i64::from(s[0]) + i64::from(s[1]);
    let so_far: Vec<_> = wins.iter().filter(|w| w.0 <= at).collect();
    let mut game = 0;
    for pair in so_far.windows(2) {
        if total(pair[1].1) <= total(pair[0].1) {
            game = pair[0].0;
        }
    }
    match so_far.last() {
        Some(last) if at > last.0 + TRAILS_MS && own[0] + own[1] < total(last.1) => last.0,
        _ => game,
    }
}

impl Room {
    /// The rounds won on the room's map, with the score after each.
    fn wins(&self, st: &State) -> Vec<(u64, [u16; 2])> {
        let part = self.part(st);
        let within = |m: &Mark| m.at >= part.origin && part.end.is_none_or(|end| m.at < end);
        self.recording.marks(0).0.iter().filter(|m| m.kind == "win" && within(m)).filter_map(|m| Some((m.at, m.score?))).collect()
    }

    /// A snapshot was played: the pages are told what to add to its score, when that changed.
    pub(super) fn score_played(&self, st: &State, snap: &Snapshot) {
        self.score_seen(st, [snap.score.blue, snap.score.red], false);
    }

    /// The room went to a moment where the game's score is `own`.
    pub(super) fn score_at(&self, st: &State, own: [i64; 2]) {
        self.score_seen(st, own, true);
    }

    fn score_seen(&self, st: &State, own: [i64; 2], moved: bool) {
        let mut scores = lock(&self.scores);
        if scores.added.is_empty() && scores.told == [0, 0] {
            scores.own = own;
            scores.looked = None;
            return;
        }
        let same = scores.own == own && scores.game.0 == st.load && scores.looked.is_some_and(|t| t.elapsed() < LOOK_EVERY);
        scores.own = own;
        if moved || !same {
            scores.game = (st.load, game_at(&self.wins(st), st.at, own));
            scores.looked = Some(Instant::now());
            self.tell_score(&mut scores);
        }
    }

    /// Tells the pages what to add to the score of the game the room is in, if that changed.
    fn tell_score(&self, scores: &mut Scores) {
        let (load, game) = scores.game;
        let add = scores.added.iter().find(|a| a.load == load && a.game == game).map_or([0, 0], |a| [a.blue, a.red]);
        if add != scores.told {
            scores.told = add;
            announce(&self.hub, "score", &score_line(add));
            if let Some(r) = &*lock(&self.relay.current) {
                r.keep_score(add);
            }
        }
    }

    /// An admin says what a team's score is now: kept for the game the room is in, as what to
    /// add to the game's own.
    pub(crate) fn set_score(&self, edit: &ScoreEdit) -> Result<()> {
        let side = match edit.team {
            stvtf::output::Team::Blue => 0,
            stvtf::output::Team::Red => 1,
            _ => bail!("Invalid team."),
        };
        if !(0..=MAX_SCORE).contains(&edit.score) {
            bail!("From 0 to {MAX_SCORE}.");
        }
        let st = lock(&self.state);
        let mut scores = lock(&self.scores);
        let own = scores.own;
        let (load, game) = (st.load, game_at(&self.wins(&st), st.at, own));
        scores.game = (load, game);
        let was = scores.added.iter().position(|a| a.load == load && a.game == game).map(|i| scores.added.remove(i));
        let mut add = was.map_or([0, 0], |a| [a.blue, a.red]);
        add[side] = edit.score - own[side];
        let [blue, red] = add;
        if add != [0, 0] {
            if scores.added.len() >= MAX_GAMES {
                scores.added.remove(0);
            }
            scores.added.push(Added { load, game, blue, red });
        }
        self.tell_score(&mut scores);
        Ok(())
    }

    /// The scores from before (a room picked up again).
    pub(super) fn keep_scores(&self, added: Vec<Added>) {
        lock(&self.scores).added = added;
    }

    /// What the pages were last told to add, for a relay just started.
    pub(super) fn score_told(&self) -> [i64; 2] {
        lock(&self.scores).told
    }

    pub(super) fn scores_added(&self) -> Vec<Added> {
        lock(&self.scores).added.clone()
    }
}

fn score_line(add: [i64; 2]) -> Arc<str> {
    serde_json::json!({ "type": "score", "blue": add[0], "red": add[1] }).to_string().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_is_a_map_from_one_reset_of_its_score_to_the_next() {
        // Two rounds, a restart, then two more.
        let wins = [(10_000, [1, 0]), (20_000, [1, 1]), (40_000, [0, 1]), (50_000, [1, 1])];
        assert_eq!(game_at(&wins, 5_000, [0, 0]), 0);
        assert_eq!(game_at(&wins, 25_000, [1, 1]), 0, "the final score still stands");
        assert_eq!(game_at(&wins, 21_000, [1, 0]), 0, "a snapshot trailing the round won");
        assert_eq!(game_at(&wins, 30_000, [0, 0]), 20_000, "restarted: no round of the next won yet");
        assert_eq!(game_at(&wins, 45_000, [0, 1]), 20_000);
        assert_eq!(game_at(&wins, 60_000, [1, 1]), 20_000);
        assert_eq!(game_at(&[], 60_000, [3, 2]), 0);
    }

    use std::fmt::Write as _;

    use stvtf::Line;
    use stvtf::output::{RoundWin, Scores as WinScores, Team};

    use super::super::play::Control;
    use crate::matches::hub::subscribe;
    use crate::matches::hub::tests::snapshot;
    use crate::matches::recording::Recording;
    use crate::rooms::room::tests::until;

    /// A minute: BLU wins a round a quarter in, the game is restarted at the half.
    fn room_of_two_games() -> Arc<Room> {
        let mut lines = String::new();
        let mut put = |line: &Line| writeln!(lines, "{}", serde_json::to_string(line).unwrap()).unwrap();
        for tick in (4..=4000).step_by(4) {
            let mut line = snapshot(tick, None, Vec::new());
            if let Line::Snapshot(s) = &mut line {
                s.score.blue = i64::from((1000..2000).contains(&tick));
            }
            put(&line);
            if tick == 1000 {
                put(&Line::RoundWin(RoundWin { tick, team: Team::Blue, reason: 1, round_time: 15.0 }));
                put(&Line::Scores(WinScores { tick, red: 0, blue: 1, game_over: false }));
            }
        }
        Room::start("r".into(), "me", Arc::new(Recording::read(lines.as_bytes(), "test").unwrap()))
    }

    fn at(room: &Room, ms: u64) {
        room.control(&Control { paused: Some(true), seek: Some(ms), ..Control::default() }).unwrap();
        until("the seek", || lock(&room.state).seek.is_none() && lock(&room.state).at == ms);
    }

    /// What the pages were last told to add.
    fn set(room: &Room, team: Team, score: i64) -> Result<()> {
        room.set_score(&ScoreEdit { team, score })
    }

    fn told(room: &Room) -> [i64; 2] {
        let line = subscribe(&room.hub, false).try_iter().filter(|l| l.contains(r#""type":"score""#)).last();
        let line: serde_json::Value = serde_json::from_str(&line.expect("a score line")).unwrap();
        [line["blue"].as_i64().unwrap(), line["red"].as_i64().unwrap()]
    }

    #[test]
    fn a_score_set_right_holds_for_its_game_and_not_the_next() {
        let room = room_of_two_games();
        at(&room, 20_000);
        // 1-0 by the game: 3-2 it is, a first half carried over.
        set(&room, Team::Blue, 3).unwrap();
        set(&room, Team::Red, 2).unwrap();
        assert_eq!(told(&room), [2, 2]);
        at(&room, 5_000);
        assert_eq!(told(&room), [2, 2], "from the game's start");
        at(&room, 45_000);
        assert_eq!(told(&room), [0, 0], "restarted: the game's own");
        set(&room, Team::Red, 1).unwrap();
        assert_eq!(told(&room), [0, 1]);
        at(&room, 20_000);
        assert_eq!(told(&room), [2, 2]);
        assert_eq!(room.saved().scores.len(), 2);
        // Set to the game's own: nothing kept for it.
        set(&room, Team::Blue, 1).unwrap();
        assert_eq!(told(&room), [0, 2]);
        assert_eq!(room.saved().scores.len(), 2);
        set(&room, Team::Red, 0).unwrap();
        assert_eq!(told(&room), [0, 0]);
        assert_eq!(room.saved().scores.len(), 1);
        assert_eq!(set(&room, Team::Red, 100).unwrap_err().to_string(), "From 0 to 99.");
        assert_eq!(set(&room, Team::Spectator, 1).unwrap_err().to_string(), "Invalid team.");
    }
}
