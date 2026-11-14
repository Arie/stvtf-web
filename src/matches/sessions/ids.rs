//! Match ids: random ones, and a demo's.

use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use std::fmt::Write as _;

/// A match id: this many random bytes, as hex in links and file names.
pub(super) const ID_BYTES: usize = 16;

/// How much of a match's or room's id the log shows.
const LOGGED_ID: usize = 8;

/// Id `id` as the log names it.
pub(crate) fn logged(id: &str) -> &str {
    id.get(..LOGGED_ID).unwrap_or(id)
}

/// Demo `demo`'s match id, the same each time it is opened: its page's link lasts.
pub(crate) fn demo_id(demo: u64) -> String {
    format!("d{demo}")
}

/// The demo whose match id `id` is, by [`demo_id`].
pub(crate) fn id_demo(id: &str) -> Option<u64> {
    id.strip_prefix('d').filter(|n| n.bytes().all(|b| b.is_ascii_digit())).and_then(|n| n.parse().ok())
}

/// Where the numbers of demos that are not demos.tf's start: a match's demo with its comms, played
/// as a demo of its own. Below 2^53, so a page reads them whole.
const OWN_DEMOS: u64 = 1 << 52;

/// The number a demo that is ours plays under: the same for the same `key` (FNV-1a), in a
/// process after this one too.
pub(crate) fn own_demo(key: &str) -> u64 {
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    OWN_DEMOS | (hash & (OWN_DEMOS - 1))
}

/// Whether demo `demo` is one of ours: demos.tf has none by that number.
pub(crate) fn is_own_demo(demo: u64) -> bool {
    demo >= OWN_DEMOS
}

/// Whether `name` is a match id, as made by [`random_id`].
pub(crate) fn is_id(name: &str) -> bool {
    name.len() == 2 * ID_BYTES && name.chars().all(|c| c.is_ascii_hexdigit())
}

/// Random bits from the OS as hex; the link to a match is all it takes to watch it.
pub(crate) fn random_id() -> String {
    let mut id = [0u8; ID_BYTES];
    OsRng.fill_bytes(&mut id);
    id.iter().fold(String::with_capacity(2 * ID_BYTES), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}
