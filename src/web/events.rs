//! A page's event stream: a hub's lines as Server-Sent Events.

use super::asked::Stream;
use crate::matches::hub::{SharedHub, subscribe_to};
use axum::body::{Body, Bytes};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderName};
use axum::response::{IntoResponse, Response};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::error::TrySendError;

/// The event stream's gzip level: most of the size win at little CPU.
pub(super) const STREAM_GZIP: u32 = 5;

/// An event stream with nothing to send sends a comment this often, so proxies keep it open.
pub(super) const KEEPALIVE: Duration = Duration::from_secs(15);

/// After a restart (a deploy), browsers come back after this instead of their default 3 s.
pub(super) const RECONNECT_AFTER: Duration = Duration::from_secs(1);

pub(super) fn too_many_streams() -> Response {
    (StatusCode::TOO_MANY_REQUESTS, "too many matches open from your address").into_response()
}

/// A match's events to one browser, until it goes away.
/// Snapshots are about 17 a second of very similar JSON: gzip, flushed after each batch so nothing
/// waits, brings a viewer from about 95 KB/s down to a few.
pub(super) fn stream(hub: &SharedHub, gzip: bool, asked: &Stream, held: impl Send + 'static) -> Response {
    let Some(open) = Streaming::open() else { return StatusCode::SERVICE_UNAVAILABLE.into_response() };
    let rx = subscribe_to(hub, asked.detail(), asked.voice());
    let (tx, body) = tokio::sync::mpsc::channel(QUEUED);
    std::thread::spawn(move || {
        // Held while it streams: its place among the streams, its viewer of the match.
        let (_open, _held) = (open, held);
        let to = Chunks { tx, held: Vec::new() };
        let mut out: Box<dyn Write> = if gzip { Box::new(GzEncoder::new(to, Compression::new(STREAM_GZIP))) } else { Box::new(to) };
        let _ = events(&mut out, &rx);
    });
    let unbuffered = HeaderName::from_static("x-accel-buffering");
    let headers = [(header::CONTENT_TYPE, "text/event-stream"), (header::CACHE_CONTROL, "no-cache"), (unbuffered, "no")];
    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(body));
    let gzipped = gzip.then_some([(header::CONTENT_ENCODING, "gzip")]);
    (headers, gzipped, body).into_response()
}

/// The lines `rx` hears written as events, flushed after each batch, until the client is gone.
fn events(out: &mut dyn Write, rx: &Receiver<Arc<str>>) -> std::io::Result<()> {
    write!(out, "retry: {}\n\n", RECONNECT_AFTER.as_millis())?;
    out.flush()?;
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

/// Event streams open at once; more are refused.
pub(crate) const MAX_STREAMS: usize = 512;
/// Flushes of events that may wait for a slow client, and how long one waits before it is gone.
const QUEUED: usize = 64;
const SLOW_FOR: Duration = Duration::from_secs(30);
const SLOW_STEP: Duration = Duration::from_millis(20);

static STREAMS: AtomicUsize = AtomicUsize::new(0);

/// Counts an event stream until dropped.
struct Streaming;

impl Streaming {
    fn open() -> Option<Self> {
        if STREAMS.fetch_add(1, Ordering::Relaxed) >= MAX_STREAMS {
            STREAMS.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        Some(Streaming)
    }
}

impl Drop for Streaming {
    fn drop(&mut self) {
        STREAMS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// What is written, sent on as a chunk of the body at each flush.
struct Chunks {
    tx: tokio::sync::mpsc::Sender<std::io::Result<Bytes>>,
    held: Vec<u8>,
}

impl Write for Chunks {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.held.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    /// Fails once the client is gone, or has taken nothing for [`SLOW_FOR`].
    fn flush(&mut self) -> std::io::Result<()> {
        if self.held.is_empty() {
            return Ok(());
        }
        let mut chunk = Ok(Bytes::from(std::mem::take(&mut self.held)));
        let since = Instant::now();
        loop {
            match self.tx.try_send(chunk) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Closed(_)) => return Err(std::io::ErrorKind::BrokenPipe.into()),
                Err(TrySendError::Full(_)) if since.elapsed() > SLOW_FOR => return Err(std::io::ErrorKind::TimedOut.into()),
                Err(TrySendError::Full(back)) => chunk = back,
            }
            std::thread::sleep(SLOW_STEP);
        }
    }
}
