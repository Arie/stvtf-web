//! What is someone's on the front page: their uploaded demo, and the matches and rooms going in
//! their name.

use super::asked::Visit;
use super::client::counted;
use super::pages::Yours;
use super::upload;
use crate::matches::sessions::{self, Sessions};

/// What is theirs, to list.
pub(super) fn mine(v: &Visit, sessions: &Sessions) -> Vec<Yours> {
    let (app, w) = (&v.app, &v.who);
    let uid = w.user.as_ref().map(|u| u.uid.as_str());
    let by = counted(w.user.as_ref(), None, v.client);
    let mut rows = Vec::new();
    let mut row = |kind: &'static str, page: String, what: &str| rows.push(Yours { kind, page, what: what.to_string() });
    let uploads = sessions.store.as_ref().map_or_default(|store| upload::kept_by(store, uid, by.as_deref()));
    for (demo, name) in &uploads {
        row("Demo", format!("/w/{}/", sessions::demo_id(*demo)), name);
    }
    for m in sessions.of(uid, by.as_deref()) {
        if m.demo.is_some_and(|d| uploads.iter().any(|(u, _)| *u == d)) {
            continue;
        }
        let kind = if m.demo.is_some() { "Demo" } else { "Match" };
        row(kind, format!("/w/{}/", m.id), m.map.as_deref().unwrap_or("starting"));
    }
    for (id, map) in app.rooms.of(&w.key) {
        row("Room", format!("/r/{id}/"), &map);
    }
    rows
}
