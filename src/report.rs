//! What goes wrong, told to Sentry too (`SENTRY_DSN`; without it nothing is sent): a panic of any
//! thread, a line said with `fault!`, and what a viewer's page reports of itself (with the viewer's
//! address, and the Steam id of one signed in).

use sentry::protocol::{Event, Exception, Frame, IpAddress, Level, Request, Stacktrace, User};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// One kind of fault is told once in this long: a save failing every few seconds is one issue.
const ONCE_IN: Duration = Duration::from_secs(60);
/// Pages' reports taken in a minute, of all viewers together, and the most kept of one's text.
const PAGE_REPORTS: usize = 30;
const PAGE_TEXT: usize = 2000;
const PAGE_FRAMES: usize = 50;

static TOLD: LazyLock<Mutex<HashMap<&'static str, Instant>>> = LazyLock::new(Mutex::default);
static PAGES: Mutex<Vec<Instant>> = Mutex::new(Vec::new());
/// Where pages' reports go: a hub of their own, without what the server says of itself (its
/// system, its process).
static PAGE_HUB: OnceLock<sentry::Hub> = OnceLock::new();

/// Starts telling Sentry, as `process` ("web" or "relay") of `STVTF_REGION`. Kept until the
/// process ends: dropped, it sends what is left.
pub(crate) fn start(process: &'static str) -> sentry::ClientInitGuard {
    let region = std::env::var("STVTF_REGION").ok().filter(|r| !r.is_empty());
    let mut options = sentry::ClientOptions::new();
    options.server_name = region.clone().map(Into::into);
    let guard = sentry::init(options);
    sentry::configure_scope(|scope| {
        scope.set_tag("process", process);
        if let Some(region) = &region {
            scope.set_tag("region", region);
        }
    });
    let mut bare = sentry::ClientOptions::new();
    bare.default_integrations = false;
    let pages = sentry::Hub::new(Some(Arc::new(sentry::Client::from(sentry::apply_defaults(bare)))), Arc::default());
    pages.configure_scope(|scope| {
        scope.set_tag("process", "page");
        if let Some(region) = &region {
            scope.set_tag("region", region);
        }
    });
    let _ = PAGE_HUB.set(pages);
    guard
}

/// Says `text` as a line, and tells Sentry: `kind` (the line unfilled) is what makes it the same
/// fault as another.
pub(crate) fn fault(kind: &'static str, text: &str) {
    tracing::error!("{text}");
    if due(kind, Instant::now()) {
        sentry::capture_event(Event {
            message: Some(text.to_string()),
            level: Level::Error,
            fingerprint: vec![kind.into()].into(),
            ..Default::default()
        });
    }
}

fn due(kind: &'static str, now: Instant) -> bool {
    let mut told = TOLD.lock().unwrap_or_else(PoisonError::into_inner);
    let due = told.get(kind).is_none_or(|at| now.duration_since(*at) >= ONCE_IN);
    if due {
        told.insert(kind, now);
    }
    due
}

/// Says a line as `log!` does, and tells Sentry of it.
macro_rules! fault {
    ($fmt:literal $($arg:tt)*) => {
        $crate::report::fault($fmt, &format!($fmt $($arg)*))
    };
}

/// The trace of the request being answered, if any.
pub(crate) fn trace_id() -> Option<String> {
    sentry::configure_scope(|s| s.get_span()).map(|span| span.get_trace_context().trace_id.to_string())
}

/// Its `sentry-trace` header, to carry it to another process.
pub(crate) fn trace_header() -> Option<String> {
    let span = sentry::configure_scope(|s| s.get_span())?;
    span.iter_headers().find(|(name, _)| *name == "sentry-trace").map(|(_, value)| value)
}

/// Tags the events of this thread, or of the request being answered.
pub(crate) fn tag(key: &str, value: &str) {
    sentry::configure_scope(|s| s.set_tag(key, value));
}

/// Tags a request with the room or match its path is of.
pub(crate) fn tag_path(path: &str) {
    if let Some((key, id)) = of_path(path) {
        tag(key, id);
    }
}

fn of_path(path: &str) -> Option<(&'static str, &str)> {
    let mut parts = path.split('/').skip(1);
    match (parts.next(), parts.next(), parts.next()) {
        (Some("r"), Some(id), _) if !id.is_empty() => Some(("room", id)),
        (Some("w"), Some("demo" | "pov" | "comms"), Some(id)) | (Some("w"), Some(id), _) if !id.is_empty() => Some(("match", id)),
        _ => None,
    }
}

/// Runs `f` in the trace another process sent (`header`, as `sentry-trace`), its events tagged.
pub(crate) fn continued<T>(name: &str, header: Option<&str>, tags: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    sentry::Hub::run(Arc::new(sentry::Hub::new_from_top(sentry::Hub::current())), || {
        let trace = header
            .map(|h| sentry::start_transaction(sentry::TransactionContext::continue_from_headers(name, "relay", [("sentry-trace", h)])));
        sentry::configure_scope(|s| {
            for (key, value) in tags {
                s.set_tag(key, value);
            }
            if let Some(t) = &trace {
                s.set_span(Some(t.clone().into()));
            }
        });
        let out = f();
        if let Some(t) = trace {
            t.finish();
        }
        out
    })
}

/// Sentry on, sending nowhere: for tests of what it is told.
#[cfg(test)]
pub(crate) fn on_for_tests() {
    static ON: std::sync::Once = std::sync::Once::new();
    let client = || {
        let mut options = sentry::ClientOptions::new();
        options.dsn = "https://public@sentry.invalid/1".parse().ok();
        options.transport = Some(Arc::new(sentry::test::TestTransport::new()));
        Some(Arc::new(sentry::Client::from(options)))
    };
    ON.call_once(|| sentry::Hub::main().bind_client(client()));
    sentry::Hub::current().bind_client(client());
}

/// What a page says went wrong in it.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(crate) struct PageFault {
    /// The error's name and message.
    kind: String,
    message: String,
    stack: String,
    /// The page's path, and the build of its files.
    page: String,
    build: String,
    /// The trace of the last request that failed it (`x-trace-id`).
    trace: String,
}

/// Tells Sentry what a page reported, with its browser (`agent`) and whose it is (`client`, and
/// `steam` of one signed in). Whether it was taken: only so many are in a minute.
pub(crate) fn page_fault(body: &[u8], agent: &str, client: IpAddr, steam: Option<&str>) -> bool {
    let Ok(fault) = serde_json::from_slice::<PageFault>(body) else { return false };
    if fault.message.is_empty() || !page_slot(Instant::now()) {
        return false;
    }
    if let Some(pages) = PAGE_HUB.get() {
        pages.capture_event(page_event(&fault, agent, client, steam));
    }
    true
}

fn page_slot(now: Instant) -> bool {
    let mut taken = PAGES.lock().unwrap_or_else(PoisonError::into_inner);
    taken.retain(|at| now.duration_since(*at) < Duration::from_secs(60));
    let free = taken.len() < PAGE_REPORTS;
    if free {
        taken.push(now);
    }
    free
}

fn page_event(fault: &PageFault, agent: &str, client: IpAddr, steam: Option<&str>) -> Event<'static> {
    let frames = frames(&fault.stack);
    let mut event = Event {
        platform: "javascript".into(),
        level: Level::Error,
        exception: vec![Exception {
            ty: cut(if fault.kind.is_empty() { "Error" } else { &fault.kind }, 100),
            value: Some(cut(&fault.message, PAGE_TEXT)),
            stacktrace: (!frames.is_empty()).then(|| Stacktrace { frames, ..Default::default() }),
            ..Default::default()
        }]
        .into(),
        user: Some(User { id: steam.map(str::to_string), ip_address: Some(IpAddress::Exact(client.to_canonical())), ..Default::default() }),
        request: Some(Request { headers: [("User-Agent".to_string(), cut(agent, 300))].into(), ..Default::default() }),
        ..Default::default()
    };
    event.tags.insert("page".into(), cut(&fault.page, 100));
    event.tags.insert("build".into(), cut(&fault.build, 40));
    // In the trace of the request that failed the page, else of its report.
    let trace = fault.trace.parse().ok().or_else(|| trace_id()?.parse().ok());
    if let Some(trace_id) = trace {
        event.contexts.insert("trace".into(), sentry::protocol::TraceContext { trace_id, ..Default::default() }.into());
    }
    event
}

/// `text`, no longer than `most` characters.
fn cut(text: &str, most: usize) -> String {
    text.chars().take(most).collect()
}

/// A browser's stack as frames, the innermost last (as Sentry has them): `at name (file:1:2)`
/// of Chrome, `name@file:1:2` of Firefox and Safari. Lines that are neither are left out.
fn frames(stack: &str) -> Vec<Frame> {
    let mut frames: Vec<Frame> = stack.lines().take(PAGE_FRAMES).filter_map(frame).collect();
    frames.reverse();
    frames
}

fn frame(line: &str) -> Option<Frame> {
    let line = line.trim();
    let (name, at) = if let Some(rest) = line.strip_prefix("at ") {
        match rest.rsplit_once(" (") {
            Some((name, at)) => (name, at.strip_suffix(')')?),
            None => ("", rest),
        }
    } else {
        line.split_once('@')?
    };
    let (rest, col) = at.rsplit_once(':')?;
    let (file, row) = rest.rsplit_once(':')?;
    // A file of this build, by its name alone: the build's part of the path changes every deploy.
    let file = file.split_once("/app/").map_or(file, |(_, f)| f.split_once('/').map_or(f, |(_, f)| f));
    Some(Frame {
        function: (!name.is_empty()).then(|| cut(name, 200)),
        filename: Some(cut(file, 200)),
        lineno: Some(row.parse().ok()?),
        colno: Some(col.parse().ok()?),
        in_app: Some(!file.contains("://")),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_of_fault_is_told_once_in_a_while() {
        let now = Instant::now();
        assert!(due("a test's fault {}", now));
        assert!(!due("a test's fault {}", now + ONCE_IN / 2));
        assert!(due("another of a test {}", now));
        assert!(due("a test's fault {}", now + ONCE_IN));
    }

    #[test]
    fn a_stack_of_either_kind_of_browser_reads_as_frames_innermost_last() {
        let chrome = "TypeError: x is undefined\n    at drawPlayers (https://stv.tf/app/1a2b/map/players.js:12:34)\n    at https://stv.tf/app/1a2b/main.js:5:6";
        let read = frames(chrome);
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].function.as_deref(), Some("drawPlayers"));
        assert_eq!(read[1].filename.as_deref(), Some("map/players.js"));
        assert_eq!((read[1].lineno, read[1].colno), (Some(12), Some(34)));
        assert_eq!((read[0].function.as_deref(), read[0].filename.as_deref()), (None, Some("main.js")));
        let firefox = "drawPlayers@https://stv.tf/app/1a2b/map/players.js:12:34\n@https://stv.tf/app/1a2b/main.js:5:6\n";
        let read = frames(firefox);
        assert_eq!(read[1].function.as_deref(), Some("drawPlayers"));
        assert_eq!(read[0].filename.as_deref(), Some("main.js"));
        // Another's script is not this build's.
        let other = frames("at f (chrome-extension://abc/inject.js:1:2)");
        assert_eq!(other[0].in_app, Some(false));
    }

    #[test]
    fn a_page_s_report_becomes_an_error_of_its_kind() {
        let fault: PageFault = serde_json::from_str(
            r#"{"kind":"TypeError","message":"x is undefined","stack":"f@https://stv.tf/app/1/a.js:1:2","page":"/r/abc/","build":"1"}"#,
        )
        .unwrap();
        let event = page_event(&fault, "Mozilla/5.0", "203.0.113.7".parse().unwrap(), Some("76561198000000001"));
        assert_eq!(event.user.as_ref().unwrap().id.as_deref(), Some("76561198000000001"));
        assert_eq!(event.user.as_ref().unwrap().ip_address, Some(IpAddress::Exact("203.0.113.7".parse().unwrap())));
        assert_eq!(event.exception[0].ty, "TypeError");
        assert_eq!(event.exception[0].value.as_deref(), Some("x is undefined"));
        assert_eq!(event.tags["page"], "/r/abc/");
        assert_eq!(event.exception[0].stacktrace.as_ref().unwrap().frames.len(), 1);
    }

    #[test]
    fn only_so_many_pages_reports_are_taken_in_a_minute() {
        let now = Instant::now() + Duration::from_secs(3600);
        assert!((0..PAGE_REPORTS).all(|_| page_slot(now)));
        assert!(!page_slot(now));
        assert!(page_slot(now + Duration::from_secs(61)));
    }

    #[test]
    fn a_path_names_its_room_or_match() {
        assert_eq!(of_path("/r/abc/events"), Some(("room", "abc")));
        assert_eq!(of_path("/w/abc/"), Some(("match", "abc")));
        assert_eq!(of_path("/w/demo/xyz/room"), Some(("match", "xyz")));
        assert_eq!(of_path("/w/"), None);
        assert_eq!(of_path("/"), None);
    }

    #[test]
    fn a_page_fault_is_in_the_trace_of_the_request_that_failed_it() {
        let trace = "0123456789abcdef0123456789abcdef";
        let fault = PageFault { message: "x".into(), trace: trace.into(), ..Default::default() };
        let event = page_event(&fault, "", IpAddr::from([127, 0, 0, 1]), None);
        let Some(sentry::protocol::Context::Trace(t)) = event.contexts.get("trace") else { panic!("no trace") };
        assert_eq!(t.trace_id.to_string(), trace);
        let junk = PageFault { message: "x".into(), trace: "<script>".into(), ..Default::default() };
        assert!(!page_event(&junk, "", IpAddr::from([127, 0, 0, 1]), None).contexts.contains_key("trace"));
    }

    #[test]
    fn a_continued_trace_is_the_senders() {
        on_for_tests();
        let header = "0123456789abcdef0123456789abcdef-0123456789abcdef-0";
        let (id, sent) = continued("t", Some(header), &[], || (trace_id(), trace_header()));
        assert_eq!(id.as_deref(), Some("0123456789abcdef0123456789abcdef"));
        assert!(sent.is_some_and(|h| h.starts_with("0123456789abcdef0123456789abcdef-")));
        assert_eq!(continued("t", None, &[], trace_id), None);
    }
}
