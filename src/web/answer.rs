//! Answers to requests, and the early ones that end a request before its work.

use std::sync::Arc;

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::app::App;
use crate::matches::recording::Mark;

/// A handler's answer, or the answer that ended the request early: a refusal, a redirect, a fault.
pub(crate) type Answered = Result<Response, Early>;

pub(crate) struct Early(pub(crate) Box<Response>);

impl IntoResponse for Early {
    fn into_response(self) -> Response {
        *self.0
    }
}

/// The app, as every handler has it.
pub(crate) type Shared = Arc<App>;

impl From<Response> for Early {
    fn from(response: Response) -> Self {
        Early(Box::new(response))
    }
}

impl From<anyhow::Error> for Early {
    fn from(_: anyhow::Error) -> Self {
        StatusCode::INTERNAL_SERVER_ERROR.into_response().into()
    }
}

impl From<std::io::Error> for Early {
    fn from(_: std::io::Error) -> Self {
        StatusCode::INTERNAL_SERVER_ERROR.into_response().into()
    }
}

/// `pairs` as a query string.
pub(super) fn query_of(pairs: &[(&str, &str)]) -> String {
    serde_urlencoded::to_string(pairs).unwrap_or_default()
}

/// The marks from the one `asked` names onward, and how many there are in all.
pub(super) fn marks_from(asked: &super::asked::Marks, marks: impl FnOnce(usize) -> (Vec<Mark>, usize)) -> Response {
    let (marks, all) = marks(asked.from());
    Json(serde_json::json!({ "marks": marks, "all": all })).into_response()
}

/// `204` when `done` is `Ok`, else `400` with the reason.
pub(super) fn done(done: Result<()>) -> Response {
    match done {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    }
}

pub(super) fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
