//! The visitor: signed in with the site, or a guest by cookie, and where they go back to.

use super::{GUEST_COOKIE, token};
use crate::App;
use crate::matches::sessions;
use cookie::time::Duration;
use cookie::{Cookie, SameSite};

/// How long a guest cookie lasts: who one is in rooms, kept across visits.
const GUEST_FOR: std::time::Duration = std::time::Duration::from_hours(365 * 24);

/// A `Set-Cookie` for `name`, lasting `seconds`: only for us, never to scripts, and over HTTPS
/// when a site is in front.
pub(super) fn set_cookie(app: &App, name: &str, value: &str, seconds: u64) -> String {
    Cookie::build((name, value))
        .path(format!("{}/", app.base))
        .max_age(Duration::seconds(i64::try_from(seconds).unwrap_or(i64::MAX)))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(app.keys.is_some())
        .build()
        .to_string()
}

/// Who is asking, to the rooms: the user signed in with the site, by their Steam id, or the
/// browser by its guest cookie; and a `Set-Cookie` for a new guest cookie if it had none.
#[derive(Clone)]
pub(super) struct Who {
    pub(super) key: String,
    pub(super) cookie: Option<String>,
    pub(super) user: Option<token::User>,
}

impl Who {
    /// A browser seen for the first time.
    pub(super) fn new_guest(app: &App) -> Who {
        let g = sessions::random_id();
        let cookie = set_cookie(app, GUEST_COOKIE, &g, GUEST_FOR.as_secs());
        Who { key: g, cookie: Some(cookie), user: None }
    }
}

/// The site's address, from where `/` sends visitors (`--home`): its scheme and host.
pub(super) fn origin(home: &str) -> Option<&str> {
    let rest = home.find("://")? + "://".len();
    Some(&home[..home[rest..].find('/').map_or(home.len(), |i| rest + i)])
}

/// A page of ours to go back to: `to` if it is one (a path, not another site), else our home.
pub(super) fn return_to(app: &App, to: Option<&str>) -> String {
    let path = to.unwrap_or_default();
    let ours =
        path.starts_with(&format!("{}/", app.base)) && !path.starts_with("//") && !path.contains(|c: char| c == '\\' || c.is_control());
    if ours { path.to_string() } else { format!("{}/", app.base) }
}
