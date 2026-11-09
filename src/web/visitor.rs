//! The visitor: signed in with the site, or a guest by cookie, and where they go back to.

use super::{Request, form_decode, token, unix_now};
use crate::App;
use crate::matches::sessions;

/// A cookie's attributes, lasting `seconds`: only for us, never to scripts, and over HTTPS when
/// a site is in front.
pub(super) fn cookie_attributes(app: &App, seconds: u64) -> String {
    let secure = if app.keys.is_some() { "; Secure" } else { "" };
    format!("Path={}/; Max-Age={seconds}; HttpOnly; SameSite=Lax{secure}", app.base)
}

/// Who is asking, to the rooms: the user signed in with the site, by their Steam id, or the
/// browser by its guest cookie; and a `Set-Cookie` for a new guest cookie if it had none.
pub(super) struct Who {
    pub(super) key: String,
    pub(super) cookie: Option<String>,
    pub(super) user: Option<token::User>,
}

pub(super) fn who(app: &App, request: &Request) -> Who {
    let user = app.keys.as_ref().zip(request.user.as_deref()).and_then(|(k, c)| k.kept(c, unix_now()));
    if let Some(user) = user {
        return Who { key: format!("{}{}", crate::rooms::people::STEAM, user.uid), cookie: None, user: Some(user) };
    }
    if let Some(g) = &request.guest {
        return Who { key: g.clone(), cookie: None, user: None };
    }
    let g = sessions::random_id();
    let cookie = format!("stvtf_guest={g}; {}", cookie_attributes(app, 365 * 24 * 3600));
    Who { key: g, cookie: Some(cookie), user: None }
}

/// The site's address, from where `/` sends visitors (`--home`): its scheme and host.
pub(super) fn origin(home: &str) -> Option<&str> {
    let rest = home.find("://")? + 3;
    Some(&home[..home[rest..].find('/').map_or(home.len(), |i| rest + i)])
}

/// A page of ours to go back to: `wanted` if it is one (a path, not another site), else our home.
pub(super) fn local_path(wanted: Option<&str>, app: &App) -> String {
    let path = wanted.map(form_decode).unwrap_or_default();
    let ours = path.starts_with(&format!("{}/", app.base)) && !path.starts_with("//") && !path.contains(['\\', '\r', '\n']);
    if ours { path } else { format!("{}/", app.base) }
}

/// `v` for a query string.
pub(super) fn url_encode(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => char::from(b).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
