//! Who is who, from one match to the next and from one region to the next: who someone is where
//! they talk (a Discord user, a Mumble certificate), the `SteamID` someone put them on, and their
//! name there. Kept in the store (`voice-who.json`).
//!
//! One stv.tf holds the list for all (`STVTF_VOICE_HOME` names it to the others; the one named
//! has none set). The others tell it what is said on their pages and ask it what changed, with
//! the voice bot's key:
//!
//! - `GET /voice/known?since=<version>`: what changed since, and the `version` of that.
//! - `POST /voice/known`: `id` is `steamid` (or with none, nobody), by the `name` they talk under.
//!
//! The sites stv.tf belongs to (`STVTF_LINK_SITES`, else its `--home`) have Discord users who
//! linked their Steam account there. Someone in a channel whom nothing is known of is asked
//! about, one site after the other (`GET <site>/discord_links/<id>`, with a token for that one
//! user sealed with the key they share), and what a site says is kept, marked as from it (`src`).
//!
//! Not all of it is as sure. A link someone made themselves (a site's) and what a caster said
//! hold everywhere as said. What a match's starter said holds in their match, and elsewhere is a
//! suggestion: shown as one, and it never takes the place of the surer.

use super::ids::{is_voice_id, steamid_of};
use crate::lock;
use crate::maps::cache::write_atomic;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FILE: &str = "voice-who.json";
/// How often the list's home is asked what changed, and how long it gets to answer.
const ASK_EVERY: Duration = Duration::from_secs(30);
pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);
/// How long what the sites said of someone stands before they are asked again: a link, and none.
const LINK_FOR: Duration = Duration::from_hours(24);
const NO_LINK_FOR: Duration = Duration::from_mins(10);
/// The most people waiting to be asked about.
const MAX_WANTED: usize = 1000;
/// The most people kept, and the longest a name.
const MAX_KNOWN: usize = 100_000;
pub(crate) const MAX_NAME: usize = 100;

/// Whom someone was said to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Known {
    /// The `SteamID` they were put on; none for someone said to be nobody.
    pub(crate) steamid: Option<String>,
    /// Their name where they talk, when that was said.
    #[serde(default)]
    pub(crate) name: String,
    /// The site whose list of links this is from; empty for what someone said on a page.
    #[serde(default)]
    src: String,
    /// Said on a page by a caster (`caster`), else by a match's starter.
    #[serde(default)]
    by: String,
    /// The list's version this was said at.
    #[serde(default)]
    v: u64,
}

pub(crate) const CASTER: &str = "caster";

impl Known {
    /// How sure: a match's starter's word, a link of someone's own, a caster's word.
    fn sure(&self) -> u8 {
        match (self.src.is_empty(), self.by == CASTER) {
            (false, _) => 2,
            (true, true) => 3,
            (true, false) => 1,
        }
    }
}

/// Whom someone is known as from before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Recalled {
    /// This `SteamID`: as said (`sure`), or a suggestion.
    Player {
        steamid: String,
        sure: bool,
    },
    Nobody,
}

#[derive(Default, Serialize, Deserialize)]
struct Kept {
    /// Counts what is said here.
    version: u64,
    /// The version of the home's list had so far.
    #[serde(default)]
    had: u64,
    /// Said here, and not told to the home yet.
    #[serde(default)]
    untold: BTreeSet<String>,
    known: HashMap<String, Known>,
}

impl Kept {
    /// Whether what is known of `who` gives way to something as sure as `sure`: nothing, nobody,
    /// and what is no surer do.
    fn yields(&self, who: &str, sure: u8) -> bool {
        self.known.get(who).is_none_or(|k| k.steamid.is_none() || k.sure() <= sure)
    }
}

/// How the list's home is asked: a method, a path, and a body to send; its answer's body.
type Asking = Box<dyn Fn(&str, &str, Option<&str>) -> Result<String> + Send + Sync>;
/// How a site is asked who a Discord user is: the Steam account (its 64-bit id) they linked
/// there, if any.
pub(crate) type Fetching = Box<dyn Fn(&str) -> Result<Option<String>> + Send + Sync>;

/// A site with links, by its name.
pub(crate) struct Site {
    pub(crate) name: String,
    pub(crate) ask: Fetching,
}

struct Inner {
    file: Option<PathBuf>,
    kept: Mutex<Kept>,
    home: Option<Asking>,
    /// The sites asked who a Discord user is.
    sites: Vec<Site>,
    /// Those to ask the sites about, and when each was last asked and whether a site knew them.
    wanted: Mutex<VecDeque<String>>,
    looked: Mutex<HashMap<String, (Instant, bool)>>,
    /// Wakes the telling when something was said.
    said: Mutex<Option<Sender<()>>>,
}

#[derive(Clone)]
pub(crate) struct WhoHistory(Arc<Inner>);

impl Default for WhoHistory {
    fn default() -> Self {
        Self::new(None, None, Vec::new())
    }
}

/// The list as read from the store: as kept now, or as it was (each id and its `SteamID`).
fn read(file: &Path) -> Option<Kept> {
    let data = std::fs::read(file).ok()?;
    if let Ok(kept) = serde_json::from_slice::<Kept>(&data) {
        return Some(kept);
    }
    let old: HashMap<String, String> = serde_json::from_slice(&data).ok()?;
    let untold = old.keys().cloned().collect();
    let known = old
        .into_iter()
        .map(|(id, steamid)| (id, Known { steamid: Some(steamid), name: String::new(), src: String::new(), by: String::new(), v: 1 }))
        .collect();
    Some(Kept { version: 1, had: 0, untold, known })
}

/// Asks the stv.tf at `url` with the bot's `key`.
fn over_http(url: &str, key: &str) -> Asking {
    let (url, bearer) = (url.trim_end_matches('/').to_string(), format!("Bearer {key}"));
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build();
    Box::new(move |method, path, body| {
        let ask = agent.request(method, &format!("{url}{path}")).set("Authorization", &bearer);
        let answer = match body {
            Some(body) => ask.set("Content-Type", "application/json").send_string(body),
            None => ask.call(),
        };
        Ok(answer.context("the list of who is who")?.into_string()?)
    })
}

impl WhoHistory {
    /// The list kept in `dir`; with a `home` (its address and the bot's key), kept the same as
    /// the one there; with `sites`, they are asked who a Discord user is.
    pub(crate) fn new(dir: Option<&Path>, home: Option<(&str, &str)>, sites: Vec<Site>) -> Self {
        Self::asking(dir, home.map(|(url, key)| over_http(url, key)), sites)
    }

    fn asking(dir: Option<&Path>, home: Option<Asking>, sites: Vec<Site>) -> Self {
        let file = dir.map(|d| d.join(FILE));
        let kept = file.as_deref().and_then(read).unwrap_or_default();
        let (wanted, looked) = Default::default();
        Self(Arc::new(Inner { file, kept: Mutex::new(kept), home, sites, wanted, looked, said: Mutex::new(None) }))
    }

    /// Keeps the list the same as its home's from here on, and asks the sites, if it has any.
    pub(crate) fn keep_up(&self) {
        if self.0.home.is_none() && self.0.sites.is_empty() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        *lock(&self.0.said) = Some(tx);
        let list = self.clone();
        std::thread::spawn(move || list.run(&rx));
    }

    fn run(&self, said: &Receiver<()>) {
        loop {
            loop {
                let Some(who) = lock(&self.0.wanted).pop_front() else { break };
                self.find(&who);
            }
            if let Err(e) = self.sync() {
                log!("who is who not kept up: {e:#}");
            }
            if said.recv_timeout(ASK_EVERY) == Err(RecvTimeoutError::Disconnected) {
                return;
            }
        }
    }

    #[cfg(test)]
    fn get(&self, who: &str) -> Option<String> {
        lock(&self.0.kept).known.get(who)?.steamid.clone()
    }

    /// Whom `who` is known as from before, and how sure that is.
    pub(crate) fn recall(&self, who: &str) -> Option<Recalled> {
        let kept = lock(&self.0.kept);
        let known = kept.known.get(who)?;
        Some(match &known.steamid {
            Some(steamid) => Recalled::Player { steamid: steamid.clone(), sure: known.sure() >= 2 },
            None => Recalled::Nobody,
        })
    }

    fn save(&self, kept: &Kept) {
        if let Some(file) = &self.0.file
            && let Err(e) = write_atomic(file, serde_json::json!(kept).to_string().as_bytes())
        {
            log!("who is who not kept: {e}");
        }
    }

    /// `who` is `steamid`, or with none nobody, by the `name` they talk under, said by a
    /// `caster` or by a match's starter: kept, and told to the list's home, unless someone surer
    /// said otherwise.
    pub(crate) fn remember(&self, who: &str, steamid: Option<&str>, name: &str, caster: bool) {
        let mut kept = lock(&self.0.kept);
        let by = if caster { CASTER } else { "" };
        if kept.yields(who, if caster { 3 } else { 1 }) && self.put(&mut kept, who, steamid, name, "", by) {
            self.save(&kept);
        }
        drop(kept);
        if let Some(said) = lock(&self.0.said).as_ref() {
            let _ = said.send(());
        }
    }

    /// Whether `who` was put in the list so: not when it is full.
    fn put(&self, kept: &mut Kept, who: &str, steamid: Option<&str>, name: &str, src: &str, by: &str) -> bool {
        if kept.known.len() >= MAX_KNOWN && !kept.known.contains_key(who) {
            return false;
        }
        kept.version += 1;
        let (name, v) = (name.chars().take(MAX_NAME).collect(), kept.version);
        kept.known
            .insert(who.to_string(), Known { steamid: steamid.map(str::to_string), name, src: src.to_string(), by: by.to_string(), v });
        if self.0.home.is_some() {
            kept.untold.insert(who.to_string());
        }
        true
    }

    /// Someone is in a channel: the sites are asked who they are, if they are a Discord user
    /// and were not asked about lately. What the sites say is known a little later.
    pub(crate) fn look_up(&self, who: &str) {
        if self.0.sites.is_empty() || who.is_empty() || who.len() > 20 || !who.bytes().all(|b| b.is_ascii_digit()) {
            return;
        }
        {
            let mut looked = lock(&self.0.looked);
            let fresh = |(at, found): &(Instant, bool)| at.elapsed() < if *found { LINK_FOR } else { NO_LINK_FOR };
            if looked.get(who).is_some_and(fresh) || looked.len() >= MAX_KNOWN {
                return;
            }
            looked.insert(who.to_string(), (Instant::now(), false));
        }
        let mut wanted = lock(&self.0.wanted);
        if wanted.len() < MAX_WANTED {
            wanted.push_back(who.to_string());
            drop(wanted);
            if let Some(said) = lock(&self.0.said).as_ref() {
                let _ = said.send(());
            }
        }
    }

    /// Asks the sites, one after the other, who Discord user `who` is, and keeps what the first
    /// that knows them says. None knowing them, a link kept from before is gone; unless a site
    /// did not answer, which says nothing.
    fn find(&self, who: &str) {
        let mut all_said = true;
        for site in &self.0.sites {
            match (site.ask)(who) {
                Ok(Some(steam)) => {
                    if let Some(steamid) = steamid_of(&steam) {
                        lock(&self.0.looked).insert(who.to_string(), (Instant::now(), true));
                        self.found(who, Some((&site.name, &steamid)));
                        return;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    all_said = false;
                    log!("{} not asked who a Discord user is: {e:#}", site.name);
                }
            }
        }
        if all_said {
            self.found(who, None);
        }
    }

    /// What the sites say of `who`: linked there (the site, and the `SteamID`), or nowhere.
    /// A link is kept over nothing, nobody, a match's starter's word and a link from before,
    /// not over a caster's word; with none, one kept from a site is nobody again. Whether that
    /// changed anything.
    fn found(&self, who: &str, link: Option<(&str, &str)>) -> bool {
        let mut kept = lock(&self.0.kept);
        let before = kept.known.get(who);
        let changed = match link {
            Some((src, steamid)) => {
                let stands = before.is_some_and(|k| k.steamid.as_deref().is_some_and(|s| s == steamid || k.sure() == 3));
                !stands && self.put(&mut kept, who, Some(steamid), "", src, "")
            }
            None => match before.filter(|k| k.steamid.is_some() && !k.src.is_empty()).map(|k| k.src.clone()) {
                Some(src) => self.put(&mut kept, who, None, "", &src, ""),
                None => false,
            },
        };
        if changed {
            self.save(&kept);
            drop(kept);
            if let Some(said) = lock(&self.0.said).as_ref() {
                let _ = said.send(());
            }
        }
        changed
    }

    /// What another stv.tf says was said there, or taken in from its site (`src`).
    pub(crate) fn learned(&self, who: &str, steamid: Option<&str>, name: &str, src: &str, by: &str) {
        let mut kept = lock(&self.0.kept);
        let sure = Known { steamid: None, name: String::new(), src: src.to_string(), by: by.to_string(), v: 0 }.sure();
        if kept.yields(who, sure) && self.put(&mut kept, who, steamid, name, src, by) {
            self.save(&kept);
        }
    }

    /// What was said since version `since`, and the version now.
    pub(crate) fn since(&self, since: u64) -> (u64, Vec<serde_json::Value>) {
        let kept = lock(&self.0.kept);
        let changed = kept
            .known
            .iter()
            .filter(|(_, k)| k.v > since)
            .map(|(id, k)| serde_json::json!({ "id": id, "steamid": k.steamid, "name": k.name, "src": k.src, "by": k.by, "v": k.v }))
            .collect();
        (kept.version, changed)
    }

    /// Tells the home what was said here, then takes what changed there.
    fn sync(&self) -> Result<()> {
        let Some(ask) = &self.0.home else { return Ok(()) };
        let untold: Vec<(String, Known)> = {
            let kept = lock(&self.0.kept);
            kept.untold.iter().filter_map(|id| Some((id.clone(), kept.known.get(id)?.clone()))).collect()
        };
        for (id, known) in untold {
            let body =
                serde_json::json!({ "id": id, "steamid": known.steamid, "name": known.name, "src": known.src, "by": known.by }).to_string();
            ask("POST", "/voice/known", Some(&body))?;
            let mut kept = lock(&self.0.kept);
            // Said anew meanwhile: told the next time.
            if kept.known.get(&id) == Some(&known) {
                kept.untold.remove(&id);
            }
        }
        let had = lock(&self.0.kept).had;
        let theirs: Theirs = serde_json::from_str(&ask("GET", &format!("/voice/known?since={had}"), None)?)?;
        let mut kept = lock(&self.0.kept);
        if theirs.known.is_empty() && kept.had == theirs.version {
            return Ok(());
        }
        for k in theirs.known.into_iter().filter(|k| is_voice_id(&k.id)) {
            // What is said here and not told yet stands.
            if !kept.untold.contains(&k.id) {
                let v = kept.version;
                kept.known.insert(k.id, Known { steamid: k.steamid, name: k.name, src: k.src, by: k.by, v });
            }
        }
        kept.had = theirs.version;
        self.save(&kept);
        Ok(())
    }
}

#[derive(Deserialize)]
struct Theirs {
    version: u64,
    known: Vec<Said>,
}

#[derive(Deserialize)]
pub(crate) struct Said {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) steamid: Option<String>,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) src: String,
    #[serde(default)]
    pub(crate) by: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn who_someone_was_said_to_be_is_kept_in_a_file_until_they_are_said_to_be_nobody() {
        let dir = TempDir::new("voicewho");
        let who = WhoHistory::new(Some(&dir), None, Vec::new());
        who.remember("mumble:cert:abc", Some("[U:1:9]"), "arie", true);
        who.remember("77", Some("[U:1:5]"), "gummy", true);
        let again = WhoHistory::new(Some(&dir), None, Vec::new());
        assert_eq!(again.get("mumble:cert:abc").as_deref(), Some("[U:1:9]"));
        again.remember("77", None, "gummy", true);
        assert_eq!(WhoHistory::new(Some(&dir), None, Vec::new()).get("77"), None);
    }

    #[test]
    fn the_list_as_it_was_kept_before_is_read_and_told_to_the_home() {
        let dir = TempDir::new("voicewho-old");
        std::fs::write(dir.join(FILE), r#"{"77":"[U:1:5]"}"#).unwrap();
        let told = Arc::new(Mutex::new(Vec::new()));
        let hears = told.clone();
        let home: Asking = Box::new(move |method, _, body| {
            if method == "POST" {
                lock(&hears).push(body.unwrap().to_string());
            }
            Ok(r#"{"version":0,"known":[]}"#.into())
        });
        let who = WhoHistory::asking(Some(&dir), Some(home), Vec::new());
        assert_eq!(who.get("77").as_deref(), Some("[U:1:5]"));
        who.sync().unwrap();
        assert_eq!(*lock(&told), [r#"{"by":"","id":"77","name":"","src":"","steamid":"[U:1:5]"}"#]);
    }

    /// A list whose home is the list `home`, asked as its routes would be.
    fn away(home: &WhoHistory, down: &Arc<Mutex<bool>>) -> WhoHistory {
        let (home, down) = (home.clone(), down.clone());
        WhoHistory::asking(
            None,
            Some(Box::new(move |method, path, body| {
                anyhow::ensure!(!*lock(&down), "down");
                if method == "POST" {
                    let said: Said = serde_json::from_str(body.unwrap())?;
                    home.learned(&said.id, said.steamid.as_deref(), &said.name, &said.src, &said.by);
                    return Ok(String::new());
                }
                let (version, known) = home.since(path.rsplit('=').next().unwrap().parse()?);
                Ok(serde_json::json!({ "version": version, "known": known }).to_string())
            })),
            Vec::new(),
        )
    }

    #[test]
    fn what_is_said_in_one_region_is_known_in_the_next_through_the_home() {
        let (home, down) = (WhoHistory::default(), Arc::new(Mutex::new(false)));
        let (na, au) = (away(&home, &down), away(&home, &down));
        na.remember("77", Some("[U:1:5]"), "gummy", true);
        na.sync().unwrap();
        au.sync().unwrap();
        assert_eq!((home.get("77"), au.get("77")), (Some("[U:1:5]".into()), Some("[U:1:5]".into())));
        // Said to be nobody, in the other region: no longer anyone, anywhere.
        au.remember("77", None, "gummy", true);
        au.sync().unwrap();
        na.sync().unwrap();
        assert_eq!((home.get("77"), na.get("77")), (None, None));
        // The home not answering: what is said stands here, and is told once it does.
        *lock(&down) = true;
        na.remember("78", Some("[U:1:6]"), "aoki", true);
        assert!(na.sync().is_err());
        assert_eq!((na.get("78").as_deref(), home.get("78")), (Some("[U:1:6]"), None));
        *lock(&down) = false;
        na.sync().unwrap();
        assert_eq!(home.get("78").as_deref(), Some("[U:1:6]"));
    }

    #[test]
    fn what_a_site_says_of_someone_is_kept_but_not_over_a_casters_word_and_goes_when_no_site_knows_them() {
        let (home, down) = (WhoHistory::default(), Arc::new(Mutex::new(false)));
        let (na, au) = (away(&home, &down), away(&home, &down));
        // A caster says 78 plays on another account than the one they linked.
        na.remember("78", Some("[U:1:60]"), "aoki", true);
        assert!(na.found("77", Some(("na.serveme.tf", "[U:1:5]"))) && !na.found("78", Some(("na.serveme.tf", "[U:1:6]"))));
        assert_eq!((na.get("77").as_deref(), na.get("78").as_deref()), (Some("[U:1:5]"), Some("[U:1:60]")));
        assert!(!na.found("77", Some(("na.serveme.tf", "[U:1:5]"))), "the same again changes nothing");
        // Known in the next region, as from that site.
        na.sync().unwrap();
        au.sync().unwrap();
        assert_eq!(au.recall("77"), Some(Recalled::Player { steamid: "[U:1:5]".into(), sure: true }));
        // Linked anew, then no longer anywhere: taken over, then nobody. A caster's word stands.
        assert!(na.found("77", Some(("serveme.tf", "[U:1:7]"))));
        assert_eq!(na.get("77").as_deref(), Some("[U:1:7]"));
        assert!(na.found("77", None) && !na.found("78", None));
        assert_eq!((na.get("77"), na.get("78").as_deref()), (None, Some("[U:1:60]")));
    }

    /// A site that knows `links` (Discord users and their 64-bit Steam ids), or does not answer.
    fn knows(name: &str, links: &[(&str, &str)], asked: &Arc<Mutex<Vec<String>>>, down: bool) -> Site {
        let links: HashMap<String, String> = links.iter().map(|(d, s)| (d.to_string(), s.to_string())).collect();
        let (asked, name) = (asked.clone(), name.to_string());
        let site = name.clone();
        let ask: Fetching = Box::new(move |who| {
            lock(&asked).push(format!("{site} {who}"));
            anyhow::ensure!(!down, "down");
            Ok(links.get(who).cloned())
        });
        Site { name, ask }
    }

    #[test]
    fn someone_in_a_channel_is_asked_about_at_each_site_until_one_knows_them_and_not_again_soon() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let sites = vec![
            knows("eu", &[], &asked, false),
            knows("na", &[("77", "76561197960265737")], &asked, false),
            knows("au", &[], &asked, false),
        ];
        let who = WhoHistory::asking(None, None, sites);
        for id in ["77", "77", "mumble:cert:abc", "78"] {
            who.look_up(id);
        }
        let wanted: Vec<String> = lock(&who.0.wanted).drain(..).collect();
        assert_eq!(wanted, ["77", "78"], "a Discord user, once");
        for id in &wanted {
            who.find(id);
        }
        assert_eq!(*lock(&asked), ["eu 77", "na 77", "eu 78", "na 78", "au 78"]);
        assert_eq!(who.recall("77"), Some(Recalled::Player { steamid: "[U:1:9]".into(), sure: true }));
        assert_eq!(who.recall("78"), None);
    }

    #[test]
    fn a_site_not_answering_takes_no_link_away() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let who = WhoHistory::asking(None, None, vec![knows("eu", &[], &asked, true), knows("na", &[], &asked, false)]);
        who.found("77", Some(("eu", "[U:1:5]")));
        who.find("77");
        assert_eq!(who.get("77").as_deref(), Some("[U:1:5]"));
    }

    #[test]
    fn a_match_starters_word_is_a_suggestion_elsewhere_and_gives_way_to_the_surer() {
        let who = WhoHistory::default();
        let player = |steamid: &str, sure| Some(Recalled::Player { steamid: steamid.into(), sure });
        who.remember("77", Some("[U:1:5]"), "gummy", false);
        assert_eq!(who.recall("77"), player("[U:1:5]", false));
        // Their own link takes its place, and a starter's word no longer does.
        assert!(who.found("77", Some(("serveme.tf", "[U:1:6]"))));
        who.remember("77", Some("[U:1:7]"), "gummy", false);
        assert_eq!(who.recall("77"), player("[U:1:6]", true));
        // A caster's word does, and their link then leaves it be.
        who.remember("77", Some("[U:1:8]"), "gummy", true);
        assert!(!who.found("77", Some(("serveme.tf", "[U:1:6]"))));
        assert_eq!(who.recall("77"), player("[U:1:8]", true));
        // Nobody is remembered too, until something is said of them.
        who.remember("78", None, "coach", false);
        assert_eq!(who.recall("78"), Some(Recalled::Nobody));
        assert_eq!(who.recall("79"), None);
    }
}
