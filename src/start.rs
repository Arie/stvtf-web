//! Starting up: the matches, maps, logs and relays the command line asks for, then serving.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use super::app::{App, Matches, is_name};
use super::sync::lock;
use crate::cli::{DEFAULT_TV_PASSWORD, Options};
use crate::maps::cache::{demostf_maps, map_geometry_gzip, overview, warm_maps};
use crate::matches::hub::{SharedHub, watch_into};
use crate::matches::replay::replay;
use crate::matches::sessions::Sessions;

/// Fetches demos.tf's overviews and builds the most played maps, so their first viewer does not
/// wait.
pub(super) fn warm(app: &App) {
    let known = {
        let _one_at_a_time = lock(&app.overviews);
        demostf_maps(&app.cache)
    };
    for map in known {
        let _one_at_a_time = lock(&app.overviews);
        if let Err(e) = overview(&app.cache, &format!("{map}.webp")) {
            tracing::info!("overview of {map}: {e}");
        }
    }
    for name in warm_maps() {
        let map_lock = app.map_lock(name);
        let _one_at_a_time = lock(&map_lock);
        // Gzipped too: browsers take that, and the JSON is made on the way.
        if let Err(e) = map_geometry_gzip(&app.cache, name, true) {
            tracing::info!("{name}: {e}");
        }
    }
}

/// Any number of matches: the hub's options, the matches a previous process left, the demos.
pub(super) fn many(options: &Options, keys: Option<&Arc<super::web::token::Keys>>) -> Result<Sessions> {
    let mut sessions = Sessions::new(options.public, options.max_matches, options.hz);
    if let Some(dir) = &options.sessions {
        sessions.store = Some(super::matches::sessions::Store { dir: dir.clone(), keys: keys.cloned() });
        // Before matches are picked up, so they record as the new ones do.
        sessions.demos = options.relays.is_some() || options.relay_socket.is_some();
        let restored = sessions.restore();
        if restored > 0 {
            tracing::info!("picked up {restored} running matches");
        }
    }
    if let Some(file) = &options.demo {
        pin_demo(&sessions, "demo", file.clone());
    }
    if let Some(dir) = &options.demos {
        for (id, file) in demo_files(dir)? {
            pin_demo(&sessions, &id, file);
        }
    }
    Ok(sessions)
}

/// A fixed match at `/w/<id>/` playing a saved `stvtf` output on a loop while someone watches.
pub(super) fn pin_demo(sessions: &Sessions, id: &str, file: PathBuf) {
    let name = id.to_string();
    sessions.pin(id, move |s| {
        s.set_file(file.clone());
        if let Err(e) = replay(&s.hub, &file, &|| s.watched()) {
            eprintln!("{name}: {e:#}");
        }
    });
}

/// The recordings in a folder of demos (`NAME.jsonl` or `NAME.jsonl.gz`, NAME of letters, digits,
/// `_` and `-`), by NAME, sorted.
pub(super) fn demo_files(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot read --demos {}", dir.display()))? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(id) = name.strip_suffix(".jsonl.gz").or_else(|| name.strip_suffix(".jsonl")) else { continue };
        if is_name(id, b"_-") {
            found.push((id.to_string(), path));
        }
    }
    found.sort();
    Ok(found)
}

/// A connect string pasted in the terminal works like one pasted on the page.
pub(super) fn prompt(app: &App, url: &str) {
    let Matches::Many(sessions) = &app.matches else { return };
    let stdin = std::io::stdin();
    loop {
        eprint!("Paste a SourceTV connect string (connect HOST:PORT; password \"...\"), or open {url}/ : ");
        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line.trim().is_empty() {
            continue;
        }
        match sessions.open(&line, None, None) {
            Ok(m) => eprintln!("watching: {url}/w/{}/  (overlay: {url}/w/{}/overlay)", m.id, m.id),
            Err(e) => eprintln!("{e:#}"),
        }
    }
}

/// What the options ask to watch: a saved match, one server, or any number of them.
pub(super) fn matches(options: &Options, keys: Option<&Arc<super::web::token::Keys>>, stop: &'static AtomicBool) -> Result<Matches> {
    let hub = SharedHub::default();
    let fed = hub.clone();
    if let Some(file) = options.replay.clone() {
        std::thread::spawn(move || {
            if let Err(e) = replay(&fed, &file, &|| true) {
                eprintln!("replay: {e}");
            }
        });
    } else if let Some((target, password)) = options.connect.clone() {
        let password = options.password.clone().or(password).unwrap_or_else(|| DEFAULT_TV_PASSWORD.to_string());
        let hz = options.hz;
        std::thread::spawn(move || {
            if let Err(e) = watch_into(&fed, &target, &password, hz, None, stop) {
                tracing::info!("{e}");
            }
            if stop.load(Ordering::Relaxed) {
                std::process::exit(0);
            }
        });
    } else {
        return Ok(Matches::Many(many(options, keys)?));
    }
    Ok(Matches::One(hub))
}

/// Who is who in a team's voice channel: kept in the store, the same as the stv.tf that holds
/// it for all regions (`STVTF_VOICE_HOME`; none on that one itself), and with the sites asked
/// who a Discord user is (`STVTF_LINK_SITES`, each as `--home`; else the one this belongs to),
/// where voice is taken in and the sites share a key.
pub(super) fn who_is_who(
    options: &Options,
    keys: Option<&Arc<super::web::token::Keys>>,
    voice_key: Option<&str>,
) -> super::voice::who::WhoIsWho {
    let home = std::env::var("STVTF_VOICE_HOME").ok().filter(|u| !u.is_empty());
    let sites = std::env::var("STVTF_LINK_SITES").ok().filter(|l| !l.is_empty()).or_else(|| options.home.clone()).unwrap_or_default();
    let sites: Vec<_> = match (keys, voice_key) {
        (Some(keys), Some(_)) => {
            sites.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| super::web::voice::who_sync::site(s, keys.clone())).collect()
        }
        _ => Vec::new(),
    };
    super::voice::who::WhoIsWho::new(options.sessions.as_deref(), home.as_deref().zip(voice_key), sites)
}

/// The cameras casters placed on maps: kept in the store, the same as the stv.tf that holds who
/// is who (`STVTF_VOICE_HOME`).
pub(super) fn cameras(options: &Options, voice_key: Option<&str>) -> super::cameras::Cameras {
    let home = std::env::var("STVTF_VOICE_HOME").ok().filter(|u| !u.is_empty());
    super::cameras::Cameras::new(options.sessions.as_deref(), home.as_deref().zip(voice_key))
}

/// The sites asked how their league knows players: those asked who a Discord user is.
pub(super) fn leagues(options: &Options, keys: Option<&Arc<super::web::token::Keys>>) -> super::leagues::Leagues {
    let sites = std::env::var("STVTF_LINK_SITES").ok().filter(|l| !l.is_empty()).or_else(|| options.home.clone()).unwrap_or_default();
    let Some(keys) = keys else { return super::leagues::Leagues::default() };
    super::leagues::Leagues::new(
        sites.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| super::leagues::site(s, keys.clone())).collect(),
    )
}

/// Logs to stderr and, with `--logs`, to files per process.
pub(super) fn keep_log(options: &Options) {
    super::log::start(options.logs.as_deref(), if options.relay_daemon.is_some() { "relay" } else { "web" });
}

/// The relay process, if `options` ask for it: only the relays (the rest are the image's for the
/// web), until told to `stop`. Or a question for the one running.
pub(super) fn relay_process(options: &Options, stop: &'static AtomicBool) -> Option<Result<()>> {
    if let Some(socket) = &options.relay_clients {
        return Some(super::relays::daemon::clients(socket).map(|answer| println!("{answer}")));
    }
    let (Some(socket), Some((host, ports))) = (&options.relay_daemon, &options.relays) else { return None };
    tracing::info!("relays on {host}, ports {}-{}", ports.start(), ports.end());
    Some(super::relays::daemon::serve(socket, &super::relays::pool::Relays::new(host, ports.clone()), super::relays::daemon::LEASE, stop))
}

/// Serves until stopped, then lets `keeper` save for the next process.
pub(super) fn serve_until_stopped(
    listen: &str,
    app: Arc<App>,
    keeper: Option<std::thread::JoinHandle<()>>,
    stop: &'static AtomicBool,
) -> Result<()> {
    super::web::server::run(listen, app, stop)?;
    if let Some(keeper) = keeper {
        let _ = keeper.join();
    }
    Ok(())
}
