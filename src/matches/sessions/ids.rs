//! Match ids: random ones, and a demo's.

use std::fmt::Write as _;

use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;

/// A match id: this many random bytes, as hex in links and file names.
pub(super) const ID_BYTES: usize = 16;

/// How much of a match's or room's id the log shows.
const LOGGED_ID: usize = 8;

/// Id `id` as the log names it.
pub(crate) fn logged(id: &str) -> &str {
    id.get(..LOGGED_ID).unwrap_or(id)
}

/// Demo `demo`'s match id, the same each time it is opened: its page's link lasts. A demos.tf
/// demo's is `d<its number>`; one of ours is its kind and the rest of its number, `pov/1a2b…`.
pub(crate) fn demo_id(demo: u64) -> String {
    if is_own_demo(demo) {
        let kind = Own::ALL[((demo >> KIND_SHIFT) & 3) as usize % Own::ALL.len()];
        return format!("{}/{:0OWN_HEX$x}", kind.name(), demo & (KIND_BIT - 1));
    }
    format!("d{demo}")
}

/// The demo whose match id `id` is, by [`demo_id`].
pub(crate) fn id_demo(id: &str) -> Option<u64> {
    if let Some((kind, hex)) = id.split_once('/') {
        let kind = Own::ALL.iter().position(|k| k.name() == kind)? as u64;
        let rest = u64::from_str_radix(hex, 16).ok().filter(|r| hex.len() == OWN_HEX && *r < KIND_BIT)?;
        return Some(OWN_DEMOS | (kind << KIND_SHIFT) | rest);
    }
    id.strip_prefix('d').filter(|n| n.bytes().all(|b| b.is_ascii_digit())).and_then(|n| n.parse().ok()).filter(|d| !is_own_demo(*d))
}

/// A match's id at the start of `path` (what follows `/w/`), and what follows it: one segment,
/// or the two of a demo of ours.
pub(crate) fn split_id(path: &str) -> (&str, &str) {
    let (first, after) = path.split_once('/').unwrap_or((path, ""));
    let second = after.split('/').next().unwrap_or_default();
    let own = first.len() + 1 + second.len();
    if id_demo(&path[..own.min(path.len())]).is_some_and(is_own_demo) {
        return (&path[..own], path[own..].strip_prefix('/').unwrap_or(""));
    }
    (first, after)
}

/// Where the numbers of demos that are not demos.tf's start: one uploaded here, or a match's demo
/// with its comms, played as a demo of its own. Below 2^53, so a page reads them whole.
const OWN_DEMOS: u64 = 1 << 52;
/// Where in such a number its kind is, the rest below it; and the hex digits of that rest.
const KIND_SHIFT: u32 = 50;
const KIND_BIT: u64 = 1 << KIND_SHIFT;
const OWN_HEX: usize = 13;

/// What a demo of ours is: its link says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Own {
    /// A `SourceTV` demo uploaded here.
    Demo,
    /// A player's own demo uploaded here.
    Pov,
    /// A match's demo with its comms.
    Comms,
}

impl Own {
    const ALL: [Own; 3] = [Own::Demo, Own::Pov, Own::Comms];

    fn name(self) -> &'static str {
        match self {
            Own::Demo => "demo",
            Own::Pov => "pov",
            Own::Comms => "comms",
        }
    }
}

/// The number a demo that is ours plays under: the same for the same `key` (FNV-1a), in a
/// process after this one too.
pub(crate) fn own_demo(kind: Own, key: &str) -> u64 {
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let kind = Own::ALL.iter().position(|k| *k == kind).unwrap_or(0) as u64;
    OWN_DEMOS | (kind << KIND_SHIFT) | (hash & (KIND_BIT - 1))
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
