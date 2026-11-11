//! What the tests share: temporary folders, scripted `SourceTV` servers, an app to serve from.

use super::*;
use stvtf::Line;
use stvtf::output;

/// A new directory under the system's temporary one, removed when dropped, also when a test fails.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
        static MADE: AtomicUsize = AtomicUsize::new(0);
        let n = MADE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("stvtf-web-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A UDP port nothing has just now.
pub(crate) fn free_port() -> u16 {
    std::net::UdpSocket::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port()
}

/// What the stand-in for a server's watcher does.
struct Script {
    target: String,
    lines: Vec<Line>,
    give_up: Option<String>,
}

static SCRIPTS: Mutex<Vec<Script>> = Mutex::new(Vec::new());

/// The watcher of `target` will send `lines` and then, if it says why, give up.
pub(crate) fn script(target: &str, lines: Vec<Line>, give_up: Option<&str>) {
    lock(&SCRIPTS).push(Script { target: target.to_string(), lines, give_up: give_up.map(str::to_string) });
}

/// Stands in for [`stvtf::watch::watch`], so no test connects to a server: it does what its
/// [`script`] says, and without one, or if the script does not give up, runs until stopped.
pub(crate) fn watch(w: &watch::Watch, stop: &AtomicBool, emit: &mut dyn FnMut(Line)) -> Result<(), watch::WatchError> {
    let script = {
        let mut scripts = lock(&SCRIPTS);
        scripts.iter().position(|s| s.target == w.target).map(|i| scripts.remove(i))
    };
    if let Some(script) = script {
        script.lines.into_iter().for_each(&mut *emit);
        if let Some(why) = script.give_up {
            return Err(watch::WatchError::Other(anyhow::anyhow!(why)));
        }
    }
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// A web server with these files; says what it was asked for. Answers until the tests end.
pub(crate) fn web_server(files: Vec<(&'static str, Vec<u8>)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = Arc::clone(&asked);
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            use std::io::{BufRead, Write};
            let mut line = String::new();
            let mut reader = std::io::BufReader::new(&conn);
            reader.read_line(&mut line).unwrap();
            // The headers and the body, read so that closing does not cut off the answer.
            let (mut header, mut length) = (String::new(), 0);
            while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                if let Some(n) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = n.trim().parse().unwrap_or(0);
                }
                header.clear();
            }
            let _ = std::io::Read::read_exact(&mut reader, &mut vec![0; length]);
            let path = line.split_whitespace().nth(1).unwrap_or_default().to_string();
            let found = files.iter().find(|(name, _)| *name == path).map(|(_, body)| body);
            lock(&log).push(path);
            let head = found.map_or_else(
                || "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                |body| format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()),
            );
            let _ = conn.write_all(head.as_bytes());
            let _ = conn.write_all(found.map_or(&[][..], |body| body));
        }
    });
    (base, asked)
}

pub(crate) fn app(matches: Matches) -> App {
    App {
        matches,
        cache: PathBuf::from("unused"),
        backpack: PathBuf::from("unused"),
        maps: Mutex::default(),
        overviews: Mutex::new(()),
        base: String::new(),
        keys: None,
        home: None,
        streams: Mutex::default(),
        rooms: rooms::registry::Rooms::default(),
        voice_key: None,
        voice_links: web::voice_link::Links::default(),
        voice_bot: None,
    }
}

pub(crate) fn server(map: &str) -> Line {
    Line::Server(output::Server {
        tick: 0,
        map: map.to_string(),
        hostname: "SourceTV".to_string(),
        tick_interval: 0.015,
        max_players: 25,
        spawn_count: 1,
    })
}
