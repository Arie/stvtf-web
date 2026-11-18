//! How the leagues know a match's players, for a room's names: each site stv.tf belongs to
//! (`STVTF_LINK_SITES`, else its `--home`) is asked what its league calls them and which teams
//! they are on (`GET <site>/league_players?steam_uids=`, with a token sealed with the key they
//! share), and each side's team is guessed from its players. What a site said is kept a while:
//! the sites keep it a day themselves.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::sync::lock;
use crate::voice::ids::{STEAM64_BASE, account_of};
use crate::web::token::Keys;

/// How long what a site said of a player stands: known there, and not.
const KNOWN_FOR: Duration = Duration::from_hours(1);
const UNKNOWN_FOR: Duration = Duration::from_mins(10);
const TIMEOUT: Duration = Duration::from_secs(20);
/// The most players asked about at once, and kept.
pub(crate) const MAX_PLAYERS: usize = 40;
const MAX_KEPT: usize = 10_000;
/// A side of this many is a Highlander team.
const HIGHLANDER: usize = 8;

/// A team a player is on in a league, and the format it plays (as the league names it).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct Team {
    name: String,
    #[serde(default)]
    format: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct Player {
    steam_uid: String,
    name: String,
    #[serde(default)]
    teams: Vec<Team>,
}

/// What a site says: its league, and those of the players asked about it knows.
#[derive(Debug, Deserialize)]
pub(crate) struct Said {
    league: Option<String>,
    #[serde(default)]
    players: Vec<Player>,
}

type Ask = Box<dyn Fn(&[u64]) -> Result<Said> + Send + Sync>;

pub(crate) struct Site {
    ask: Ask,
    league: Mutex<Option<String>>,
    /// By account: when it was asked, and what it said.
    kept: Mutex<HashMap<u32, (Instant, Option<Player>)>>,
}

/// A player of the match, as a page says: their `SteamID` and side.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Asked {
    pub(crate) steamid: String,
    pub(crate) team: String,
}

/// A league's names for a match: its players' by `SteamID`, and each side's team if most of the
/// side is on one.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct League {
    #[serde(rename = "league")]
    pub(crate) name: String,
    pub(crate) players: BTreeMap<String, String>,
    pub(crate) teams: BTreeMap<String, String>,
}

#[derive(Default)]
pub(crate) struct Leagues(Vec<Site>);

/// The site at `home` (as a stv.tf's `--home`), asked with tokens sealed with `keys`.
pub(crate) fn site(home: &str, keys: Arc<Keys>) -> Site {
    let url = format!("{}/league_players", home.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build();
    Site::asking(Box::new(move |steam_uids| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let bearer = format!("Bearer {}", keys.league_token(now));
        let ids = steam_uids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let answer = agent.get(&url).query("steam_uids", &ids).set("Authorization", &bearer).call();
        Ok(serde_json::from_str(&answer.context("cannot ask a site for league names")?.into_string()?)?)
    }))
}

impl Site {
    fn asking(ask: Ask) -> Site {
        Site { ask, league: Mutex::default(), kept: Mutex::default() }
    }

    /// What it says of `accounts`, asking about those it was not asked about lately.
    fn players(&self, accounts: &[u32]) -> Vec<(u32, Player)> {
        let now = Instant::now();
        let fresh = |(at, said): &(Instant, Option<Player>)| now.duration_since(*at) < if said.is_some() { KNOWN_FOR } else { UNKNOWN_FOR };
        let wanted: Vec<u32> = {
            let kept = lock(&self.kept);
            accounts.iter().copied().filter(|a| !kept.get(a).is_some_and(fresh)).collect()
        };
        if !wanted.is_empty() {
            let ids: Vec<u64> = wanted.iter().map(|a| STEAM64_BASE + u64::from(*a)).collect();
            match (self.ask)(&ids) {
                Ok(said) => {
                    *lock(&self.league) = said.league;
                    let mut kept = lock(&self.kept);
                    if kept.len() > MAX_KEPT {
                        kept.clear();
                    }
                    for a in &wanted {
                        let known = said.players.iter().find(|p| p.steam_uid.parse() == Ok(STEAM64_BASE + u64::from(*a)));
                        kept.insert(*a, (now, known.cloned()));
                    }
                }
                Err(e) => tracing::info!("{e:#}"),
            }
        }
        let kept = lock(&self.kept);
        accounts.iter().filter_map(|a| Some((*a, kept.get(a)?.1.clone()?))).collect()
    }
}

impl Leagues {
    pub(crate) fn new(sites: Vec<Site>) -> Self {
        Leagues(sites)
    }

    /// The leagues that know any of `asked`, the one knowing most first.
    pub(crate) fn look_up(&self, asked: &[Asked]) -> Vec<League> {
        let players: Vec<(u32, &Asked)> = asked.iter().filter_map(|p| Some((account_of(&p.steamid)?, p))).take(MAX_PLAYERS).collect();
        let accounts: Vec<u32> = players.iter().map(|(a, _)| *a).collect();
        let mut leagues: Vec<League> = std::thread::scope(|s| {
            let asking: Vec<_> = self
                .0
                .iter()
                .map(|site| {
                    let accounts = &accounts;
                    s.spawn(move || (site, site.players(accounts)))
                })
                .collect();
            asking
                .into_iter()
                .filter_map(|a| a.join().ok())
                .filter_map(|(site, known)| {
                    let league = lock(&site.league).clone().filter(|_| !known.is_empty())?;
                    let asked_of = |a: u32| players.iter().find(|(account, _)| *account == a).map(|(_, p)| *p);
                    let names = known.iter().filter_map(|(a, p)| Some((asked_of(*a)?.steamid.clone(), p.name.clone()))).collect();
                    let teams = ["red", "blue"]
                        .into_iter()
                        .filter_map(|side| {
                            let on_side: Vec<&Player> =
                                known.iter().filter(|(a, _)| asked_of(*a).is_some_and(|p| p.team == side)).map(|(_, p)| p).collect();
                            let size = players.iter().filter(|(_, p)| p.team == side).count();
                            Some((side.to_string(), guess_team(&on_side, size)?))
                        })
                        .collect();
                    Some(League { name: league, players: names, teams })
                })
                .collect()
        });
        leagues.sort_by(|a, b| b.players.len().cmp(&a.players.len()).then_with(|| a.name.cmp(&b.name)));
        leagues
    }
}

/// The team most of a side's `players` are on, of a side of `size`: at least two of them, and of
/// the format the side's size is, when two teams have as many.
fn guess_team(players: &[&Player], size: usize) -> Option<String> {
    let highlander = size >= HIGHLANDER;
    let fits = |t: &Team| t.format.as_deref().is_some_and(|f| f.to_ascii_lowercase().contains("highlander") == highlander);
    let mut on: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
    for t in players.iter().flat_map(|p| &p.teams) {
        let team = on.entry(&t.name).or_default();
        *team = (team.0 + 1, team.1 || fits(t));
    }
    let (name, (count, _)) = on.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))?;
    (count >= 2.min(size.max(1))).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(account: u32, name: &str, teams: &[(&str, &str)]) -> Player {
        let teams = teams.iter().map(|(name, format)| Team { name: (*name).to_string(), format: Some((*format).to_string()) }).collect();
        Player { steam_uid: (STEAM64_BASE + u64::from(account)).to_string(), name: name.into(), teams }
    }

    fn asked(account: u32, team: &str) -> Asked {
        Asked { steamid: format!("[U:1:{account}]"), team: team.into() }
    }

    /// A site of `league` knowing `players`, counting how many it was asked about.
    fn knowing(league: &str, players: Vec<Player>, asked: &Arc<Mutex<Vec<usize>>>) -> Site {
        let (league, asked) = (league.to_string(), asked.clone());
        Site::asking(Box::new(move |ids| {
            lock(&asked).push(ids.len());
            let players = players.iter().filter(|p| ids.contains(&p.steam_uid.parse().unwrap())).cloned().collect();
            Ok(Said { league: Some(league.clone()), players })
        }))
    }

    #[test]
    fn a_leagues_names_and_each_sides_team_come_from_the_site_that_knows_the_players() {
        let count = Arc::default();
        let known = vec![
            player(1, "One", &[("Reds", "6v6"), ("Nine", "Highlander")]),
            player(2, "Two", &[("Reds", "6v6"), ("Nine", "Highlander")]),
            player(3, "Three", &[("Blues", "6v6")]),
        ];
        let leagues = Leagues::new(vec![knowing("ETF2L", known, &count), knowing("RGL", vec![player(3, "Tres", &[])], &count)]);
        let match_of =
            [asked(1, "red"), asked(2, "red"), asked(3, "blue"), asked(4, "blue"), Asked { steamid: "BOT".into(), team: "red".into() }];
        let found = leagues.look_up(&match_of);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "ETF2L", "the one knowing most first");
        assert_eq!(found[0].players["[U:1:1]"], "One");
        assert_eq!(found[0].players.len(), 3);
        // Both on two teams: the one of the format a side of two is. One player is no team.
        assert_eq!(found[0].teams, BTreeMap::from([("red".to_string(), "Reds".to_string())]));
        assert_eq!((found[1].name.as_str(), found[1].players["[U:1:3]"].as_str()), ("RGL", "Tres"));
        assert!(found[1].teams.is_empty());
    }

    #[test]
    fn a_side_of_nine_is_its_highlander_team() {
        let both = [("Reds", "6v6"), ("Nine", "Highlander")];
        let players: Vec<Player> = (1..=9).map(|n| player(n, "p", &both)).collect();
        let refs: Vec<&Player> = players.iter().collect();
        assert_eq!(guess_team(&refs, 9).as_deref(), Some("Nine"));
        assert_eq!(guess_team(&refs[..6], 6).as_deref(), Some("Reds"));
        assert_eq!(guess_team(&[], 6), None);
    }

    #[test]
    fn a_site_is_asked_about_a_player_once_in_a_while_and_one_not_answering_knows_nobody() {
        let count: Arc<Mutex<Vec<usize>>> = Arc::default();
        let leagues = Leagues::new(vec![knowing("ETF2L", vec![player(1, "One", &[])], &count)]);
        leagues.look_up(&[asked(1, "red"), asked(2, "blue")]);
        leagues.look_up(&[asked(1, "red"), asked(2, "blue"), asked(3, "blue")]);
        assert_eq!(*lock(&count), [2, 1], "only who is new");
        let down = Leagues::new(vec![Site::asking(Box::new(|_| anyhow::bail!("down")))]);
        assert_eq!(down.look_up(&[asked(1, "red")]), []);
        assert_eq!(Leagues::default().look_up(&[asked(1, "red")]), []);
    }
}
