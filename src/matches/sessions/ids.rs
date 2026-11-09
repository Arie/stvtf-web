//! Match ids: random ones, and a demos.tf demo's.

use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use std::fmt::Write as _;

/// A match id: this many random bytes, as hex in links and file names.
pub(super) const ID_BYTES: usize = 16;

/// Demo `demo`'s match id, the same each time it is opened: its page's link lasts.
pub(crate) fn demo_id(demo: u64) -> String {
    format!("d{demo}")
}

/// The demo whose match id `id` is, by [`demo_id`].
pub(crate) fn id_demo(id: &str) -> Option<u64> {
    id.strip_prefix('d').filter(|n| n.bytes().all(|b| b.is_ascii_digit())).and_then(|n| n.parse().ok())
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
