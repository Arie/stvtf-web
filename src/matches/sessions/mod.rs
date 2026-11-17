//! Many matches in one process: a watcher per `SourceTV` server, started when someone asks for
//! it and stopped a while after its last viewer leaves. With a store, the matches outlive the
//! process: a deploy's new container picks them up under the same links.

mod address;
mod ids;
mod pool;
mod record;
mod session;
mod store;
mod voice;

pub(crate) use self::address::is_public;
pub(crate) use self::ids::{Own, demo_id, id_demo, is_id, is_own_demo, logged, own_demo, random_id, split_id};
pub(crate) use self::pool::MAKES_ROOM;
pub use self::pool::Sessions;
pub(crate) use self::session::{Session, Viewer};
pub(crate) use self::store::{DEMO_PREFIX, LIVE_PREFIX, Store, age, demo_dem, demo_info, demo_output, is_kept_demo, sweep_demos};

#[cfg(test)]
mod handover_tests;
#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
use self::store::live_file;
