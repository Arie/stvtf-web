//! The real program, started as a user would. `--replay` needs no network; without an address it
//! would start building the most played maps.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const RECORDING: &str =
    r#"{"type":"server","tick":5,"map":"cp_badlands","hostname":"SourceTV","tick_interval":0.015,"max_players":25,"spawn_count":1}"#;

/// The server, stopped when dropped, also when a test fails.
struct Server {
    child: Child,
    address: String,
    dir: std::path::PathBuf,
}

impl Server {
    fn start(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("stvtf-web-smoke-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let recording = dir.join("match.jsonl");
        std::fs::write(&recording, format!("{RECORDING}\n")).unwrap();
        let address = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        let child = Command::new(env!("CARGO_BIN_EXE_stvtf-web"))
            .args(["--replay".as_ref(), recording.as_os_str()])
            .args(["--listen", &address])
            .args(["--cache".as_ref(), dir.as_os_str()])
            .args(["--backpack".as_ref(), dir.as_os_str()])
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let server = Server { child, address, dir };
        let deadline = Instant::now() + Duration::from_secs(10);
        // Only a real answer counts: connecting to a free port can succeed, to itself.
        while !server.try_get("/up").is_some_and(|r| r.starts_with("HTTP/1.1 200") && r.ends_with("ok")) {
            assert!(Instant::now() < deadline, "the server did not start answering");
            sleep(Duration::from_millis(10));
        }
        server
    }

    fn connect(&self, path: &str) -> std::io::Result<TcpStream> {
        let mut s = TcpStream::connect(&self.address)?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        write!(s, "GET {path} HTTP/1.1\r\nHost: test\r\n\r\n")?;
        Ok(s)
    }

    fn try_get(&self, path: &str) -> Option<String> {
        let mut response = String::new();
        self.connect(path).ok()?.read_to_string(&mut response).ok()?;
        Some(response)
    }

    fn get(&self, path: &str) -> String {
        self.try_get(path).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_viewer_gets_the_page_its_modules_and_the_match() {
    let server = Server::start("viewer");
    let page = server.get("/");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    // The page asks for its code under the build's version, which the server must then have.
    let main = page.split('"').find(|s| s.starts_with("app/") && s.ends_with("/main.js")).unwrap_or_else(|| panic!("{page}"));
    assert!(!main.contains("/dev/"), "{main}");
    let module = server.get(&format!("/{main}"));
    assert!(module.starts_with("HTTP/1.1 200") && module.contains("text/javascript"), "{module}");

    let mut events = BufReader::new(server.connect("/events").unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no match on the stream");
        let mut line = String::new();
        assert!(events.read_line(&mut line).unwrap() > 0, "the stream ended");
        if line.starts_with("data: ") && line.contains(r#""map":"cp_badlands""#) {
            break;
        }
    }
}

#[test]
fn the_command_line_is_checked_before_serving() {
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_stvtf-web")).args(args).output().unwrap();
    let help = run(&["--help"]);
    assert!(help.status.success() && String::from_utf8_lossy(&help.stdout).contains("--replay"));
    assert!(!run(&["--no-such-option"]).status.success());
}
