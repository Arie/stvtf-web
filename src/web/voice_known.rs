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
//! The site a stv.tf belongs to (`--home`) has Discord users who linked their Steam account
//! there: its whole list is asked for now and then (`GET <home>/discord_links`, with a token
//! sealed with the key the two share) and taken into this one, marked as from that site (`src`).
//! What someone said by hand stands over it.

use super::token::Keys;
use super::voice_in::{is_key, is_voice_id};
use super::voice_who::steamid_of;
use super::{JSON, Request, TEXT, json_body, param, respond};
use crate::maps::cache::write_atomic;
use crate::{App, lock};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::io::{BufReader, Take};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FILE: &str = "voice-who.json";
/// How often the list's home is asked what changed, and how long it gets to answer.
const ASK_EVERY: Duration = Duration::from_secs(30);
const TIMEOUT: Duration = Duration::from_secs(5);
/// How often the site is asked for its links.
const LINKS_EVERY: Duration = Duration::from_secs(600);
/// The most people kept, and the longest a name.
const MAX_KNOWN: usize = 100_000;
const MAX_NAME: usize = 100;

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
    /// The list's version this was said at.
    #[serde(default)]
    v: u64,
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

/// How the list's home is asked: a method, a path, and a body to send; its answer's body.
type Asking = Box<dyn Fn(&str, &str, Option<&str>) -> Result<String> + Send + Sync>;
/// How a site is asked for its links: its answer's body.
pub(crate) type Fetching = Box<dyn Fn() -> Result<String> + Send + Sync>;

/// How the site at `home` (a stv.tf's `--home`) is asked for its links, and its name.
pub(crate) fn from_site(home: &str, keys: Arc<Keys>) -> (String, Fetching) {
    let url = format!("{}/discord_links", home.trim_end_matches('/'));
    let name = home.split("://").last().unwrap_or(home).split('/').next().unwrap_or_default().to_string();
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build();
    let fetch = move || {
        let bearer = format!("Bearer {}", keys.links_token(super::unix_now()));
        Ok(agent.get(&url).set("Authorization", &bearer).call().context("the site's Discord links")?.into_string()?)
    };
    (name, Box::new(fetch))
}

struct Inner {
    file: Option<PathBuf>,
    kept: Mutex<Kept>,
    home: Option<Asking>,
    /// The site whose links are taken in: its name, and how it is asked.
    site: Option<(String, Fetching)>,
    /// Wakes the telling when something was said.
    said: Mutex<Option<Sender<()>>>,
}

#[derive(Clone)]
pub(crate) struct WhoHistory(Arc<Inner>);

impl Default for WhoHistory {
    fn default() -> Self {
        Self::new(None, None, None)
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
        .map(|(id, steamid)| (id, Known { steamid: Some(steamid), name: String::new(), src: String::new(), v: 1 }))
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
    /// the one there; with a `site`, its links taken in.
    pub(crate) fn new(dir: Option<&Path>, home: Option<(&str, &str)>, site: Option<(String, Fetching)>) -> Self {
        Self::asking(dir, home.map(|(url, key)| over_http(url, key)), site)
    }

    fn asking(dir: Option<&Path>, home: Option<Asking>, site: Option<(String, Fetching)>) -> Self {
        let file = dir.map(|d| d.join(FILE));
        let kept = file.as_deref().and_then(read).unwrap_or_default();
        Self(Arc::new(Inner { file, kept: Mutex::new(kept), home, site, said: Mutex::new(None) }))
    }

    /// Keeps the list the same as its home's and its site's from here on, if it has any.
    pub(crate) fn keep_up(&self) {
        if self.0.home.is_none() && self.0.site.is_none() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        *lock(&self.0.said) = Some(tx);
        let list = self.clone();
        std::thread::spawn(move || list.run(&rx));
    }

    fn run(&self, said: &Receiver<()>) {
        let mut asked: Option<Instant> = None;
        loop {
            if asked.is_none_or(|at| at.elapsed() >= LINKS_EVERY) {
                asked = Some(Instant::now());
                if let Err(e) = self.take_links() {
                    eprintln!("stvtf-web: the site's Discord links not taken in: {e:#}");
                }
            }
            if let Err(e) = self.sync() {
                eprintln!("stvtf-web: who is who not kept up: {e:#}");
            }
            if said.recv_timeout(ASK_EVERY) == Err(RecvTimeoutError::Disconnected) {
                return;
            }
        }
    }

    pub(crate) fn get(&self, who: &str) -> Option<String> {
        lock(&self.0.kept).known.get(who)?.steamid.clone()
    }

    fn save(&self, kept: &Kept) {
        if let Some(file) = &self.0.file
            && let Err(e) = write_atomic(file, serde_json::json!(kept).to_string().as_bytes())
        {
            eprintln!("stvtf-web: who is who not kept: {e}");
        }
    }

    /// `who` is `steamid`, or with none nobody, by the `name` they talk under: kept, and told to
    /// the list's home.
    pub(crate) fn remember(&self, who: &str, steamid: Option<&str>, name: &str) {
        let mut kept = lock(&self.0.kept);
        if self.put(&mut kept, who, steamid, name, "") {
            self.save(&kept);
        }
        drop(kept);
        if let Some(said) = lock(&self.0.said).as_ref() {
            let _ = said.send(());
        }
    }

    /// Whether `who` was put in the list so: not when it is full.
    fn put(&self, kept: &mut Kept, who: &str, steamid: Option<&str>, name: &str, src: &str) -> bool {
        if kept.known.len() >= MAX_KNOWN && !kept.known.contains_key(who) {
            return false;
        }
        kept.version += 1;
        let (name, v) = (name.chars().take(MAX_NAME).collect(), kept.version);
        kept.known.insert(who.to_string(), Known { steamid: steamid.map(str::to_string), name, src: src.to_string(), v });
        if self.0.home.is_some() {
            kept.untold.insert(who.to_string());
        }
        true
    }

    /// The links of the site `src`, all of them (each a Discord user and their `SteamID`), taken
    /// in: over nothing, over nobody and over what that site said before, not over whom
    /// someone was put on by hand or by another site. Those it no longer has are nobody again.
    /// How many changed.
    fn take_in(&self, src: &str, links: &HashMap<String, String>) -> usize {
        let mut kept = lock(&self.0.kept);
        let mut changed = 0;
        for (who, steamid) in links {
            let stands = kept.known.get(who).is_some_and(|k| k.steamid.as_ref().is_some_and(|s| s == steamid || k.src != src));
            if !stands && self.put(&mut kept, who, Some(steamid), "", src) {
                changed += 1;
            }
        }
        let gone: Vec<String> = kept
            .known
            .iter()
            .filter(|(who, k)| k.src == src && k.steamid.is_some() && !links.contains_key(*who))
            .map(|(who, _)| who.clone())
            .collect();
        for who in &gone {
            self.put(&mut kept, who, None, "", src);
        }
        changed += gone.len();
        if changed > 0 {
            self.save(&kept);
        }
        changed
    }

    /// Asks the site for its links and takes them in.
    fn take_links(&self) -> Result<()> {
        let Some((src, fetch)) = &self.0.site else { return Ok(()) };
        let theirs: Links = serde_json::from_str(&fetch()?).context("the site's Discord links")?;
        let links = theirs
            .links
            .into_iter()
            .filter(|l| !l.discord.is_empty() && l.discord.len() <= 20 && l.discord.bytes().all(|b| b.is_ascii_digit()))
            .filter_map(|l| Some((l.discord, steamid_of(&l.steam)?)))
            .collect();
        let changed = self.take_in(src, &links);
        if changed > 0 {
            eprintln!("stvtf-web: {changed} Discord links taken in from {src}");
        }
        Ok(())
    }

    /// What another stv.tf says was said there, or taken in from its site (`src`).
    fn learned(&self, who: &str, steamid: Option<&str>, name: &str, src: &str) {
        let mut kept = lock(&self.0.kept);
        if self.put(&mut kept, who, steamid, name, src) {
            self.save(&kept);
        }
    }

    /// What was said since version `since`, and the version now.
    fn since(&self, since: u64) -> (u64, Vec<serde_json::Value>) {
        let kept = lock(&self.0.kept);
        let changed = kept
            .known
            .iter()
            .filter(|(_, k)| k.v > since)
            .map(|(id, k)| serde_json::json!({ "id": id, "steamid": k.steamid, "name": k.name, "src": k.src, "v": k.v }))
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
            let body = serde_json::json!({ "id": id, "steamid": known.steamid, "name": known.name, "src": known.src }).to_string();
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
        for k in theirs.known.into_iter().filter(|k| is_voice_id(&k.id)) {
            // What is said here and not told yet stands.
            if !kept.untold.contains(&k.id) {
                let v = kept.version;
                kept.known.insert(k.id, Known { steamid: k.steamid, name: k.name, src: k.src, v });
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
struct Said {
    id: String,
    #[serde(default)]
    steamid: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    src: String,
}

#[derive(Deserialize)]
struct Links {
    links: Vec<Link>,
}

/// A Discord user and the Steam account (its 64-bit id) they linked on the site.
#[derive(Deserialize)]
struct Link {
    discord: String,
    steam: String,
}

/// `GET /voice/known?since=<version>`, another stv.tf's: what changed since.
pub(super) fn known(s: &mut TcpStream, app: &App, request: &Request) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let since = param(&request.query, "since").and_then(|v| v.parse().ok()).unwrap_or(0);
    let (version, known) = app.voice_who.since(since);
    respond(s, "200 OK", JSON, serde_json::json!({ "version": version, "known": known }).to_string().as_bytes())
}

/// `POST /voice/known`, another stv.tf's: what was said on a page there.
pub(super) fn learn(s: &mut TcpStream, reader: &mut BufReader<Take<TcpStream>>, app: &App, request: &Request) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let Ok(said) = json_body::<Said>(reader, request) else { return respond(s, "400 Bad Request", TEXT, b"Invalid") };
    let steamid = said.steamid.as_deref().map(steamid_of);
    if !is_voice_id(&said.id) || steamid == Some(None) {
        return respond(s, "400 Bad Request", TEXT, b"Invalid");
    }
    let name: String = said.name.chars().filter(|c| !c.is_control()).collect();
    if said.src.len() > MAX_NAME || !said.src.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':')) {
        return respond(s, "400 Bad Request", TEXT, b"Invalid");
    }
    app.voice_who.learned(&said.id, steamid.flatten().as_deref(), &name, &said.src);
    respond(s, "204 No Content", TEXT, b"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn who_someone_was_said_to_be_is_kept_in_a_file_until_they_are_said_to_be_nobody() {
        let dir = TempDir::new("voicewho");
        let who = WhoHistory::new(Some(&dir), None, None);
        who.remember("mumble:cert:abc", Some("[U:1:9]"), "arie");
        who.remember("77", Some("[U:1:5]"), "gummy");
        let again = WhoHistory::new(Some(&dir), None, None);
        assert_eq!(again.get("mumble:cert:abc").as_deref(), Some("[U:1:9]"));
        again.remember("77", None, "gummy");
        assert_eq!(WhoHistory::new(Some(&dir), None, None).get("77"), None);
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
        let who = WhoHistory::asking(Some(&dir), Some(home), None);
        assert_eq!(who.get("77").as_deref(), Some("[U:1:5]"));
        who.sync().unwrap();
        assert_eq!(*lock(&told), [r#"{"id":"77","name":"","src":"","steamid":"[U:1:5]"}"#]);
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
                    home.learned(&said.id, said.steamid.as_deref(), &said.name, &said.src);
                    return Ok(String::new());
                }
                let (version, known) = home.since(path.rsplit('=').next().unwrap().parse()?);
                Ok(serde_json::json!({ "version": version, "known": known }).to_string())
            })),
            None,
        )
    }

    #[test]
    fn what_is_said_in_one_region_is_known_in_the_next_through_the_home() {
        let (home, down) = (WhoHistory::default(), Arc::new(Mutex::new(false)));
        let (na, au) = (away(&home, &down), away(&home, &down));
        na.remember("77", Some("[U:1:5]"), "gummy");
        na.sync().unwrap();
        au.sync().unwrap();
        assert_eq!((home.get("77"), au.get("77")), (Some("[U:1:5]".into()), Some("[U:1:5]".into())));
        // Said to be nobody, in the other region: no longer anyone, anywhere.
        au.remember("77", None, "gummy");
        au.sync().unwrap();
        na.sync().unwrap();
        assert_eq!((home.get("77"), na.get("77")), (None, None));
        // The home not answering: what is said stands here, and is told once it does.
        *lock(&down) = true;
        na.remember("78", Some("[U:1:6]"), "aoki");
        assert!(na.sync().is_err());
        assert_eq!((na.get("78").as_deref(), home.get("78")), (Some("[U:1:6]"), None));
        *lock(&down) = false;
        na.sync().unwrap();
        assert_eq!(home.get("78").as_deref(), Some("[U:1:6]"));
    }

    #[test]
    fn a_sites_links_are_taken_in_but_not_over_what_someone_said_and_go_when_the_site_drops_them() {
        let (home, down) = (WhoHistory::default(), Arc::new(Mutex::new(false)));
        let (na, au) = (away(&home, &down), away(&home, &down));
        let links = |pairs: &[(&str, &str)]| pairs.iter().map(|(d, s)| (d.to_string(), s.to_string())).collect::<HashMap<_, _>>();
        // By hand, 78 plays on another account than the one they linked.
        na.remember("78", Some("[U:1:60]"), "aoki");
        assert_eq!(na.take_in("na.serveme.tf", &links(&[("77", "[U:1:5]"), ("78", "[U:1:6]")])), 1);
        assert_eq!((na.get("77").as_deref(), na.get("78").as_deref()), (Some("[U:1:5]"), Some("[U:1:60]")));
        assert_eq!(na.take_in("na.serveme.tf", &links(&[("77", "[U:1:5]"), ("78", "[U:1:6]")])), 0, "the same again changes nothing");
        // Known in the next region, as from that site: its own site's other link does not take its place.
        na.sync().unwrap();
        au.sync().unwrap();
        assert_eq!(au.get("77").as_deref(), Some("[U:1:5]"));
        assert_eq!(au.take_in("au.serveme.tf", &links(&[("77", "[U:1:9]")])), 0);
        // Linked anew on the site, then no longer: taken over, then nobody.
        assert_eq!(na.take_in("na.serveme.tf", &links(&[("77", "[U:1:7]")])), 1);
        assert_eq!(na.get("77").as_deref(), Some("[U:1:7]"));
        assert_eq!(na.take_in("na.serveme.tf", &links(&[])), 1);
        assert_eq!((na.get("77"), na.get("78").as_deref()), (None, Some("[U:1:60]")));
    }

    #[test]
    fn a_site_is_asked_for_its_links_as_it_writes_them() {
        let site: Fetching = Box::new(|| {
            Ok(r#"{"links":[{"discord":"77","steam":"76561197960265737"},{"discord":"x","steam":"76561197960265737"},{"discord":"78","steam":"gaben"}]}"#.into())
        });
        let who = WhoHistory::asking(None, None, Some(("serveme.tf".into(), site)));
        who.take_links().unwrap();
        assert_eq!((who.get("77").as_deref(), who.get("x"), who.get("78")), (Some("[U:1:9]"), None, None));
    }
}
