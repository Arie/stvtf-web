//! A match's voice routes, the same under its page and under the page of a room on it.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use super::super::asked::{MatchAt, Since, Visit, answer};
use super::super::{Answered, Early, Shared};
use super::incoming::VoicePost;
use super::mumble::Ask;
use super::who::{Match, Report};
use crate::matches::sessions::Session;
use crate::voice::who::RegionEntry;

/// The match a voice route under `/w/` or `/r/` is for: a room's is the live one it is on, if any.
pub(super) fn of(v: &Visit, at: &MatchAt) -> Result<Option<Arc<Session>>, Early> {
    if let Some(room) = v.path.strip_prefix("/r/").and_then(|p| p.split('/').next()) {
        let room = super::super::rooms::found(v, room)?;
        return Ok(super::super::rooms::live(&v.app, v.sessions()?, &room));
    }
    Ok(Some(super::super::matches::found(v, at)?))
}

/// `POST <page>voice-mumble`.
pub(in crate::web) async fn mumble(v: Visit, at: MatchAt, Json(ask): Json<Ask>) -> Answered {
    answer(move || Ok(super::mumble::post_mumble(&v.app, of(&v, &at)?.as_deref(), ask, &v.who))).await
}

/// `POST <page>voice-status`.
pub(in crate::web) async fn status(v: Visit, at: MatchAt, Json(said): Json<Report>) -> Answered {
    answer(move || Ok(super::who::post_voice_status(&v.app, of(&v, &at)?.as_deref(), v.bearer.as_deref(), said))).await
}

/// `GET <page>voice-comms`.
pub(in crate::web) async fn comms(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(super::who::get_voice_comms(&v.app, of(&v, &at)?.as_deref(), &v.who))).await
}

/// `GET <page>voices`.
pub(in crate::web) async fn voices(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(super::who::get_voices(&v.app, of(&v, &at)?.as_deref(), &v.who))).await
}

/// `POST <page>voice-match`.
pub(in crate::web) async fn matched(v: Visit, at: MatchAt, Json(said): Json<Match>) -> Answered {
    answer(move || Ok(super::who::post_voice_match(&v.app, of(&v, &at)?.as_deref(), &said, &v.who))).await
}

/// `GET <page>voice-invite`.
pub(in crate::web) async fn invite(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(super::incoming::get_voice_invite(&v.app, of(&v, &at)?.as_deref(), &v.who))).await
}

/// `POST <page>voice`.
pub(in crate::web) async fn voice(v: Visit, at: MatchAt, Json(said): Json<VoicePost>) -> Answered {
    answer(move || {
        Ok(match of(&v, &at)? {
            Some(m) => super::incoming::post_voice(&v.app, &m, v.bearer.as_deref(), said),
            None => (StatusCode::NOT_FOUND, "not a live match").into_response(),
        })
    })
    .await
}

/// `/voice/link`: off to Discord.
pub(in crate::web) async fn link(State(app): State<Shared>) -> impl IntoResponse {
    super::link::link(&app)
}

/// What Discord sends a browser back with.
#[derive(Deserialize)]
pub(in crate::web) struct Linked {
    #[serde(default)]
    pub(super) state: String,
    #[serde(default)]
    pub(super) code: String,
}

/// `/voice/linked`: back from Discord.
pub(in crate::web) async fn linked(v: Visit, jar: CookieJar, Query(q): Query<Linked>) -> Answered {
    answer(move || {
        let began = jar.get(super::link::STATE_COOKIE).map(|c| c.value().to_string());
        Ok(super::link::linked(&v.app, &q.state, &q.code, began.as_deref()))
    })
    .await
}

/// `?discord=<id>`.
#[derive(Deserialize)]
pub(in crate::web) struct Discord {
    pub(super) discord: Option<String>,
}

/// `/voice/who`: the Steam id linked to a Discord user, to the bot.
pub(in crate::web) async fn who(v: Visit, Query(q): Query<Discord>) -> Answered {
    answer(move || Ok(super::link::who(&v.app, v.bearer.as_deref(), q.discord.as_deref()))).await
}

/// `GET /voice/mumble`: the Mumble channels the bot is wanted in; it waits for a change.
pub(in crate::web) async fn mumble_jobs(v: Visit, Query(q): Query<Since>) -> Answered {
    answer(move || Ok(super::mumble::get_mumble_jobs(&v.app, v.sessions()?, v.bearer.as_deref(), q.since()))).await
}

/// `GET /voice/known`.
pub(in crate::web) async fn known(v: Visit, Query(q): Query<Since>) -> Answered {
    answer(move || Ok(super::who_sync::get_known(&v.app, v.bearer.as_deref(), q.since().unwrap_or(0)))).await
}

/// `POST /voice/known`.
pub(in crate::web) async fn told(v: Visit, Json(said): Json<RegionEntry>) -> Answered {
    answer(move || Ok(super::who_sync::post_known(&v.app, v.bearer.as_deref(), &said))).await
}
