//! A page's event stream: a hub's lines as Server-Sent Events.

use super::{TEXT, param, respond, write_head};
use crate::matches::hub::{SharedHub, subscribe_to};
use anyhow::Result;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;
use std::net::TcpStream;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

/// The event stream's gzip level: most of the size win at little CPU.
pub(super) const STREAM_GZIP: u32 = 5;

/// An event stream with nothing to send sends a comment this often, so proxies keep it open.
pub(super) const KEEPALIVE: Duration = Duration::from_secs(15);

/// After a restart (a deploy), browsers come back after this instead of their default 3 s.
pub(super) const RECONNECT_AFTER: Duration = Duration::from_secs(1);

pub(super) fn too_many_streams(s: &mut TcpStream) -> std::io::Result<()> {
    respond(s, "429 Too Many Requests", TEXT, b"too many matches open from your address")
}

pub(super) fn wants_detail(query: &str) -> bool {
    param(query, "detail") == Some("1")
}

pub(super) fn wants_voice(query: &str) -> bool {
    param(query, "voice") == Some("1")
}

/// A match's events to one browser, until it goes away.
/// Snapshots are about 17 a second of very similar JSON: gzip, flushed after each batch so nothing
/// waits, brings a viewer from about 95 KB/s down to a few.
pub(super) fn stream(mut s: TcpStream, hub: &SharedHub, gzip: bool, detail: bool, voice: bool) -> Result<()> {
    let mut headers = vec![("Content-Type", "text/event-stream"), ("Cache-Control", "no-cache"), ("X-Accel-Buffering", "no")];
    if gzip {
        headers.push(("Content-Encoding", "gzip"));
    }
    write_head(&mut s, "200 OK", &headers)?;
    let mut out: Box<dyn Write> = if gzip { Box::new(GzEncoder::new(s, Compression::new(STREAM_GZIP))) } else { Box::new(s) };
    write!(out, "retry: {}\n\n", RECONNECT_AFTER.as_millis())?;
    let rx = subscribe_to(hub, detail, voice);
    loop {
        match rx.recv_timeout(KEEPALIVE) {
            // Whatever else is already queued goes out in the same flush.
            Ok(line) => {
                for line in std::iter::once(line).chain(rx.try_iter()) {
                    write!(out, "data: {line}\n\n")?;
                }
            }
            Err(RecvTimeoutError::Timeout) => write!(out, ": keepalive\n\n")?,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        out.flush()?;
    }
}
