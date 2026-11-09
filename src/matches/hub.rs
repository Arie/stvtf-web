//! The latest state of a match and the queues of the pages watching it.

use crate::lock;
#[cfg(test)]
use crate::testing::watch;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stvtf::Line;
use stvtf::output::{Class, PlayerId, Round, Score, Status, Team, round1};
use stvtf::watch::Watch;
#[cfg(not(test))]
use stvtf::watch::watch;

/// The latest match state for browsers that join late, and a queue per browser.
#[derive(Default)]
pub(crate) struct Hub {
    status: Option<Arc<str>>,
    server: Option<Arc<str>>,
    world: Option<Arc<str>>,
    snapshot: Option<Arc<str>>,
    clients: Vec<Client>,
    pub(crate) map: Option<String>,
    /// Feed events of the current map, so a page opened late still shows kills and chat.
    pub(crate) history: VecDeque<Arc<str>>,
    /// Counts changes to `map` and `history`, so a match is saved only when they changed.
    pub(crate) changes: u64,
    /// Tick and round time of the latest snapshot, to stamp events with time into the round.
    round_clock: Option<(u32, f64)>,
    /// The latest snapshot's tick.
    tick: Option<u32>,
    /// When the latest snapshot came in, so a page opened after the stream stopped can say so.
    snapshot_at: Option<Instant>,
    /// Name and team of everyone seen on this map: the feed keeps them for players who left.
    names: HashMap<PlayerId, Seen>,
    /// Seconds per tick, from the server line.
    tick_interval: Option<f64>,
    /// Play time of a replayed recording, in milliseconds: where its shared playback is.
    pub(crate) played: u64,
    /// A room's latest lines of its own (playback, people), by type.
    room: BTreeMap<&'static str, Arc<str>>,
}

/// A match at a glance, read from the latest server line and snapshot when the list asks.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Summary {
    pub(crate) hostname: Option<String>,
    #[serde(flatten)]
    pub(crate) snapshot: Option<SnapshotSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SnapshotSummary {
    pub(crate) round: Round,
    pub(crate) round_time: Option<f64>,
    pub(crate) time_left: Option<f64>,
    pub(crate) match_time: Option<f64>,
    pub(crate) win_limit: Option<i64>,
    pub(crate) score: Score,
    pub(crate) players: Vec<Rostered>,
}

#[derive(Deserialize)]
struct ServerSummary {
    hostname: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Rostered {
    pub(crate) name: String,
    pub(crate) steamid: String,
    pub(crate) team: Team,
    pub(crate) class: Class,
}

impl Hub {
    /// Player `id`'s name and team, as last seen on this map.
    pub(crate) fn seen(&self, id: PlayerId) -> Option<(&str, Team)> {
        self.names.get(&id).map(|s| (s.name.as_str(), s.team))
    }

    /// Seconds a server tick.
    pub(crate) fn tick_interval(&self) -> f64 {
        self.tick_interval.unwrap_or_else(default_tick_interval)
    }

    /// The latest snapshot's tick: where a room that played up to here is.
    pub(crate) fn tick(&self) -> Option<u32> {
        self.tick
    }

    /// How long ago the latest snapshot came in.
    pub(crate) fn data_age(&self) -> Option<Duration> {
        self.snapshot_at.map(|t| t.elapsed())
    }

    /// The state a page joins with, without the pages.
    pub(crate) fn checkpoint(&self) -> Hub {
        Hub {
            status: self.status.clone(),
            server: self.server.clone(),
            world: self.world.clone(),
            snapshot: self.snapshot.clone(),
            clients: Vec::new(),
            map: self.map.clone(),
            history: self.history.clone(),
            changes: self.changes,
            round_clock: self.round_clock,
            tick: self.tick,
            snapshot_at: self.snapshot_at,
            names: self.names.clone(),
            tick_interval: self.tick_interval,
            room: self.room.clone(),
            played: self.played,
        }
    }

    /// The latest status, server, world and snapshot lines: the match's state, without its feed.
    pub(crate) fn state(&self) -> impl Iterator<Item = &Arc<str>> {
        [&self.status, &self.server, &self.world, &self.snapshot].into_iter().flatten()
    }

    pub(crate) fn summary(&self) -> Summary {
        Summary { hostname: parse(self.server.as_ref()).map(|s: ServerSummary| s.hostname), snapshot: parse(self.snapshot.as_ref()) }
    }
}

/// The fields of a kept line that `T` asks for.
fn parse<T: serde::de::DeserializeOwned>(line: Option<&Arc<str>>) -> Option<T> {
    serde_json::from_str(line?).ok()
}

/// Seconds per tick of a server that has not said yet.
pub(crate) fn default_tick_interval() -> f64 {
    f64::from(stvtf::DEFAULT_TICK_INTERVAL)
}

/// A page's queue of lines.
struct Client {
    tx: SyncSender<Arc<str>>,
    /// Whether the page asked for detail lines.
    detail: bool,
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

#[allow(clippy::too_many_lines, reason = "a table: every kind of line, and its route")]
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

impl Hub {
    /// A status line: a map change forgets the old map's state and feed.
    fn on_status(&mut self, map_change: bool, json: &Arc<str>) {
        if map_change {
            self.snapshot = None;
            self.history.clear();
            self.names.clear();
            self.changes += 1;
        }
        self.status = Some(json.clone());
    }

    /// The server's line: signon restarts announce the same map again; only a new map starts a
    /// new history.
    fn on_server(&mut self, server: &stvtf::output::Server, json: &Arc<str>) {
        if self.map.as_deref() != Some(server.map.as_str()) {
            self.history.clear();
            self.names.clear();
            self.map = Some(server.map.clone());
            self.changes += 1;
        }
        (self.server, self.snapshot, self.round_clock, self.tick) = (Some(json.clone()), None, None, None);
        self.tick_interval = Some(server.tick_interval).filter(|i| *i > 0.0);
    }

    /// A snapshot: the latest state, and the names of everyone in it.
    fn on_snapshot(&mut self, snapshot: &stvtf::output::Snapshot, json: &Arc<str>) {
        self.round_clock = snapshot.round_time.map(|t| (snapshot.tick, t));
        self.tick = Some(snapshot.tick);
        self.snapshot = Some(json.clone());
        self.snapshot_at = Some(Instant::now());
        for p in &snapshot.players {
            // Every snapshot names everyone: reuse the string rather than allocate a new one.
            let seen = self.names.entry(p.id).or_insert_with(|| Seen { name: String::new(), team: p.team });
            seen.name.clone_from(&p.name);
            seen.team = p.team;
        }
    }

    /// A feed line, kept in the history with the names it uses and its time into the round.
    fn keep(&mut self, line: &Line) {
        let interval = self.tick_interval.unwrap_or_else(default_tick_interval);
        let at = self.round_clock.zip(line.tick()).map(|((tick, time), at)| {
            let seconds = time + (f64::from(at) - f64::from(tick)) * interval;
            round1(seconds).max(0.0)
        });
        let names = line.players().filter_map(|id| Some((id, self.names.get(&id)?))).collect();
        let past = serde_json::to_string(&Past { line, history: true, names, at }).expect("a line serializes");
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(past.into());
        self.changes += 1;
    }
}

pub(crate) type SharedHub = Arc<Mutex<Hub>>;

pub(crate) fn publish(hub: &SharedHub, line: &Line) {
    let json: Arc<str> = serde_json::to_string(line).expect("a line serializes").into();
    let route = route(line);
    let mut h = lock(hub);
    match line {
        Line::Status(status) => h.on_status(*status == Status::MapChange, &json),
        Line::Server(server) => h.on_server(server, &json),
        Line::World(_) => h.world = Some(json.clone()),
        Line::Snapshot(snapshot) => h.on_snapshot(snapshot, &json),
        _ if route == Route::Feed => h.keep(line),
        _ => {}
    }
    // A browser that cannot keep up is dropped; it reconnects and resyncs from the latest state.
    let detail = route == Route::Detail;
    h.clients.retain(|c| {
        (detail && !c.detail) || !matches!(c.tx.try_send(json.clone()), Err(TrySendError::Full(_) | TrySendError::Disconnected(_)))
    });
}

/// Watches a `SourceTV` server into `hub` until `stop`, recording it as `demos` if given; a watch
/// that gives up says so to the pages.
pub(crate) fn watch_into(
    hub: &SharedHub,
    target: &str,
    password: &str,
    hz: f32,
    demos: Option<Arc<dyn stvtf::demo::Files>>,
    stop: &AtomicBool,
) -> Result<(), stvtf::watch::WatchError> {
    let mut w = Watch::new(target, password);
    w.snapshot_hz = hz;
    w.demos = demos;
    let result = watch(&w, stop, &mut |line| publish(hub, &line));
    if let Err(e) = &result {
        publish(hub, &Line::Status(Status::GaveUp { reason: e.to_string() }));
    }
    result
}

pub(crate) fn subscribe(hub: &SharedHub, detail: bool) -> Receiver<Arc<str>> {
    let (tx, rx) = sync_channel(PAGE_QUEUE);
    let mut h = lock(hub);
    for line in join_lines(&h) {
        let _ = tx.try_send(line);
    }
    h.clients.push(Client { tx, detail });
    rx
}

/// What a page joins with.
fn join_lines(h: &Hub) -> Vec<Arc<str>> {
    // The snapshot goes before the history so the page knows the players' names.
    let mut lines: Vec<Arc<str>> = [&h.status, &h.server, &h.world, &h.snapshot].into_iter().flatten().chain(&h.history).cloned().collect();
    if let Some(age) = h.data_age() {
        let age = DataAge { ms: age.as_millis() as u64 };
        lines.push(serde_json::to_string(&age).expect("a number serializes").into());
    }
    lines.extend(h.room.values().cloned());
    lines
}

/// Puts `from` (a [`Hub::checkpoint`] played on) in place, keeping the pages and the room's lines, and
/// starts the pages over from it.
pub(crate) fn restore(hub: &SharedHub, mut from: Hub) {
    let mut h = lock(hub);
    from.clients = std::mem::take(&mut h.clients);
    from.room = std::mem::take(&mut h.room);
    from.snapshot_at = Some(Instant::now());
    *h = from;
    let mut lines = join_lines(&h);
    lines.insert(0, r#"{"type":"seek"}"#.into());
    h.clients.retain(|c| lines.iter().all(|l| c.tx.try_send(l.clone()).is_ok()));
}

/// A line to every page now, not kept.
pub(crate) fn send(hub: &SharedHub, json: &Arc<str>) {
    lock(hub).clients.retain(|c| c.tx.try_send(json.clone()).is_ok());
}

/// A line to every page now; joiners get `kept` as the room's `kind` line instead.
pub(crate) fn send_keeping(hub: &SharedHub, json: &Arc<str>, kind: &'static str, kept: Arc<str>) {
    let mut h = lock(hub);
    h.room.insert(kind, kept);
    h.clients.retain(|c| c.tx.try_send(json.clone()).is_ok());
}

/// A room's line of type `kind`, to every page now and (the latest of each kind) on join.
pub(crate) fn announce(hub: &SharedHub, kind: &'static str, json: &Arc<str>) {
    let mut h = lock(hub);
    h.room.insert(kind, json.clone());
    h.clients.retain(|c| c.tx.try_send(json.clone()).is_ok());
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::{script, server};
    use serde_json::Value;
    use stvtf::output::{self, PlayerId, Team};

    /// The types of the lines queued for a page.
    fn types(rx: &Receiver<Arc<str>>) -> Vec<String> {
        rx.try_iter().map(|l| serde_json::from_str::<Value>(&l).unwrap()["type"].as_str().unwrap().to_string()).collect()
    }

    pub(crate) fn player(id: u16, name: &str, team: Team) -> output::Player {
        output::Player {
            id: PlayerId(id),
            ent: u32::from(id),
            name: name.to_string(),
            steamid: "BOT".to_string(),
            team,
            class: Class::Scout,
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

    pub(crate) fn snapshot(tick: u32, round_time: Option<f64>, players: Vec<output::Player>) -> Line {
        Line::Snapshot(output::Snapshot {
            tick,
            players,
            projectiles: Vec::new(),
            buildings: Vec::new(),
            objectives: Vec::new(),
            round: Round::Running,
            round_time,
            time_left: None,
            win_limit: None,
            match_time: None,
            score: Score { red: 0, blue: 0 },
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
    fn the_summary_follows_the_latest_snapshot_and_map() {
        let hub = SharedHub::default();
        publish(&hub, &server("cp_process_f12"));
        publish(&hub, &snapshot(1, Some(12.0), vec![player(3, "can of tuna", Team::Red), player(4, "silvo", Team::Blue)]));
        let summary = hub.lock().unwrap().summary();
        assert_eq!(summary.hostname.as_deref(), Some("SourceTV"));
        let snapshot = summary.snapshot.unwrap();
        assert_eq!((snapshot.round, snapshot.round_time, snapshot.players.len()), (Round::Running, Some(12.0), 2));
        assert_eq!(snapshot.players[0].name, "can of tuna");
        assert!(hub.lock().unwrap().data_age().is_some());
        publish(&hub, &server("koth_product_final"));
        assert!(hub.lock().unwrap().summary().snapshot.is_none(), "a new map starts empty");
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
    fn a_restored_hub_reprimes_its_pages() {
        let hub = SharedHub::default();
        publish(&hub, &server("cp_process_f12"));
        publish(&hub, &kill(2, 3, 4));
        let saved = hub.lock().unwrap().checkpoint();
        let page = subscribe(&hub, false);
        publish(&hub, &kill(3, 4, 3));
        page.try_iter().count();
        restore(&hub, saved);
        assert_eq!(types(&page), ["seek", "server", "kill", "data_age"]);
        assert_eq!(hub.lock().unwrap().history.len(), 1);
    }

    #[test]
    fn pages_hear_the_playback_on_join_and_on_change() {
        let hub = SharedHub::default();
        let early = subscribe(&hub, false);
        announce(&hub, "playback", &r#"{"type":"playback"}"#.into());
        announce(&hub, "people", &r#"{"type":"people"}"#.into());
        announce(&hub, "playback", &r#"{"type":"playback"}"#.into());
        assert_eq!(types(&early), ["playback", "people", "playback"]);
        assert_eq!(types(&subscribe(&hub, false)), ["people", "playback"], "the latest of each");
    }

    #[test]
    fn detail_lines_reach_only_detail_clients() {
        let hub = SharedHub::default();
        let (plain, detail) = (subscribe(&hub, false), subscribe(&hub, true));
        let heal =
            output::Heal { tick: 1, patient: PlayerId(4), healer: Some(PlayerId(3)), amount: 20, source: output::HealSource::Medigun };
        publish(&hub, &Line::Heal(heal));
        publish(&hub, &kill(2, 3, 4));
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

    #[test]
    fn pages_hear_a_watch_and_why_it_gave_up() {
        let hub = SharedHub::default();
        let page = subscribe(&hub, false);
        script("hub-gives-up", vec![server("cp_process_f12")], Some("wrong password"));
        let result = watch_into(&hub, "hub-gives-up", "tv", 20.0, None, &AtomicBool::new(false));
        assert_eq!(result.unwrap_err().to_string(), "wrong password");
        let lines: Vec<Value> = page.try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["map"], "cp_process_f12");
        assert_eq!(lines[1]["type"], "status");
        assert!(lines[1].to_string().contains("wrong password"), "{}", lines[1]);
    }

    #[test]
    fn a_watch_that_was_stopped_tells_the_pages_nothing() {
        let hub = SharedHub::default();
        let page = subscribe(&hub, false);
        assert!(watch_into(&hub, "hub-stopped", "tv", 20.0, None, &AtomicBool::new(true)).is_ok());
        assert_eq!(types(&page), Vec::<String>::new());
    }

    #[test]
    fn a_map_change_forgets_the_match_so_far() {
        let hub = SharedHub::default();
        publish(&hub, &snapshot(1, None, vec![player(3, "can of tuna", Team::Red)]));
        publish(&hub, &kill(2, 3, 3));
        let before = hub.lock().unwrap().changes;
        publish(&hub, &Line::Status(Status::MapChange));
        assert_eq!(types(&subscribe(&hub, false)), ["status", "data_age"], "no snapshot or kill from the old map");
        assert!(hub.lock().unwrap().changes > before, "the emptied feed is saved");
        publish(&hub, &kill(3, 3, 3));
        let page = subscribe(&hub, false);
        let kill = page.try_iter().map(|l| serde_json::from_str::<Value>(&l).unwrap()).find(|l| l["type"] == "kill").unwrap();
        assert!(kill.get("names").is_none(), "names from the old map are gone: {kill}");
    }

    #[test]
    fn only_a_new_map_starts_a_new_history() {
        let hub = SharedHub::default();
        publish(&hub, &server("cp_process_f12"));
        publish(&hub, &kill(1, 3, 4));
        let saved = hub.lock().unwrap().changes;
        publish(&hub, &server("cp_process_f12"));
        assert_eq!(hub.lock().unwrap().history.len(), 1, "a signon restart keeps it");
        assert_eq!(hub.lock().unwrap().changes, saved, "and has nothing new to save");
        publish(&hub, &server("cp_badlands"));
        assert!(hub.lock().unwrap().history.is_empty());
        assert_eq!(hub.lock().unwrap().map.as_deref(), Some("cp_badlands"));
    }

    #[test]
    fn the_history_keeps_only_the_latest_lines() {
        let hub = SharedHub::default();
        for tick in 1..=HISTORY as u32 + 1 {
            publish(&hub, &kill(tick, 3, 4));
        }
        let h = hub.lock().unwrap();
        let tick = |l: &Arc<str>| serde_json::from_str::<Value>(l).unwrap()["tick"].as_u64().unwrap();
        assert_eq!(h.history.len(), HISTORY);
        assert_eq!(tick(h.history.front().unwrap()), 2, "the oldest went");
        assert_eq!(tick(h.history.back().unwrap()), HISTORY as u64 + 1);
    }

    #[test]
    fn a_page_that_falls_behind_is_dropped_and_others_keep_going() {
        let hub = SharedHub::default();
        let (slow, fast) = (subscribe(&hub, false), subscribe(&hub, false));
        for tick in 1..=PAGE_QUEUE as u32 + 1 {
            publish(&hub, &snapshot(tick, None, Vec::new()));
            fast.try_iter().for_each(drop);
        }
        assert_eq!(hub.lock().unwrap().clients.len(), 1);
        assert_eq!(slow.try_iter().count(), PAGE_QUEUE, "what it had queued it can still read");
        assert!(slow.try_recv().is_err_and(|e| e == std::sync::mpsc::TryRecvError::Disconnected), "and then it reconnects");
        publish(&hub, &snapshot(PAGE_QUEUE as u32 + 2, None, Vec::new()));
        assert_eq!(fast.try_iter().count(), 1);
    }

    #[test]
    fn a_page_that_left_is_forgotten() {
        let hub = SharedHub::default();
        drop(subscribe(&hub, false));
        publish(&hub, &snapshot(1, None, Vec::new()));
        assert!(hub.lock().unwrap().clients.is_empty());
    }

    /// `testdata/lines.jsonl` is what `app/` is tested with too: a line stvtf renames or gives
    /// another shape fails here, and what the page does not read from it fails there.
    #[test]
    fn the_sample_lines_are_what_stvtf_sends() {
        let samples = include_str!("../../testdata/lines.jsonl");
        for sample in samples.lines() {
            let line: Line = serde_json::from_str(sample).unwrap_or_else(|e| panic!("{sample}: {e}"));
            let written: Value = serde_json::to_value(&line).unwrap();
            assert_eq!(written, serde_json::from_str::<Value>(sample).unwrap());
            let page_only = matches!(line, Line::VoiceCommand(_) | Line::ServerText(_));
            assert_eq!(route(&line), if page_only { Route::Detail } else { Route::Feed }, "{sample}");
        }
    }
}
