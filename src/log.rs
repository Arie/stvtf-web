//! What the server says of itself, through `tracing`: on stderr, and with `--logs DIR` in files
//! there too, which outlive the container (a deploy starts a new one, and the old one's own log
//! goes with it): `<DIR>/<name>.<date>.log`, a new one each day and a week of them kept.
//! `RUST_LOG` says how much (default `info`; `tower_http=debug` has each request).

use std::path::Path;
use std::sync::Mutex;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, fmt};

/// How many days' files are kept.
const KEPT: usize = 7;

/// Starts saying things: on stderr, and in `dir` as `<name>.<date>.log` if it can be written
/// (a folder that cannot is not worth not starting for: stderr has it all).
pub(crate) fn start(dir: Option<&Path>, name: &str) {
    let level = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // Plain, without colours: it is read from files and containers' logs.
    let stderr = fmt::layer().with_target(false).with_ansi(false).with_writer(std::io::stderr).with_filter(level());
    if let Some(dir) = dir {
        let _ = std::fs::create_dir_all(dir);
    }
    let daily = |dir| {
        RollingFileAppender::builder().rotation(Rotation::DAILY).filename_prefix(name).filename_suffix("log").max_log_files(KEPT).build(dir)
    };
    let (files, refused) = match dir.map(daily) {
        Some(Ok(files)) => (Some(files), None),
        Some(Err(e)) => (None, Some(e)),
        None => (None, None),
    };
    // Written as it is said: little is, and nothing of it is lost when the process ends.
    let kept = files.map(|files| fmt::layer().with_target(false).with_ansi(false).with_writer(Mutex::new(files)).with_filter(level()));
    tracing_subscriber::registry().with(stderr).with(kept).init();
    if let (Some(dir), Some(e)) = (dir, refused) {
        tracing::warn!("no log kept in {}: {e}", dir.display());
    }
}

/// Says a line, formatted as `println!` would.
macro_rules! log {
    ($($arg:tt)*) => {
        tracing::info!($($arg)*)
    };
}
