//! A match's voice routes, the same on its page and on a room's on it.

use super::asked::{MatchAt, Since, Visit, answer};
use super::voice_in::VoicePost;
use super::voice_mumble::Ask;
use super::voice_who::{Match, Report};
use super::{Answered, Early, Shared, voice_in, voice_link, voice_mumble, voice_who, voice_who_sync};
use crate::matches::sessions::Session;
use crate::voice::who::RegionEntry;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use std::sync::Arc;

/// The match a voice route under `/w/` or `/r/` is for: a room's is the live one it is on, if any.
fn of(v: &Visit, at: &MatchAt) -> Result<Option<Arc<Session>>, Early> {
    if let Some(room) = v.path.strip_prefix("/r/").and_then(|p| p.split('/').next()) {
        let room = super::rooms::found(v, room)?;
        return Ok(super::rooms::live(&v.app, v.sessions()?, &room));
    }
    Ok(Some(super::matches::found(v, at)?))
}

/// `POST <page>voice-mumble`.
pub(super) async fn mumble(v: Visit, at: MatchAt, Json(ask): Json<Ask>) -> Answered {
    answer(move || Ok(voice_mumble::post_mumble(&v.app, of(&v, &at)?.as_deref(), ask, &v.who))).await
}

/// `POST <page>voice-status`.
pub(super) async fn status(v: Visit, at: MatchAt, Json(said): Json<Report>) -> Answered {
    answer(move || Ok(voice_who::post_voice_status(&v.app, of(&v, &at)?.as_deref(), v.bearer.as_deref(), said))).await
}

/// `GET <page>voice-comms`.
pub(super) async fn comms(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(voice_who::get_voice_comms(&v.app, of(&v, &at)?.as_deref(), &v.who))).await
}

/// `GET <page>voices`.
pub(super) async fn voices(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(voice_who::get_voices(&v.app, of(&v, &at)?.as_deref(), &v.who))).await
}

/// `POST <page>voice-match`.
pub(super) async fn matched(v: Visit, at: MatchAt, Json(said): Json<Match>) -> Answered {
    answer(move || Ok(voice_who::post_voice_match(&v.app, of(&v, &at)?.as_deref(), &said, &v.who))).await
}

/// `GET <page>voice-invite`.
pub(super) async fn invite(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(voice_in::get_voice_invite(&v.app, of(&v, &at)?.as_deref(), &v.who))).await
}

/// `POST <page>voice`.
pub(super) async fn voice(v: Visit, at: MatchAt, Json(said): Json<VoicePost>) -> Answered {
    answer(move || {
        Ok(match of(&v, &at)? {
            Some(m) => voice_in::post_voice(&v.app, &m, v.bearer.as_deref(), said),
            None => (StatusCode::NOT_FOUND, "not a live match").into_response(),
        })
    })
    .await
}

/// `/voice/link`: off to Discord.
pub(super) async fn link(State(app): State<Shared>) -> impl IntoResponse {
    voice_link::link(&app)
}

/// What Discord sends a browser back with.
#[derive(Deserialize)]
pub(super) struct Linked {
    #[serde(default)]
    state: String,
    #[serde(default)]
    code: String,
}

/// `/voice/linked`: back from Discord.
pub(super) async fn linked(v: Visit, jar: CookieJar, Query(q): Query<Linked>) -> Answered {
    answer(move || {
        let began = jar.get(voice_link::STATE_COOKIE).map(|c| c.value().to_string());
        Ok(voice_link::linked(&v.app, &q.state, &q.code, began.as_deref()))
    })
    .await
}

/// `?discord=<id>`.
#[derive(Deserialize)]
pub(super) struct Discord {
    discord: Option<String>,
}

/// `/voice/who`: the Steam id linked to a Discord user, to the bot.
pub(super) async fn who(v: Visit, Query(q): Query<Discord>) -> Answered {
    answer(move || Ok(voice_link::who(&v.app, v.bearer.as_deref(), q.discord.as_deref()))).await
}

/// `GET /voice/mumble`: the Mumble channels the bot is wanted in; it waits for a change.
pub(super) async fn mumble_jobs(v: Visit, Query(q): Query<Since>) -> Answered {
    answer(move || Ok(voice_mumble::get_mumble_jobs(&v.app, v.sessions()?, v.bearer.as_deref(), q.since()))).await
}

/// `GET /voice/known`.
pub(super) async fn known(v: Visit, Query(q): Query<Since>) -> Answered {
    answer(move || Ok(voice_who_sync::get_known(&v.app, v.bearer.as_deref(), q.since().unwrap_or(0)))).await
}

/// `POST /voice/known`.
pub(super) async fn told(v: Visit, Json(said): Json<RegionEntry>) -> Answered {
    answer(move || Ok(voice_who_sync::post_known(&v.app, v.bearer.as_deref(), &said))).await
}
