//! The cameras casters placed on maps (`crate::cameras`): the routes a page uses, and those
//! another stv.tf uses.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::answer::{Answered, done};
use super::asked::{Visit, answer};
use super::voice::is_key;
use crate::app::is_name;
use crate::cameras::Change;

#[derive(serde::Deserialize)]
pub(super) struct Since {
    #[serde(default)]
    since: u64,
}

/// Whether the request carries the bot's key, as one from another stv.tf does.
fn keyed(v: &Visit) -> bool {
    v.app.voice.key.as_deref().is_some_and(|key| is_key(key, v.bearer.as_deref()))
}

/// `GET /cameras/<map>`: the map's cameras, for anyone.
pub(super) async fn on_map(v: Visit, Path(map): Path<String>) -> Answered {
    answer(move || {
        if !is_name(&map, b"_") {
            return Ok(StatusCode::NOT_FOUND.into_response());
        }
        Ok(Json(v.app.cameras.on(&map)).into_response())
    })
    .await
}

/// `POST /cameras/<map>`: a change to them, by the site's streamers (anyone, where nobody signs
/// in) and by another stv.tf passing one on.
pub(super) async fn change(v: Visit, Path(map): Path<String>, Json(change): Json<Change>) -> Answered {
    answer(move || {
        let may = v.app.keys.is_none() || v.who.user.as_ref().is_some_and(|u| u.streamer) || keyed(&v);
        if !may {
            return Ok((StatusCode::FORBIDDEN, "Only the site's streamers place cameras.").into_response());
        }
        Ok(done(v.app.cameras.change(&map, &change)))
    })
    .await
}

/// `GET /cameras?since=<version>`, for another stv.tf: all of them, or `204` when unchanged since.
pub(super) async fn all(v: Visit, Query(q): Query<Since>) -> Answered {
    answer(move || {
        if !keyed(&v) {
            return Ok(StatusCode::UNAUTHORIZED.into_response());
        }
        Ok(match v.app.cameras.since(q.since) {
            Some(kept) => Json(kept).into_response(),
            None => StatusCode::NO_CONTENT.into_response(),
        })
    })
    .await
}

/// `GET /sight/<map>`: what blocks a camera's sight there, built on a miss for a live map.
pub(super) async fn sight(v: Visit, Path(map): Path<String>) -> Response {
    let built = answer(move || Ok(super::assets::sight(&v.app, &map, v.gzip))).await;
    built.unwrap_or_else(IntoResponse::into_response)
}
