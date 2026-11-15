//! What is someone's on the front page: their uploaded demo, and the matches and rooms going in
//! their name.

use super::asked::Visit;
use super::client::counted;
use super::escape;
use super::upload;
use crate::matches::sessions::{self, Sessions};
use std::fmt::Write as _;

/// The list, as HTML; nothing when nothing is theirs.
pub(super) fn mine(v: &Visit, sessions: &Sessions) -> String {
    let (app, w) = (&v.app, &v.who);
    let uid = w.user.as_ref().map(|u| u.uid.as_str());
    let by = counted(w.user.as_ref(), None, v.client);
    let mut rows = String::new();
    let mut row = |kind: &str, page: String, what: &str| {
        let _ = write!(rows, r#"<li><a href="{}{page}"><b>{kind}</b> {}</a></li>"#, app.base, escape(what));
    };
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
    if rows.is_empty() { rows } else { format!(r#"<h2>Yours</h2><ul class="mine">{rows}</ul>"#) }
}
