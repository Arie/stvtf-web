//! Who is in a room: names, and who of them are admins. People are known by their guest cookie,
//! which only the server sees; pages see a number per person instead.

use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// The longest name, in characters.
const MAX_NAME: usize = 32;

pub(crate) struct People {
    owner: String,
    by_guest: HashMap<String, Person>,
    /// Admins by guest cookie, kept while they are away.
    admins: HashSet<String>,
    next: u32,
}

struct Person {
    id: u32,
    name: String,
    /// Open pages: someone with none has left.
    pages: usize,
}

/// A person as pages see them.
#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct Shown {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) admin: bool,
    pub(crate) owner: bool,
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
            Person { id: *next - 1, name: String::new(), pages: 0 }
        })
    }

    /// The number pages know `guest` by.
    pub(crate) fn id(&mut self, guest: &str) -> u32 {
        self.person(guest).id
    }

    pub(crate) fn join(&mut self, guest: &str, name: Option<&str>) {
        let p = self.person(guest);
        p.pages += 1;
        if let Some(name) = name {
            p.name = clean(name);
        }
    }

    pub(crate) fn leave(&mut self, guest: &str) {
        let p = self.person(guest);
        p.pages = p.pages.saturating_sub(1);
    }

    pub(crate) fn rename(&mut self, guest: &str, name: &str) {
        self.person(guest).name = clean(name);
    }

    pub(crate) fn is_admin(&self, guest: &str) -> bool {
        self.admins.contains(guest)
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
        p.join("owner", Some("  coach\n"));
        p.join("viewer", None);
        let shown = p.shown();
        assert_eq!(shown[0], Shown { id: 1, name: "coach".into(), admin: true, owner: true });
        assert_eq!(shown[1], Shown { id: 2, name: "Guest 2".into(), admin: false, owner: false });
        p.rename("viewer", &"x".repeat(40));
        assert_eq!(p.shown()[1].name.len(), MAX_NAME);
    }

    #[test]
    fn someone_with_no_pages_left_is_gone_and_keeps_their_number() {
        let mut p = People::new("owner");
        p.join("viewer", None);
        p.join("viewer", None);
        p.leave("viewer");
        assert_eq!(p.shown().len(), 1, "one page still open");
        p.leave("viewer");
        assert_eq!(p.shown(), []);
        p.join("viewer", None);
        assert_eq!(p.shown()[0].id, 2, "the owner is 1");
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
