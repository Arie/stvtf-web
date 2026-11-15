//! A match's voice routes, the same on its page and on a room's on it.

use super::Out;
use super::visitor::Who;
use super::{Request, TEXT, respond};
use crate::App;
use crate::matches::sessions::Session;
use std::sync::Arc;

/// `<page><rest>` if it is one of the voice routes, for match `m` (for a room, while its match
/// is live): the answer; none for another route.
pub(super) fn voice_route(
    s: &mut Out,
    app: &App,
    m: Option<&Arc<Session>>,
    request: &Request,
    rest: &str,
    who: &Who,
) -> Option<std::io::Result<()>> {
    let match_of = m.map(Arc::as_ref);
    Some(match (rest, request.method.as_str()) {
        ("voice-mumble", "POST") => super::voice_mumble::post_mumble(s, app, match_of, request, who),
        ("voice-status", "POST") => super::voice_who::post_voice_status(s, app, match_of, request),
        ("voice-comms", "GET") => super::voice_who::get_voice_comms(s, app, match_of, who),
        ("voices", "GET") => super::voice_who::get_voices(s, app, match_of, who),
        ("voice-match", "POST") => super::voice_who::post_voice_match(s, app, match_of, request, who),
        ("voice-invite", "GET") => super::voice_in::get_voice_invite(s, app, match_of, who),
        ("voice", "POST") => match m {
            Some(m) => super::voice_in::post_voice(s, app, m, request),
            None => respond(s, "404 Not Found", TEXT, b"not a live match"),
        },
        _ => return None,
    })
}
