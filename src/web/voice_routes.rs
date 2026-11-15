//! A match's voice routes, the same on its page and on a room's on it.

// Handlers have the shape the routes' adapters take (`server::to` and the like), whether or not
// one can fail or needs to own what was posted.
#![allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)]

use super::routes::Asked;
use super::visitor::{Who, who};
use super::voice_in::VoicePost;
use super::voice_mumble::Ask;
use super::voice_who::{Match, Report};
use super::{Answered, Early, TEXT, respond};
use crate::matches::sessions::Session;
use axum::http::StatusCode;
use std::sync::Arc;

/// The match a voice route under `/w/` or `/r/` is for: a room's is the live one it is on, if any.
fn of(a: &Asked) -> Result<Option<Arc<Session>>, Early> {
    if a.request.path.starts_with("/r/") {
        let (room, _) = super::rooms::found(a)?;
        return Ok(super::rooms::live(&a.app, a.sessions()?, &room));
    }
    Ok(Some(super::matches::found(a)?))
}

fn asking(a: &Asked) -> Who {
    who(&a.app, &a.request)
}

/// `POST <page>voice-mumble`.
pub(super) fn mumble(a: &Asked, ask: Ask) -> Answered {
    Ok(super::voice_mumble::post_mumble(&a.app, of(a)?.as_deref(), ask, &asking(a)))
}

/// `POST <page>voice-status`.
pub(super) fn status(a: &Asked, said: Report) -> Answered {
    Ok(super::voice_who::post_voice_status(&a.app, of(a)?.as_deref(), &a.request, said))
}

/// `GET <page>voice-comms`.
pub(super) fn comms(a: &Asked) -> Answered {
    Ok(super::voice_who::get_voice_comms(&a.app, of(a)?.as_deref(), &asking(a)))
}

/// `GET <page>voices`.
pub(super) fn voices(a: &Asked) -> Answered {
    Ok(super::voice_who::get_voices(&a.app, of(a)?.as_deref(), &asking(a)))
}

/// `POST <page>voice-match`.
pub(super) fn matched(a: &Asked, v: Match) -> Answered {
    Ok(super::voice_who::post_voice_match(&a.app, of(a)?.as_deref(), &v, &asking(a)))
}

/// `GET <page>voice-invite`.
pub(super) fn invite(a: &Asked) -> Answered {
    Ok(super::voice_in::get_voice_invite(&a.app, of(a)?.as_deref(), &asking(a)))
}

/// `POST <page>voice`.
pub(super) fn voice(a: &Asked, said: VoicePost) -> Answered {
    Ok(match of(a)? {
        Some(m) => super::voice_in::post_voice(&a.app, &m, &a.request, said),
        None => respond(StatusCode::NOT_FOUND, TEXT, b"not a live match"),
    })
}
