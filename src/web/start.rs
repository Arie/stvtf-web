//! Starting to watch a server or demo, and the page to start from.

use std::net::IpAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::response::Response;

use super::client::counted;
use crate::app::App;
use crate::matches::sessions;
use crate::matches::sessions::Sessions;

/// The match `line` asks for: a connect string, or a demos.tf link. Behind a site, only for
/// someone signed in.
pub(super) fn watch(
    app: &App,
    sessions: &Sessions,
    user: Option<&super::token::User>,
    line: &str,
    client: IpAddr,
) -> Result<Arc<sessions::Session>> {
    if app.keys.is_some() && user.is_none() {
        anyhow::bail!("Sign in first");
    }
    let by = counted(user, None, client);
    let owner = user.map(|u| u.uid.as_str());
    match demostf_link(line) {
        Some(demo) => sessions.open_demo_as(by.as_deref(), demo, owner),
        None => sessions.open_as(by.as_deref(), line, owner, None),
    }
}

/// The demo of a demos.tf link.
pub(super) fn demostf_link(line: &str) -> Option<u64> {
    let rest = line.trim().trim_start_matches("https://").trim_start_matches("www.").strip_prefix("demos.tf/")?;
    rest.trim_end_matches('/').parse().ok().filter(|d| !sessions::is_own_demo(*d))
}

/// The front page, with `error` if any.
pub(super) fn landing(v: &super::asked::Visit, sessions: &Sessions, error: &str) -> Response {
    let signed = v.app.keys.is_none() || v.who.user.is_some();
    super::pages::front(&v.app, signed, error, super::mine::mine(v, sessions))
}
