//! A live demo's voice: who spoke, picked by whom, and the demo with it built in.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};

use super::downloads::{load_tick, map_of};
use super::sweep::KEPT_PREFIX;
use crate::app::is_name;
use crate::matches::sessions::LIVE_PREFIX;

/// What follows a demo's name in that of its voice.
pub(super) const VOICE: &str = ".voice";

/// Something said from outside the match, as the match's stream came to its tick: what writing it
/// into a demo takes.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct Spoken {
    /// The server's tick it was heard at.
    pub(crate) tick: u32,
    #[serde(flatten)]
    pub(crate) by: By,
    /// Opus frames, base64.
    pub(crate) frames: Vec<String>,
}

/// Whose voice it was.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum By {
    /// A player's then: their slot (their entity less one) and Steam account.
    Player { slot: u8, account: u32 },
    /// No player's then, by who they are where they talk (as their lines name them) and the
    /// name they talk under: a player's once someone says whose, when the demo is built.
    Voice {
        who: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// Whose account someone in a voice channel is said to be by now, by who they are there.
pub(crate) type WhoNow<'a> = &'a dyn Fn(&str) -> Option<u32>;

/// A demo's voice file.
pub(super) fn voice_of(demo: &Path) -> PathBuf {
    let mut name = demo.as_os_str().to_owned();
    name.push(VOICE);
    PathBuf::from(name)
}

/// Whether anything was said in `demo`.
pub(super) fn has_voice(demo: &Path) -> bool {
    std::fs::metadata(voice_of(demo)).is_ok_and(|m| m.len() > 0)
}

/// Match `id`'s demo `file`, with comms: being recorded, or kept.
pub(super) fn with_comms(dir: &Path, id: &str, file: &str) -> Option<PathBuf> {
    if load_tick(file).is_none() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    [LIVE_PREFIX, KEPT_PREFIX].iter().map(|p| dir.join(format!("{p}{id}-{file}.dem"))).find(|p| has_voice(p))
}

/// Where what was said of whose voice is whose in match `id`'s demo `file` is kept: for that demo
/// alone, whatever is known of them elsewhere.
pub(super) fn picks_file(dir: &Path, id: &str, file: &str) -> PathBuf {
    dir.join(format!("picks-{id}-{file}.json"))
}

/// Whom those who talked in match `id`'s demo `file` as no player were said to be since, by who
/// they are where they talk: a `SteamID`, or nobody.
pub(crate) fn picks(dir: &Path, id: &str, file: &str) -> BTreeMap<String, Option<String>> {
    std::fs::read(picks_file(dir, id, file)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// `who` in match `id`'s demo `file` is the player with `steamid`, or nobody.
pub(crate) fn pick(dir: &Path, id: &str, file: &str, who: &str, steamid: Option<&str>) -> Result<()> {
    with_comms(dir, id, file).context("no such demo")?;
    let mut all = picks(dir, id, file);
    all.insert(who.to_string(), steamid.map(str::to_string));
    Ok(std::fs::write(picks_file(dir, id, file), serde_json::to_vec(&all)?)?)
}

/// Whose account `voice` is in a demo with `picks`: as said of that demo, else as known (`who`).
pub(super) fn whose(picks: &BTreeMap<String, Option<String>>, who: WhoNow, voice: &str) -> Option<u32> {
    match picks.get(voice) {
        Some(steamid) => crate::voice::ids::account_of(steamid.as_deref()?),
        None => who(voice),
    }
}

/// What was said in `demo`, as kept beside it. A line cut short (the match still going) is left
/// out.
pub(super) fn spoken_in(demo: &Path) -> Result<Vec<Spoken>> {
    let lines = std::io::BufReader::new(File::open(voice_of(demo))?).lines();
    Ok(lines.map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect())
}

/// What tells match `id`'s demo `file` with its comms from any other, and from itself once more
/// was recorded or said, or someone in it is said to be another (`who`): None if it is none.
pub(crate) fn comms_key(dir: &Path, id: &str, file: &str, who: WhoNow) -> Option<String> {
    let path = with_comms(dir, id, file)?;
    let len = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
    let mut said: Vec<String> = spoken_in(&path)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| match s.by {
            By::Voice { who, .. } => Some(who),
            By::Player { .. } => None,
        })
        .collect();
    said.sort();
    said.dedup();
    let picks = picks(dir, id, file);
    let whom: Vec<String> = said.iter().filter_map(|w| Some(format!("{w}={}", whose(&picks, who, w)?))).collect();
    Some(format!("{id}-{file}-{}-{}-{}", len(&path), len(&voice_of(&path)), whom.join(",")))
}

/// Samples a second of the voice kept.
pub(super) const RATE: u16 = 24000;

/// How often the players' slots are looked at in a demo, for the voices put on them since.
pub(super) const SLOTS_HZ: f32 = 1.0;

/// Where each account's player was in `demo`, snapshot by snapshot: the tick, and each one's slot
/// (their entity less one). Of a demo still being written, as far as it reads.
pub(super) fn slots_in(demo: &[u8]) -> Vec<(u32, Vec<(u32, u8)>)> {
    let mut slots = Vec::new();
    let _ = stvtf::decode::demo_file(demo, SLOTS_HZ, &mut |line| {
        if let stvtf::Line::Snapshot(snap) = line {
            let slot =
                |p: &stvtf::output::Player| Some((crate::voice::ids::account_of(&p.steamid)?, u8::try_from(p.ent.checked_sub(1)?).ok()?));
            slots.push((snap.tick, snap.players.iter().filter_map(slot).collect()));
        }
    });
    slots
}

/// A demo with its comms written into it.
pub(crate) struct Built {
    /// What to call it.
    pub(crate) name: String,
    pub(crate) demo: Vec<u8>,
    /// What was said in it by those who are no player's still: not in the demo, lines of their
    /// own for whoever watches it here.
    pub(crate) nobodys: Vec<stvtf::output::Voice>,
}

/// Those who talked in match `id`'s demo `file` as no player: who each is where they talk, and
/// the name they talked under.
pub(crate) fn voices_in(dir: &Path, id: &str, file: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for s in with_comms(dir, id, file).and_then(|p| spoken_in(&p).ok()).unwrap_or_default() {
        if let By::Voice { who, name } = s.by
            && !found.iter().any(|(w, _)| *w == who)
        {
            found.push((who, name.unwrap_or_default()));
        }
    }
    found
}

/// Match `id`'s demo `file` as TF2 plays it, the comms written into it as its players' voice. A
/// voice that was no player's is the player's it is said to be by now (`who`), where that one
/// was playing; else it is nobody's still.
pub(crate) fn build(dir: &Path, id: &str, file: &str, who: WhoNow) -> Result<Built> {
    let path = with_comms(dir, id, file).context("no such demo")?;
    let first = i64::from(load_tick(file).context("no such demo")?.1);
    let map = map_of(&path).context("not a demo")?;
    if !is_name(&map, b"_-") {
        bail!("a map of that name");
    }
    let said = spoken_in(&path)?;
    let picks = picks(dir, id, file);
    // Whose each voice is now, asked once; the players' slots only if any is someone's.
    let mut whom: HashMap<String, Option<u32>> = HashMap::new();
    for s in &said {
        if let By::Voice { who: voice, .. } = &s.by
            && !whom.contains_key(voice)
        {
            whom.insert(voice.clone(), whose(&picks, who, voice));
        }
    }
    let slots = if whom.values().any(Option::is_some) { slots_in(&std::fs::read(&path)?) } else { Vec::new() };
    let slot_at = |tick: u32, account: u32| {
        let at = slots.partition_point(|(t, _)| *t <= tick).checked_sub(1)?;
        slots[at].1.iter().find(|(a, _)| *a == account).map(|(_, slot)| *slot)
    };
    let b64 = base64::engine::general_purpose::STANDARD;
    let (mut voices, mut nobodys) = (Vec::new(), Vec::new());
    for s in said {
        let player = match &s.by {
            By::Player { slot, account } => Some((*slot, *account)),
            By::Voice { who: voice, .. } => {
                whom.get(voice.as_str()).copied().flatten().and_then(|account| Some((slot_at(s.tick, account)?, account)))
            }
        };
        match (player, s.by) {
            (Some((slot, account)), _) => voices.push(stvtf::demo::Voiced {
                tick: i32::try_from(i64::from(s.tick) - first).unwrap_or(0),
                slot,
                account,
                rate: RATE,
                frames: s.frames.iter().filter_map(|f| b64.decode(f).ok()).collect(),
            }),
            (None, By::Voice { who, name }) => nobodys.push(stvtf::output::Voice {
                tick: s.tick,
                player: None,
                steamid: who,
                rate: RATE,
                frames: s.frames,
                injected: true,
                pause: false,
                team: None,
                name,
            }),
            (None, By::Player { .. }) => {}
        }
    }
    let src = std::io::BufReader::new(File::open(&path)?);
    let demo = stvtf::demo::with_voice(src, &voices, std::io::Cursor::new(Vec::new()))?.into_inner();
    Ok(Built { name: format!("{map}-comms-{file}.dem"), demo, nobodys })
}
