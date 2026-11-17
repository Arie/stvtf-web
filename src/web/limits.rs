//! How much the site takes and keeps: cookies, request sizes, streams and caching.

use std::time::Duration;

/// Who the browser is, to the rooms; and who signed in with the site, still sealed.
pub(super) const GUEST_COOKIE: &str = "stvtf_guest";

pub(super) const USER_COOKIE: &str = "stvtf_user";

/// Request line and headers together; the pages send far less.
pub(super) const MAX_REQUEST: u64 = 16 << 10;

/// Event streams from one address, so a single client cannot take all the connections. A LAN
/// event or an office behind one address stays well under it.
pub(crate) const MAX_STREAMS_PER_CLIENT: usize = 32;

/// The longest delay a page can ask for after its last `/`, to line up with a stream of the match.
pub(super) const MAX_DELAY_S: f64 = 60.0;

/// How long a browser keeps a file that does not change for a given build.
pub(super) const CACHE_FOR: Duration = Duration::from_hours(24);
