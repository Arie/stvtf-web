//! Who is in a room: names, and who of them are admins. People are known by their guest cookie,
//! or, signed in with the site, by their Steam id; only the server sees either: pages see a
//! number per person instead.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// How a user signed in with the site is known: this and their Steam id.
pub(crate) const STEAM: &str = "steam:";

/// The longest name, in characters.
const MAX_NAME: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct People {
    owner: String,
    by_guest: HashMap<String, Person>,
    /// Admins by guest cookie, kept while they are away.
    admins: HashSet<String>,
    next: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Person {
    id: u32,
    name: String,
    /// Pen colour, as `#rrggbb`, once chosen.
    ink: Option<String>,
    /// Signed in with the site: the name is theirs there.
    #[serde(default)]
    signed: bool,
    /// Open pages: someone with none has left. Not saved: pages come back to a new process.
    #[serde(skip)]
    pages: usize,
}

/// A person as pages see them.
#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct Shown {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) admin: bool,
    pub(crate) owner: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ink: Option<String>,
    pub(crate) signed: bool,
    /// Signed in with the site: their Steam id, for their profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) steamid: Option<String>,
}

impl People {
    /// With `owner` as number 1.
    pub(crate) fn new(owner: &str) -> People {
        let mut people = People { owner: owner.to_string(), by_guest: HashMap::new(), admins: HashSet::from([owner.to_string()]), next: 1 };
        people.id(owner);
        people
    }

    fn person(&mut self, guest: &str) -> &mut Person {
        let next = &mut self.next;
        self.by_guest.entry(guest.to_string()).or_insert_with(|| {
            *next += 1;
            Person { id: *next - 1, name: String::new(), ink: None, signed: false, pages: 0 }
        })
    }

    /// The number pages know `guest` by.
    pub(crate) fn id(&mut self, guest: &str) -> u32 {
        self.person(guest).id
    }

    /// A page of `guest`'s joins, with the name it gives; one signed in (`site`) is called what the
    /// site calls them.
    pub(crate) fn join(&mut self, guest: &str, name: Option<&str>, site: Option<&str>) {
        let p = self.person(guest);
        p.pages += 1;
        if let Some(name) = site {
            (p.name, p.signed) = (clean(name), true);
        } else if let Some(name) = name.filter(|_| !p.signed) {
            p.name = clean(name);
        }
    }

    pub(crate) fn leave(&mut self, guest: &str) {
        let p = self.person(guest);
        p.pages = p.pages.saturating_sub(1);
    }

    /// A new name, unless the site names them.
    pub(crate) fn rename(&mut self, guest: &str, name: &str) {
        let p = self.person(guest);
        if !p.signed {
            p.name = clean(name);
        }
    }

    /// `guest` is an admin from now on: a streamer of the site's, say.
    pub(crate) fn make_admin(&mut self, guest: &str) {
        self.person(guest);
        self.admins.insert(guest.to_string());
    }

    /// An admin's pen colour, `#rrggbb`.
    pub(crate) fn set_ink(&mut self, guest: &str, ink: &str) -> Result<()> {
        if !self.is_admin(guest) {
            bail!("only admins draw");
        }
        if !(ink.len() == 7 && ink.starts_with('#') && ink[1..].bytes().all(|b| b.is_ascii_hexdigit())) {
            bail!("a colour is #rrggbb");
        }
        self.person(guest).ink = Some(ink.to_ascii_lowercase());
        Ok(())
    }

    pub(crate) fn is_admin(&self, guest: &str) -> bool {
        self.admins.contains(guest)
    }

    pub(crate) fn is_owner(&self, guest: &str) -> bool {
        self.owner == guest
    }

    /// Admin `by` makes person `id` an admin, or not; the owner stays one.
    pub(crate) fn set_admin(&mut self, by: &str, id: u32, admin: bool) -> Result<()> {
        if !self.is_admin(by) {
            bail!("only admins choose admins");
        }
        let Some(guest) = self.by_guest.iter().find(|(_, p)| p.id == id).map(|(g, _)| g.clone()) else { bail!("nobody is {id}") };
        if guest == self.owner {
            bail!("the owner stays an admin");
        }
        if admin {
            self.admins.insert(guest);
        } else {
            self.admins.remove(&guest);
        }
        Ok(())
    }

    /// Everyone here now, in the order they came.
    pub(crate) fn shown(&self) -> Vec<Shown> {
        let mut here: Vec<Shown> = self
            .by_guest
            .iter()
            .filter(|(_, p)| p.pages > 0)
            .map(|(g, p)| Shown {
                id: p.id,
                name: if p.name.is_empty() { format!("Guest {}", p.id) } else { p.name.clone() },
                admin: self.admins.contains(g),
                owner: *g == self.owner,
                ink: p.ink.clone(),
                signed: p.signed,
                steamid: g.strip_prefix(STEAM).filter(|_| p.signed).map(str::to_string),
            })
            .collect();
        here.sort_by_key(|p| p.id);
        here
    }
}

/// A name as shown: on one line, trimmed, at most [`MAX_NAME`] characters.
fn clean(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_are_shown_by_number_with_their_names() {
        let mut p = People::new("owner");
        p.join("owner", Some("  coach\n"), None);
        p.join("viewer", None, None);
        let shown = p.shown();
        assert_eq!(shown[0], Shown { id: 1, name: "coach".into(), admin: true, owner: true, ink: None, signed: false, steamid: None });
        assert_eq!(shown[1], Shown { id: 2, name: "Guest 2".into(), admin: false, owner: false, ink: None, signed: false, steamid: None });
        p.rename("viewer", &"x".repeat(40));
        assert_eq!(p.shown()[1].name.len(), MAX_NAME);
    }

    #[test]
    fn someone_with_no_pages_left_is_gone_and_keeps_their_number() {
        let mut p = People::new("owner");
        p.join("viewer", None, None);
        p.join("viewer", None, None);
        p.leave("viewer");
        assert_eq!(p.shown().len(), 1, "one page still open");
        p.leave("viewer");
        assert_eq!(p.shown(), []);
        p.join("viewer", None, None);
        assert_eq!(p.shown()[0].id, 2, "the owner is 1");
    }

    #[test]
    fn the_site_names_who_signed_in_and_streamers_make_themselves_admin() {
        let mut p = People::new("owner");
        p.join("steam:7656", Some("fake"), Some("Arie"));
        p.rename("steam:7656", "someone else");
        p.join("steam:7656", Some("again"), None);
        let shown = p.shown();
        assert_eq!((shown[0].name.as_str(), shown[0].signed, shown[0].admin), ("Arie", true, false));
        assert_eq!(shown[0].steamid.as_deref(), Some("7656"));
        p.make_admin("steam:7656");
        assert!(p.is_admin("steam:7656"));
    }

    #[test]
    fn admins_pick_their_pen_colour() {
        let mut p = People::new("owner");
        p.join("owner", None, None);
        p.set_ink("owner", "#00FF7f").unwrap();
        assert_eq!(p.shown()[0].ink.as_deref(), Some("#00ff7f"));
        assert!(p.set_ink("owner", "red").is_err());
        assert!(p.set_ink("viewer", "#000000").is_err(), "not an admin");
    }

    #[test]
    fn admins_choose_admins_and_the_owner_stays_one() {
        let mut p = People::new("owner");
        let (owner, a, b) = (p.id("owner"), p.id("a"), p.id("b"));
        assert!(p.set_admin("a", b, true).is_err(), "a is no admin yet");
        p.set_admin("owner", a, true).unwrap();
        p.set_admin("a", b, true).unwrap();
        assert!(p.is_admin("b"));
        p.set_admin("b", a, false).unwrap();
        assert!(!p.is_admin("a"));
        assert!(p.set_admin("b", owner, false).is_err());
        assert!(p.set_admin("b", 99, true).is_err());
    }
}
