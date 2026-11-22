//! The names a room's admins give the match's players and teams (a league's, say), in place of
//! the game's: told to every page, and to its relay for those watching in TF2.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::Room;
use crate::matches::hub::announce;
use crate::sync::lock;
use crate::voice::ids::account_of;

/// The longest name, and the most players named.
const MAX_NAME: usize = 32;
const MAX_PLAYERS: usize = 64;

/// Players' names by `SteamID`, and teams' by side (`red`, `blue`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Names {
    #[serde(default)]
    pub(crate) players: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) teams: BTreeMap<String, String>,
}

impl Names {
    pub(super) fn is_empty(&self) -> bool {
        self.players.is_empty() && self.teams.is_empty()
    }

    /// As a relay shows them in TF2.
    pub(super) fn for_tf2(&self) -> stvtf::relay::Names {
        stvtf::relay::Names { players: self.players.clone(), teams: self.teams.clone() }
    }

    fn line(&self) -> Arc<str> {
        serde_json::json!({ "type": "names", "players": self.players, "teams": self.teams }).to_string().into()
    }
}

/// `name` as it is shown: no control characters, no spaces around it.
fn shown(name: &str) -> Result<String> {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let name = name.trim();
    if name.chars().count() > MAX_NAME {
        bail!("At most {MAX_NAME} characters.");
    }
    Ok(name.to_string())
}

impl Room {
    /// An admin names players and teams, in place of the names from before: one with no name
    /// given keeps the game's.
    pub(crate) fn set_names(&self, names: &Names) -> Result<()> {
        if names.players.len() > MAX_PLAYERS {
            bail!("At most {MAX_PLAYERS} players.");
        }
        let mut kept = Names::default();
        for (steamid, name) in &names.players {
            if account_of(steamid).is_none() {
                bail!("Invalid player.");
            }
            let name = shown(name)?;
            if !name.is_empty() {
                kept.players.insert(steamid.clone(), name);
            }
        }
        for (side, name) in &names.teams {
            if side != "red" && side != "blue" {
                bail!("Invalid team.");
            }
            let name = shown(name)?;
            if !name.is_empty() {
                kept.teams.insert(side.clone(), name);
            }
        }
        self.keep_names(kept);
        Ok(())
    }

    /// The room's names are `names`: every page and the relay are told.
    pub(super) fn keep_names(&self, names: Names) {
        announce(&self.hub, "names", &names.line());
        if let Some(r) = &*lock(&self.relay.current) {
            r.keep_names(names.for_tf2());
        }
        *lock(&self.names) = names;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matches::hub::subscribe;
    use crate::rooms::room::tests::arc_room;

    fn names(players: &[(&str, &str)], teams: &[(&str, &str)]) -> Names {
        let map = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
        Names { players: map(players), teams: map(teams) }
    }

    #[test]
    fn names_given_are_told_to_every_page_and_those_who_come_later_and_saved() {
        let (r, _dir) = arc_room();
        let page = subscribe(&r.hub, false);
        let _joined: Vec<_> = page.try_iter().collect();
        r.set_names(&names(&[("[U:1:5]", "  Five\u{7} "), ("[U:1:6]", " ")], &[("red", "Reds"), ("blue", "")])).unwrap();
        let line = page.try_iter().find(|l| l.contains(r#""type":"names""#)).unwrap();
        let line: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(line, serde_json::json!({ "type": "names", "players": { "[U:1:5]": "Five" }, "teams": { "red": "Reds" } }));
        let later: Vec<_> = subscribe(&r.hub, false).try_iter().collect();
        assert!(later.iter().any(|l| l.contains(r#""type":"names""#) && l.contains("Five")));
        assert_eq!(r.saved().names, names(&[("[U:1:5]", "Five")], &[("red", "Reds")]));
        // The next names are in place of these.
        r.set_names(&Names::default()).unwrap();
        assert!(r.saved().names.is_empty());
    }

    #[test]
    fn names_that_will_not_do_are_refused_and_the_ones_there_stay() {
        let (r, _dir) = arc_room();
        r.set_names(&names(&[("[U:1:5]", "Five")], &[])).unwrap();
        for (bad, why) in [
            (names(&[("someone", "x")], &[]), "Invalid player."),
            (names(&[], &[("green", "x")]), "Invalid team."),
            (names(&[("[U:1:5]", &"x".repeat(33))], &[]), "At most 32 characters."),
        ] {
            assert_eq!(r.set_names(&bad).unwrap_err().to_string(), why);
        }
        assert_eq!(r.saved().names.players["[U:1:5]"], "Five");
    }
}
