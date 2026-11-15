//! Demos uploaded here, to watch with a team: a `SourceTV` demo, or a player's own (which has
//! only what that player's game was sent, and is not relayed to TF2), as it is or packed (see
//! [`super::unpack`]). Sent in parts, a proxy in front taking only so much in one request:
//!
//! - `GET /upload`: the page.
//! - `POST /upload` `{"size"}`: begins one; `{"id", "part"}`, the most bytes a part may be.
//! - `PUT /upload/<id>?at=<bytes so far>`: the next part; `409` with how many bytes there are
//!   when that is not where it goes on.
//! - `POST /upload/<id>` `{"name"}`: all of it is there; `{"page"}`, where it plays:
//!   `/w/demo/<its id>/`, or a player's own at `/w/pov/<its id>/`.
//!
//! By whoever is signed in (where nobody signs in, anyone). Kept as a demos.tf demo is, and of
//! one person only one: another takes its place.

use super::client::counted;
use super::visitor::who;
use super::{HTML, JSON, Request, TEXT, json_body, param, refuse, respond};
use crate::App;
use crate::matches::sessions::{self, DEMO_PREFIX, Sessions, Store, demo_dem, demo_info, demo_output};
use anyhow::{Context, Result, bail};
use std::io::{BufReader, Read, Take, Write};
use std::net::{IpAddr, TcpStream};

const PAGE: &str = include_str!("../../app/pages/upload.html");
/// The biggest demo (sent, and unpacked), and part of one: under what Cloudflare lets through.
const MAX_DEMO: u64 = 512 << 20;
const MAX_PART: usize = 32 << 20;
/// What a `SourceTV` demo's header names as its client.
const STV_CLIENT: &str = "SourceTV Demo";

/// A demo on its way in, by its id: beside the others until it is whole (swept like a demo half
/// fetched).
fn part_name(id: &str) -> String {
    format!("{DEMO_PREFIX}up-{id}.part")
}

/// Who uploads: `Err` with why not; none for someone no limit holds for.
fn uploader(app: &App, request: &Request, client: IpAddr) -> Result<Option<String>> {
    let user = who(app, request).user;
    if app.keys.is_some() && user.is_none() {
        bail!("Sign in first");
    }
    Ok(counted(user.as_ref(), None, client))
}

/// `/upload` and what is under it.
pub(super) fn serve_upload(
    s: &mut TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    sessions: &Sessions,
    request: &Request,
    client: IpAddr,
) -> std::io::Result<()> {
    let Some(store) = &sessions.store else { return respond(s, "404 Not Found", TEXT, b"No demos are kept here") };
    let id = request.path.strip_prefix("/upload/").filter(|id| sessions::is_id(id));
    if let ("/upload", "GET") = (request.path.as_str(), request.method.as_str()) {
        let signed = app.keys.is_none() || who(app, request).user.is_some();
        let page = PAGE.replace("{signed}", if signed { "1" } else { "" }).replace("{max}", &(MAX_DEMO >> 20).to_string());
        return respond(s, "200 OK", HTML, &app.page(&page));
    }
    let by = match uploader(app, request, client) {
        Ok(by) => by,
        Err(e) => return refuse(s, "403 Forbidden", &e),
    };
    let done = match (request.path.as_str(), request.method.as_str(), id) {
        ("/upload", "POST", _) => begin(reader, request, store),
        (_, "PUT", Some(id)) => return put_part(s, reader, request, store, id),
        (_, "POST", Some(id)) => finish(reader, app, sessions, request, store, id, by.as_deref()),
        _ => return respond(s, "404 Not Found", TEXT, b"not found"),
    };
    match done {
        Ok(said) => respond(s, "200 OK", JSON, said.to_string().as_bytes()),
        Err(e) => refuse(s, "400 Bad Request", &e),
    }
}

#[derive(serde::Deserialize)]
struct Begin {
    size: u64,
}

fn begin(reader: &mut BufReader<Take<TcpStream>>, request: &Request, store: &Store) -> Result<serde_json::Value> {
    let Begin { size } = json_body(reader, request)?;
    if size > MAX_DEMO {
        bail!("Over {} MB", MAX_DEMO >> 20);
    }
    if size < stvtf::demo::HEADER_LEN as u64 {
        bail!("Not a demo");
    }
    let id = sessions::random_id();
    std::fs::create_dir_all(&store.dir)?;
    std::fs::File::create(store.dir.join(part_name(&id)))?;
    Ok(serde_json::json!({ "id": id, "part": MAX_PART }))
}

/// `PUT /upload/<id>?at=`: the body goes on the end of what is there, if `at` is where that is.
fn put_part(s: &mut TcpStream, reader: &mut BufReader<Take<TcpStream>>, request: &Request, store: &Store, id: &str) -> std::io::Result<()> {
    let path = store.dir.join(part_name(id));
    let Ok(had) = std::fs::metadata(&path).map(|m| m.len()) else { return respond(s, "404 Not Found", TEXT, b"No such upload") };
    let at = param(&request.query, "at").and_then(|a| a.parse::<u64>().ok());
    if request.length > MAX_PART || had + request.length as u64 > MAX_DEMO {
        return respond(s, "413 Content Too Large", TEXT, b"Too big");
    }
    if at != Some(had) {
        return respond(s, "409 Conflict", TEXT, had.to_string().as_bytes());
    }
    // The body has an allowance of its own: the headers may have used up most of theirs.
    let buffered = reader.buffer().len() as u64;
    reader.get_mut().set_limit((request.length as u64).saturating_sub(buffered));
    let mut body = vec![0; request.length];
    reader.read_exact(&mut body)?;
    std::fs::File::options().append(true).open(&path)?.write_all(&body)?;
    respond(s, "200 OK", TEXT, (had + request.length as u64).to_string().as_bytes())
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Finish {
    name: String,
}

/// The number an uploaded demo plays under: the same for the same file, whoever sends it.
fn number_of(path: &std::path::Path, kind: sessions::Own) -> Result<u64> {
    let mut file = std::fs::File::open(path)?;
    let (mut hash, mut len, mut chunk) = (0xcbf2_9ce4_8422_2325_u64, 0u64, vec![0u8; 1 << 20]);
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        len += n as u64;
        hash = chunk[..n].iter().fold(hash, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
    }
    Ok(sessions::own_demo(kind, &format!("up-{hash:016x}-{len}")))
}

/// What is kept of `by`'s uploads: each one's number.
fn kept_of(store: &Store, by: &str) -> Vec<u64> {
    std::fs::read_dir(&store.dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let demo: u64 = name.strip_prefix(DEMO_PREFIX)?.strip_suffix(".up")?.parse().ok()?;
            let info: serde_json::Value = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            (info["by"].as_str() == Some(by)).then_some(demo)
        })
        .collect()
}

fn forget(store: &Store, demo: u64) {
    for name in [demo_dem(demo), demo_output(demo), demo_info(demo)] {
        let _ = std::fs::remove_file(store.dir.join(name));
    }
}

/// `POST /upload/<id>`: the demo is whole. It is kept, and plays.
fn finish(
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    sessions: &Sessions,
    request: &Request,
    store: &Store,
    id: &str,
    by: Option<&str>,
) -> Result<serde_json::Value> {
    let said: Finish = if request.length > 0 { json_body(reader, request)? } else { Finish::default() };
    let part = store.dir.join(part_name(id));
    let kept = keep(store, &part, &said.name, by);
    if kept.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    let demo = kept?;
    let m = sessions.open_upload_as(by, demo).context("Not kept")??;
    log!("demo {demo} uploaded by {}, {}", by.unwrap_or("-"), said.name.chars().filter(|c| !c.is_control()).take(80).collect::<String>());
    Ok(serde_json::json!({ "page": format!("{}/w/{}/", app.base, m.id) }))
}

/// The upload at `part` kept as a demo of its own, `by`'s: its number.
pub(super) fn keep(store: &Store, part: &std::path::Path, name: &str, by: Option<&str>) -> Result<u64> {
    super::unpack::unpack(part, MAX_DEMO)?;
    let mut head = vec![0u8; stvtf::demo::HEADER_LEN];
    std::fs::File::open(part).context("No such upload")?.read_exact(&mut head).context("Not a demo")?;
    let header = stvtf::demo::Header::parse(&head).ok().filter(|h| h.game == "tf").context("Not a TF2 demo")?;
    let pov = header.client != STV_CLIENT;
    let demo = number_of(part, if pov { sessions::Own::Pov } else { sessions::Own::Demo })?;
    let known = store.has(&demo_info(demo)) && (store.has(&demo_output(demo)) || store.has(&demo_dem(demo)));
    if known {
        // Sent before, by them or another: the one kept plays.
        std::fs::remove_file(part)?;
        return Ok(demo);
    }
    // Their last goes.
    for old in by.map_or_default(|by| kept_of(store, by)) {
        forget(store, old);
    }
    std::fs::rename(part, store.dir.join(demo_dem(demo)))?;
    let name: String = name.chars().filter(|c| !c.is_control()).take(100).collect();
    let info = serde_json::json!({ "by": by, "pov": pov, "name": name, "map": header.map });
    std::fs::write(store.dir.join(demo_info(demo)), info.to_string())?;
    Ok(demo)
}
