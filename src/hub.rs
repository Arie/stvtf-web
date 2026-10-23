//! The latest state of a match and the queues of the pages watching it.

use crate::lock;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use stvtf::Line;
use stvtf::output::{PlayerId, Status, Team, round1};
use stvtf::watch::Watch;

/// The latest match state for browsers that join late, and a queue per browser.
#[derive(Default)]
pub(crate) struct Hub {
    status: Option<Arc<str>>,
    server: Option<Arc<str>>,
    world: Option<Arc<str>>,
    snapshot: Option<Arc<str>>,
    /// With whether the page asked for detail lines.
    clients: Vec<(SyncSender<Arc<str>>, bool)>,
    pub(crate) map: Option<String>,
    /// Feed events of the current map, so a page opened late still shows kills and chat.
    pub(crate) history: VecDeque<Arc<str>>,
    /// Tick and round time of the latest snapshot, to stamp events with time into the round.
    round_clock: Option<(u32, f64)>,
    /// When the latest snapshot came in, so a page opened after the stream stopped can say so.
    snapshot_at: Option<Instant>,
    /// Name and team of everyone seen on this map: the feed keeps them for players who left.
    names: HashMap<PlayerId, Seen>,
    /// Seconds per tick, from the server line.
    tick_interval: Option<f64>,
}

/// Seconds per tick of a server that has not said yet.
pub(crate) fn default_tick_interval() -> f64 {
    f64::from(stvtf::DEFAULT_TICK_INTERVAL)
}

/// A player's name and team, as last seen in a snapshot.
#[derive(Debug, Clone, Serialize)]
struct Seen {
    name: String,
    team: Team,
}

const HISTORY: usize = 1000;

/// Lines queued for a page; one that falls this far behind is dropped and reconnects.
const PAGE_QUEUE: usize = 4096;

/// Where a line goes besides every page: into the latest state, the feed history, or (for the
/// high-volume detail lines) only to pages that ask for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Kept as the latest status, server, world or snapshot.
    State,
    /// Kept in the history, so a page opened late still shows kills and chat.
    Feed,
    /// Only for pages that ask (`?detail=1`); never kept.
    Detail,
    /// Passed on and not kept.
    Live,
}

fn route(line: &Line) -> Route {
    match line {
        Line::Status(_) | Line::Server(_) | Line::World(_) | Line::Snapshot(_) => Route::State,
        Line::Kill(_)
        | Line::Flag(_)
        | Line::Uber(_)
        | Line::UberReady(_)
        | Line::MedicDeath(_)
        | Line::CaptureBlocked(_)
        | Line::BuildingDestroyed(_)
        | Line::Banner(_)
        | Line::PointCaptured(_)
        | Line::RoundStart(_)
        | Line::RoundWin(_)
        | Line::Scores(_)
        | Line::Stalemate(_)
        | Line::Overtime(_)
        | Line::GameOver(_)
        | Line::PlayerLeave(_)
        | Line::TeamChange(_)
        | Line::Chat(_)
        | Line::CaptureBroken(_)
        | Line::KilledCapper(_)
        | Line::MedicDefended(_)
        | Line::VoteOptions(_)
        | Line::VoteCast(_)
        | Line::Tournament(_)
        | Line::Announcer(_)
        | Line::TimeRemaining(_)
        | Line::PassScore(_)
        | Line::PassStolen(_)
        | Line::PassBlocked(_)
        | Line::BallCarrierKilled(_)
        | Line::RobotKilled(_)
        | Line::SpecialScore(_)
        | Line::TeamLeaderKilled(_)
        | Line::RocketOpen(_)
        | Line::PointLocked(_)
        | Line::Sapped(_)
        | Line::VoteStarted(_)
        | Line::VotePassed(_)
        | Line::VoteFailed(_) => Route::Feed,
        Line::Heal(_)
        | Line::BlastJump(_)
        | Line::Pickup(_)
        | Line::Impact(_)
        | Line::Mark(_)
        | Line::VoiceCommand(_)
        | Line::ServerText(_)
        | Line::BuildingRepair(_)
        | Line::PassGet(_)
        | Line::PassFree(_)
        | Line::PassCaught(_)
        | Line::PointsScored(_)
        | Line::Stun(_)
        | Line::CaptureStarted(_)
        | Line::HudText(_)
        | Line::Hint(_)
        | Line::RoundSelected(_)
        | Line::Teleported(_)
        | Line::BuildingRemoved(_)
        | Line::BuildingDetonated(_)
        | Line::DetStickies(_)
        | Line::NpcDamage(_)
        | Line::Upgrade(_) => Route::Detail,
        Line::Damage(_)
        | Line::Spawn(_)
        | Line::UberEnded(_)
        | Line::ClassChange(_)
        | Line::Build(_)
        | Line::BuildingUpgrade(_)
        | Line::TimeAdded(_)
        | Line::RoundActive(_)
        | Line::SetupFinished(_)
        | Line::Shot(_)
        | Line::Explosion(_)
        | Line::SentryShot(_)
        | Line::Airblast(_)
        | Line::Reflect(_) => Route::Live,
    }
}

/// A feed line as kept in the history: marked as such, with the names of the players it names
/// (they may have left by the time a page shows it) and its time into the round.
#[derive(Serialize)]
struct Past<'a> {
    #[serde(flatten)]
    line: &'a Line,
    history: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    names: BTreeMap<PlayerId, &'a Seen>,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<f64>,
}

/// How long ago the latest snapshot came in, for a page that joins.
#[derive(Serialize)]
#[serde(tag = "type", rename = "data_age")]
struct DataAge {
    ms: u64,
}

pub(crate) type SharedHub = Arc<Mutex<Hub>>;

pub(crate) fn publish(hub: &SharedHub, line: &Line) {
    let json: Arc<str> = serde_json::to_string(line).expect("a line serializes").into();
    let route = route(line);
    let mut h = lock(hub);
    match line {
        Line::Status(status) => {
            if *status == Status::MapChange {
                h.snapshot = None;
                h.history.clear();
                h.names.clear();
            }
            h.status = Some(json.clone());
        }
        Line::Server(server) => {
            // Signon restarts announce the same map again; only a new map starts a new history.
            if h.map.as_deref() != Some(server.map.as_str()) {
                h.history.clear();
                h.names.clear();
                h.map = Some(server.map.clone());
            }
            (h.server, h.snapshot, h.round_clock) = (Some(json.clone()), None, None);
            h.tick_interval = Some(server.tick_interval).filter(|i| *i > 0.0);
        }
        Line::World(_) => h.world = Some(json.clone()),
        Line::Snapshot(snapshot) => {
            h.round_clock = snapshot.round_time.map(|t| (snapshot.tick, t));
            h.snapshot = Some(json.clone());
            h.snapshot_at = Some(Instant::now());
            for p in &snapshot.players {
                // Every snapshot names everyone: reuse the string rather than allocate a new one.
                let seen = h.names.entry(p.id).or_insert_with(|| Seen { name: String::new(), team: p.team });
                seen.name.clone_from(&p.name);
                seen.team = p.team;
            }
        }
        _ if route == Route::Feed => {
            let interval = h.tick_interval.unwrap_or_else(default_tick_interval);
            let at = h.round_clock.zip(line.tick()).map(|((tick, time), at)| {
                let seconds = time + (f64::from(at) - f64::from(tick)) * interval;
                round1(seconds).max(0.0)
            });
            let names = line.players().filter_map(|id| Some((id, h.names.get(&id)?))).collect();
            let past = serde_json::to_string(&Past { line, history: true, names, at }).expect("a line serializes");
            if h.history.len() == HISTORY {
                h.history.pop_front();
            }
            h.history.push_back(past.into());
        }
        _ => {}
    }
    // A browser that cannot keep up is dropped; it reconnects and resyncs from the latest state.
    let detail = route == Route::Detail;
    h.clients.retain(|(c, wants)| {
        (detail && !wants) || !matches!(c.try_send(json.clone()), Err(TrySendError::Full(_) | TrySendError::Disconnected(_)))
    });
}

/// Watches a `SourceTV` server into `hub` until `stop`; a watch that gives up says so to the pages.
pub(crate) fn watch_into(
    hub: &SharedHub,
    target: &str,
    password: &str,
    hz: f32,
    stop: &AtomicBool,
) -> Result<(), stvtf::watch::WatchError> {
    let mut w = Watch::new(target, password);
    w.snapshot_hz = hz;
    let result = stvtf::watch::watch(&w, stop, &mut |line| publish(hub, &line));
    if let Err(e) = &result {
        publish(hub, &Line::Status(Status::GaveUp { reason: e.to_string() }));
    }
    result
}

pub(crate) fn subscribe(hub: &SharedHub, detail: bool) -> Receiver<Arc<str>> {
    let (tx, rx) = sync_channel(PAGE_QUEUE);
    let mut h = lock(hub);
    // The snapshot goes before the history so the page knows the players' names.
    for line in [&h.status, &h.server, &h.world, &h.snapshot].into_iter().flatten().chain(&h.history) {
        let _ = tx.try_send(line.clone());
    }
    if let Some(at) = h.snapshot_at {
        let age = DataAge { ms: at.elapsed().as_millis() as u64 };
        let _ = tx.try_send(serde_json::to_string(&age).expect("a number serializes").into());
    }
    h.clients.push((tx, detail));
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use stvtf::output::{self, PlayerId, Team};

    fn player(id: u16, name: &str, team: Team) -> output::Player {
        output::Player {
            id: PlayerId(id),
            ent: u32::from(id),
            name: name.to_string(),
            steamid: "BOT".to_string(),
            team,
            class: output::Class::Scout,
            alive: true,
            hp: 125,
            max_hp: 125,
            pos: Some([0.0; 3]),
            yaw: Some(0.0),
            pitch: Some(0.0),
            uber: 0,
            ping: 0,
            conds: Vec::new(),
            ubered: None,
            medigun: None,
            weapon: None,
            flames: None,
            respawn: None,
            score: None,
            carrying: None,
        }
    }

    fn snapshot(tick: u32, round_time: Option<f64>, players: Vec<output::Player>) -> Line {
        Line::Snapshot(output::Snapshot {
            tick,
            players,
            projectiles: Vec::new(),
            buildings: Vec::new(),
            objectives: Vec::new(),
            round: output::Round::Running,
            round_time,
            time_left: None,
            win_limit: None,
            match_time: None,
            score: output::Score { red: 0, blue: 0 },
            captures: None,
            points: None,
            mvm: None,
            timers: Vec::new(),
        })
    }

    fn kill(tick: u32, attacker: u16, victim: u16) -> Line {
        Line::Kill(output::Kill {
            tick,
            victim: PlayerId(victim),
            attacker: Some(PlayerId(attacker)),
            assister: None,
            weapon: "scattergun".to_string(),
            crit: false,
            headshot: false,
            backstab: false,
            suicide: false,
            dominated: false,
            revenge: false,
            feign: false,
            airborne: false,
            victim_pos: None,
            attacker_pos: None,
        })
    }

    #[test]
    fn the_feed_keeps_names_of_players_who_left() {
        let hub = SharedHub::default();
        publish(&hub, &snapshot(1, None, vec![player(3, "can of tuna", Team::Red), player(4, "silvo", Team::Blue)]));
        publish(&hub, &kill(2, 3, 4));
        let cap = output::PointCaptured { tick: 3, point: 0, name: "#cp".to_string(), team: Team::Red, cappers: vec![Some(PlayerId(3))] };
        publish(&hub, &Line::PointCaptured(cap));
        publish(&hub, &snapshot(4, None, Vec::new()));
        let lines: Vec<Value> = subscribe(&hub, false).try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
        let kill = lines.iter().find(|l| l["type"] == "kill").unwrap();
        assert_eq!(kill["names"]["3"]["name"], "can of tuna");
        assert_eq!(kill["names"]["4"]["team"], "blue");
        let cap = lines.iter().find(|l| l["type"] == "point_captured").unwrap();
        assert_eq!(cap["names"]["3"]["name"], "can of tuna");
    }

    #[test]
    fn history_entries_are_the_line_with_names_and_time_into_the_round() {
        let hub = SharedHub::default();
        publish(&hub, &snapshot(1000, Some(30.0), vec![player(3, "can of tuna", Team::Red)]));
        publish(&hub, &kill(1200, 3, 9));
        let past: Value = serde_json::from_str(&hub.lock().unwrap().history[0]).unwrap();
        let mut expected = serde_json::to_value(kill(1200, 3, 9)).unwrap();
        expected["history"] = true.into();
        expected["names"] = serde_json::json!({"3": {"name": "can of tuna", "team": "red"}});
        expected["at"] = 33.0.into();
        assert_eq!(past, expected);
    }

    #[test]
    fn a_page_opened_late_hears_how_old_the_data_is() {
        let hub = SharedHub::default();
        publish(&hub, &snapshot(1, None, Vec::new()));
        let last: Value = serde_json::from_str(&subscribe(&hub, false).try_iter().last().unwrap()).unwrap();
        assert_eq!(last["type"], "data_age");
        assert!(last["ms"].as_u64().is_some_and(|ms| ms < 1000), "{last}");
    }

    #[test]
    fn detail_lines_reach_only_detail_clients() {
        let hub = SharedHub::default();
        let (plain, detail) = (subscribe(&hub, false), subscribe(&hub, true));
        let heal =
            output::Heal { tick: 1, patient: PlayerId(4), healer: Some(PlayerId(3)), amount: 20, source: output::HealSource::Medigun };
        publish(&hub, &Line::Heal(heal));
        publish(&hub, &kill(2, 3, 4));
        let types = |rx: &Receiver<Arc<str>>| -> Vec<String> {
            rx.try_iter().map(|l| serde_json::from_str::<Value>(&l).unwrap()["type"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(types(&plain), ["kill"]);
        assert_eq!(types(&detail), ["heal", "kill"]);
        assert_eq!(hub.lock().unwrap().history.len(), 1);
        assert_eq!(types(&subscribe(&hub, true)), ["kill"], "detail is not replayed");
    }

    #[test]
    fn pass_time_goals_are_kept_and_ball_pickups_are_detail() {
        let hub = SharedHub::default();
        publish(&hub, &snapshot(1, None, vec![player(3, "can of tuna", Team::Red)]));
        let (plain, detail) = (subscribe(&hub, false), subscribe(&hub, true));
        // What a page joining gets first: the snapshot and its age.
        assert_eq!((plain.try_iter().count(), detail.try_iter().count()), (2, 2));
        publish(&hub, &Line::PassGet(output::PassGet { tick: 2, player: PlayerId(3) }));
        let goal = output::PassScore { tick: 3, scorer: PlayerId(3), assister: None, points: 1 };
        publish(&hub, &Line::PassScore(goal));
        let types = |rx: &Receiver<Arc<str>>| -> Vec<String> {
            rx.try_iter().map(|l| serde_json::from_str::<Value>(&l).unwrap()["type"].as_str().unwrap().to_string()).collect()
        };
        // High-volume lines the page draws nothing for only go to pages that ask for detail.
        let hit = output::NpcDamage {
            tick: 4,
            id: 700,
            attacker: Some(PlayerId(3)),
            amount: 20,
            health: 130,
            crit: false,
            weapon_id: 18,
            boss: 0,
        };
        publish(&hub, &Line::NpcDamage(hit));
        assert_eq!(types(&plain), ["pass_score"]);
        assert_eq!(types(&detail), ["pass_get", "pass_score", "npc_damage"]);
        let kept: Value = serde_json::from_str(hub.lock().unwrap().history.back().unwrap()).unwrap();
        assert_eq!((kept["type"].as_str(), &kept["names"]["3"]["name"]), (Some("pass_score"), &Value::from("can of tuna")));
    }
}
