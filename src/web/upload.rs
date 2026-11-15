//! Demos uploaded here, to watch with a team: a `SourceTV` demo, or a player's own (which has
//! only what that player's game was sent, and is not relayed to TF2), as it is or packed (see
//! [`super::unpack`]). Sent in parts, a proxy in front taking only so much in one request:
//!
//! - `GET /upload`: to the front page, which has the form.
//! - `POST /upload` `{"size"}`: begins one; `{"id", "part"}`, the most bytes a part may be.
//! - `PUT /upload/<id>?at=<bytes so far>`: the next part; `409` with how many bytes there are
//!   when that is not where it goes on.
//! - `POST /upload/<id>` `{"name"}`: all of it is there; `{"page"}`, where it plays:
//!   `/w/demo/<its id>/`, or a player's own at `/w/pov/<its id>/`.
//!
//! By whoever is signed in (where nobody signs in, anyone). Kept as a demos.tf demo is, and of
//! one person only one: another takes its place.

// Handlers have the shape the routes' adapters take (`server::to` and the like), whether or not
// one can fail or needs to own what was posted.
#![allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)]

use super::client::counted;
use super::routes::Asked;
use super::visitor::who;
use super::{Answered, Early};
use super::{JSON, Request, TEXT, refuse, respond};
use crate::App;
use crate::matches::sessions::{self, DEMO_PREFIX, Sessions, Store, demo_dem, demo_info, demo_output};
use anyhow::{Context, Result, bail};
use axum::body::Bytes;
use axum::http::StatusCode;
use std::io::{Read, Write};
use std::net::IpAddr;

/// The biggest demo (sent, and unpacked), and part of one: under what Cloudflare lets through.
pub(super) const MAX_DEMO: u64 = 512 << 20;
pub(super) const MAX_PART: usize = 32 << 20;
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

/// `GET /upload`: the form is on the front page.
pub(super) fn form(a: &Asked) -> Answered {
    Ok(a.app.redirect(StatusCode::FOUND, "/"))
}

/// Where uploads are kept, and who uploads. Answered early when nothing is, or they may not.
fn uploading(a: &Asked) -> Result<(&Store, Option<String>), Early> {
    let Some(store) = &a.sessions()?.store else {
        return Err(respond(StatusCode::NOT_FOUND, TEXT, b"No demos are kept here").into());
    };
    match uploader(&a.app, &a.request, a.client) {
        Ok(by) => Ok((store, by)),
        Err(e) => Err(refuse(StatusCode::FORBIDDEN, &e).into()),
    }
}

/// The upload a request under `/upload/` is of.
fn upload_id(a: &Asked) -> Option<String> {
    a.request.path.strip_prefix("/upload/").filter(|id| sessions::is_id(id)).map(str::to_string)
}

fn said(done: Result<serde_json::Value>) -> Answered {
    Ok(match done {
        Ok(said) => respond(StatusCode::OK, JSON, said.to_string().as_bytes()),
        Err(e) => refuse(StatusCode::BAD_REQUEST, &e),
    })
}

/// `POST /upload`.
pub(super) fn begun(a: &Asked, upload: Begin) -> Answered {
    let (store, _) = uploading(a)?;
    said(begin(&upload, store))
}

/// `PUT /upload/<id>`.
pub(super) fn part(a: &Asked, body: Bytes) -> Answered {
    let (store, _) = uploading(a)?;
    let Some(id) = upload_id(a) else { return Ok(respond(StatusCode::NOT_FOUND, TEXT, b"not found")) };
    put_part(&a.request, &body, store, &id)
}

/// `POST /upload/<id>`.
pub(super) fn finished(a: &Asked, said_of_it: Option<Finish>) -> Answered {
    let (store, by) = uploading(a)?;
    let Some(id) = upload_id(a) else { return Ok(respond(StatusCode::NOT_FOUND, TEXT, b"not found")) };
    said(finish(&a.app, a.sessions()?, &a.request, store, &id, by.as_deref(), &said_of_it.unwrap_or_default()))
}

#[derive(serde::Deserialize)]
pub(super) struct Begin {
    size: u64,
}

fn begin(upload: &Begin, store: &Store) -> Result<serde_json::Value> {
    let size = upload.size;
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
fn put_part(request: &Request, body: &[u8], store: &Store, id: &str) -> Answered {
    let path = store.dir.join(part_name(id));
    let Ok(had) = std::fs::metadata(&path).map(|m| m.len()) else { return Ok(respond(StatusCode::NOT_FOUND, TEXT, b"No such upload")) };
    let at = request.query.get("at").and_then(|a| a.parse::<u64>().ok());
    if had + body.len() as u64 > MAX_DEMO {
        return Ok(respond(StatusCode::PAYLOAD_TOO_LARGE, TEXT, b"Too big"));
    }
    if at != Some(had) {
        return Ok(respond(StatusCode::CONFLICT, TEXT, had.to_string().as_bytes()));
    }
    std::fs::File::options().append(true).open(&path)?.write_all(body)?;
    Ok(respond(StatusCode::OK, TEXT, (had + body.len() as u64).to_string().as_bytes()))
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(super) struct Finish {
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

/// The uploads kept whose info is `theirs`: each one's number and info.
fn kept(store: &Store, theirs: impl Fn(&serde_json::Value) -> bool) -> Vec<(u64, serde_json::Value)> {
    std::fs::read_dir(&store.dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let demo: u64 = name.strip_prefix(DEMO_PREFIX)?.strip_suffix(".up")?.parse().ok()?;
            let info: serde_json::Value = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            theirs(&info).then_some((demo, info))
        })
        .collect()
}

/// The uploads kept of user `uid`, or counting against `by`: each one's number and name.
pub(super) fn kept_by(store: &Store, uid: Option<&str>, by: Option<&str>) -> Vec<(u64, String)> {
    let is = |v: &serde_json::Value, who: Option<&str>| who.is_some() && v.as_str() == who;
    let mut theirs: Vec<(u64, String)> = kept(store, |info| is(&info["uid"], uid) || is(&info["by"], by))
        .into_iter()
        .map(|(demo, info)| (demo, info["name"].as_str().filter(|n| !n.is_empty()).or(info["map"].as_str()).unwrap_or("demo").to_string()))
        .collect();
    theirs.sort();
    theirs
}

fn forget(store: &Store, demo: u64) {
    for name in [demo_dem(demo), demo_output(demo), demo_info(demo)] {
        let _ = std::fs::remove_file(store.dir.join(name));
    }
}

/// `POST /upload/<id>`: the demo is whole. It is kept, and plays.
fn finish(
    app: &App,
    sessions: &Sessions,
    request: &Request,
    store: &Store,
    id: &str,
    by: Option<&str>,
    said: &Finish,
) -> Result<serde_json::Value> {
    let part = store.dir.join(part_name(id));
    let user = who(app, request).user;
    let kept = keep(store, &part, &said.name, by, user.as_ref().map(|u| u.uid.as_str()));
    if kept.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    let demo = kept?;
    let m = sessions.open_upload_as(by, demo).context("Not kept")??;
    log!("demo {demo} uploaded by {}, {}", by.unwrap_or("-"), said.name.chars().filter(|c| !c.is_control()).take(80).collect::<String>());
    Ok(serde_json::json!({ "page": format!("{}/w/{}/", app.base, m.id) }))
}

/// The upload at `part` kept as a demo of its own, `by`'s: its number.
pub(super) fn keep(store: &Store, part: &std::path::Path, name: &str, by: Option<&str>, uid: Option<&str>) -> Result<u64> {
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
    for (old, _) in by.map_or_default(|by| kept(store, |info| info["by"].as_str() == Some(by))) {
        forget(store, old);
    }
    std::fs::rename(part, store.dir.join(demo_dem(demo)))?;
    let name: String = name.chars().filter(|c| !c.is_control()).take(100).collect();
    let info = serde_json::json!({ "by": by, "uid": uid, "pov": pov, "name": name, "map": header.map });
    std::fs::write(store.dir.join(demo_info(demo)), info.to_string())?;
    Ok(demo)
}
