//! Live top-down web viewer for a TF2 match, fed by stvtf.
//!
//! Given a server, one `SourceTV` connection fanned out to every browser over Server-Sent
//! Events; without one, a page to start watching any server, each match at its own link.
//! Map geometry comes from the map's BSP on fastdl.

mod app;
mod cli;
mod housekeeping;
mod log;
mod maps;
mod matches;
mod relays;
mod report;
mod rooms;
mod start;
mod sync;
mod voice;
mod web;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use stvtf::watch;

use self::app::{App, Matches};
use self::housekeeping::{keep_house, live_ids};
use self::start::{keep_log, matches, prompt, relay_process, serve_until_stopped, warm, who_is_who};
use crate::cli::Options;

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

fn main() -> Result<()> {
    static STOP: AtomicBool = AtomicBool::new(false);
    let options = Options::parse_from(std::env::args().skip(1)).unwrap_or_else(|e| e.exit());
    let _told = report::start(if options.relay_daemon.is_some() { "relay" } else { "web" });
    watch::stop_on_ctrl_c(&STOP)?;
    keep_log(&options);
    if let Some(served) = relay_process(&options, &STOP) {
        return served;
    }
    let keys = std::env::var("STVTF_TOKEN_KEY").ok().map(|k| web::token::Keys::from_hex(&k).map(Arc::new)).transpose()?;
    let console_prompt = keys.is_none() && std::io::IsTerminal::is_terminal(&std::io::stdin());
    let matches = matches(&options, keys.as_ref(), &STOP)?;
    let store = if let Matches::Many(sessions) = &matches { sessions.store.clone() } else { None };
    let relays = match (&options.relays, &options.relay_socket) {
        (Some(_), _) | (_, Some(_)) if store.is_none() => bail!("cannot relay rooms without --sessions"),
        (Some((host, ports)), _) => Some(Arc::new(relays::pool::Pool::Local(relays::pool::Relays::new(host, ports.clone())))),
        (None, Some(socket)) => Some(Arc::new(relays::pool::Pool::Remote(relays::daemon::Remote::new(socket)))),
        (None, None) => None,
    };
    let voice_key = std::env::var("STVTF_VOICE_KEY").ok().filter(|k| !k.is_empty());
    let voice_who = who_is_who(&options, keys.as_ref(), voice_key.as_deref());
    let app = Arc::new(App {
        matches,
        cache: options.cache.clone(),
        backpack: options.backpack.clone(),
        maps: Mutex::default(),
        overviews: Mutex::new(()),
        base: options.base_path.clone(),
        keys,
        home: options.home.clone(),
        streams: Arc::default(),
        rooms: rooms::registry::Rooms::new(store, relays).with_maps(options.cache.clone()),
        voice: voice::VoiceSetup {
            key: voice_key.clone(),
            links: voice::links::Links::from_env(options.sessions.as_deref()),
            bot: std::env::var("STVTF_VOICE_BOT").ok().filter(|u| !u.is_empty()),
            mumble_changes: voice::mumble::Changes::default(),
            mumble_anywhere: std::env::var("STVTF_MUMBLE_ANYWHERE").is_ok_and(|v| v == "1"),
            who: voice_who,
        },
    });
    app.voice.who.keep_up();
    app.rooms.sweep(&live_ids(&app.matches));
    let mut keeper = None;
    if let Matches::Many(_) = app.matches {
        keeper = Some(keep_house(app.clone(), &STOP));
        if !options.no_warm {
            let warming = app.clone();
            std::thread::spawn(move || warm(&warming));
        }
        if console_prompt {
            let (console, url) = (app.clone(), format!("http://{}{}", options.listen, app.base));
            std::thread::spawn(move || prompt(&console, &url));
        }
    }
    serve_until_stopped(&options.listen, app, keeper, &STOP)
}
