//! Demos uploaded here, to watch with a team: a `SourceTV` demo, or a player's own (which has
//! only what that player's game was sent, and is relayed to TF2 as that player), as it is or
//! packed (see [`super::unpack`]). Sent in parts, a proxy in front taking only so much in one
//! request:
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

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use super::asked::{Visit, answer};
use super::client::counted;
use super::{Answered, Early, Shared};
use crate::matches::sessions::{self, DEMO_PREFIX, Store, demo_dem, demo_info, demo_output};
use crate::sync::lock;

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

/// `GET /upload`: the form is on the front page.
pub(super) async fn form(State(app): State<Shared>) -> Redirect {
    Redirect::to(&format!("{}/", app.base))
}

/// The upload store and the uploader (none if unlimited); refused without a store, or when the
/// visitor has to sign in first.
fn uploading(v: &Visit) -> Result<(&Store, Option<String>), Early> {
    let Some(store) = &v.sessions()?.store else {
        return Err((StatusCode::NOT_FOUND, "No demos are kept here").into_response().into());
    };
    if v.app.keys.is_some() && v.who.user.is_none() {
        return Err((StatusCode::FORBIDDEN, "Sign in first").into_response().into());
    }
    Ok((store, counted(v.who.user.as_ref(), None, v.client)))
}

/// An upload's id in a path: one of ours, or answered early that there is none.
fn upload_id(id: &str) -> Result<&str, Early> {
    if sessions::is_id(id) { Ok(id) } else { Err(StatusCode::NOT_FOUND.into_response().into()) }
}

fn said(done: Result<serde_json::Value>) -> Response {
    match done {
        Ok(said) => Json(said).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    }
}

/// `POST /upload`.
pub(super) async fn begun(v: Visit, Json(upload): Json<Begin>) -> Answered {
    answer(move || {
        let (store, by) = uploading(&v)?;
        Ok(said(begin(&upload, store, by.as_deref())))
    })
    .await
}

/// `?at=`: where a part goes on.
#[derive(serde::Deserialize)]
pub(super) struct At {
    at: Option<String>,
}

/// `PUT /upload/<id>`.
pub(super) async fn part(v: Visit, Path(id): Path<String>, Query(at): Query<At>, body: Bytes) -> Answered {
    answer(move || {
        let (store, _) = uploading(&v)?;
        put_part(at.at.as_deref().and_then(|a| a.parse().ok()), &body, store, upload_id(&id)?)
    })
    .await
}

/// `POST /upload/<id>`.
pub(super) async fn finished(v: Visit, Path(id): Path<String>, said_of_it: Option<Json<Finish>>) -> Answered {
    answer(move || {
        let (store, by) = uploading(&v)?;
        let name = said_of_it.map_or_default(|Json(said)| said);
        Ok(said(finish(&v, store, upload_id(&id)?, by.as_deref(), &name)))
    })
    .await
}

#[derive(serde::Deserialize)]
pub(super) struct Begin {
    size: u64,
}

/// Each person's upload under way: beginning another drops it.
static UNDER_WAY: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub(super) fn begin(upload: &Begin, store: &Store, by: Option<&str>) -> Result<serde_json::Value> {
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
    let last = by.and_then(|by| lock(&UNDER_WAY).get_or_insert_default().insert(by.to_string(), id.clone()));
    if let Some(last) = last {
        let _one = lock(&ONE_AT_A_TIME);
        let _ = std::fs::remove_file(store.dir.join(part_name(&last)));
    }
    Ok(serde_json::json!({ "id": id, "part": MAX_PART }))
}

/// `PUT /upload/<id>?at=`: the body goes on the end of what is there, if `at` is where that is.
fn put_part(at: Option<u64>, body: &[u8], store: &Store, id: &str) -> Answered {
    let _one = lock(&ONE_AT_A_TIME);
    let path = store.dir.join(part_name(id));
    let Ok(had) = std::fs::metadata(&path).map(|m| m.len()) else { return Ok((StatusCode::NOT_FOUND, "No such upload").into_response()) };
    if had + body.len() as u64 > MAX_DEMO {
        return Ok((StatusCode::PAYLOAD_TOO_LARGE, "Too big").into_response());
    }
    if at != Some(had) {
        return Ok((StatusCode::CONFLICT, had.to_string()).into_response());
    }
    std::fs::File::options().append(true).open(&path)?.write_all(body)?;
    Ok((had + body.len() as u64).to_string().into_response())
}

/// Held while a part is added or an upload finished: what was checked still holds.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

#[derive(Default, serde::Deserialize)]
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
fn finish(v: &Visit, store: &Store, id: &str, by: Option<&str>, said: &Finish) -> Result<serde_json::Value> {
    let (app, sessions, user) = (&v.app, v.sessions()?, &v.who.user);
    let part = store.dir.join(part_name(id));
    let demo = {
        let _one = lock(&ONE_AT_A_TIME);
        let kept = keep(store, &part, &said.name, by, user.as_ref().map(|u| u.uid.as_str()));
        if kept.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        kept?
    };
    lock(&UNDER_WAY).get_or_insert_default().retain(|_, theirs| theirs != id);
    let m = sessions.open_upload_as(by, demo).context("Not kept")??;
    tracing::info!(
        "demo {demo} uploaded by {}, {}",
        by.unwrap_or("-"),
        said.name.chars().filter(|c| !c.is_control()).take(80).collect::<String>()
    );
    Ok(serde_json::json!({ "page": format!("{}/w/{}/", app.base, m.id) }))
}

/// Keeps the upload at `part` as a demo of its own, counting against `by`. Returns its number.
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
    // Their previous upload goes.
    for (old, _) in by.map_or_default(|by| kept(store, |info| info["by"].as_str() == Some(by))) {
        forget(store, old);
    }
    std::fs::rename(part, store.dir.join(demo_dem(demo)))?;
    let name: String = name.chars().filter(|c| !c.is_control()).take(100).collect();
    let info = serde_json::json!({ "by": by, "uid": uid, "pov": pov, "name": name, "map": header.map });
    std::fs::write(store.dir.join(demo_info(demo)), info.to_string())?;
    Ok(demo)
}
