//! The command line and connect strings.

use std::ops::RangeInclusive;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Parser;

/// The `tv_password` tried when a connect string has none.
pub(crate) const DEFAULT_TV_PASSWORD: &str = "tv";
/// `SourceTV`'s port when a connect string has none.
const DEFAULT_TV_PORT: u16 = 27020;

/// Live top-down viewer of a TF2 match in the browser.
#[derive(Parser)]
#[command(
    name = "stvtf-web",
    version,
    after_help = "\
Given a server (HOST:TVPORT [tv], or 'connect HOST:PORT; password \"tv\"'), one match at /; without one,
any server: paste a connect string on the page.
With STVTF_VOICE_KEY set, a voice bot posts a team's voice to live matches with it (POST /w/<id>/voice).
With STVTF_TOKEN_KEY (64 hex digits) set, matches start from a site's encrypted /start?t= links, and
from / for whoever signed in at that site (--home URL)."
)]
struct Cli {
    /// The server to watch, as a connect string.
    #[arg(value_name = "CONNECT")]
    connect: Vec<String>,
    /// Its `tv_password`, over the connect string's.
    #[arg(long)]
    password: Option<String>,
    /// Where pages are served.
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,
    /// Snapshots a second (0: none).
    #[arg(long, default_value_t = stvtf::DEFAULT_SNAPSHOT_HZ, value_parser = stvtf::parse_snapshot_hz)]
    hz: f32,
    /// Where map geometry and overviews are kept.
    #[arg(long, value_name = "DIR", default_value = "cache")]
    cache: PathBuf,
    /// Where backpack images are kept.
    #[arg(long, value_name = "DIR", default_value = "backpack")]
    backpack: PathBuf,
    /// stvtf output to play at game speed, instead of a server.
    #[arg(long, value_name = "FILE.jsonl")]
    replay: Option<PathBuf>,
    /// How many matches at once.
    #[arg(long, default_value_t = 20)]
    max_matches: usize,
    /// Where a proxy serves us, like `/live`.
    #[arg(long, value_name = "PATH", default_value = "", value_parser = base_path)]
    base_path: String,
    /// A match to show on the landing page.
    #[arg(long, value_name = "MATCH.jsonl[.gz]")]
    demo: Option<PathBuf>,
    /// Saved matches, one each.
    #[arg(long, value_name = "DIR")]
    demos: Option<PathBuf>,
    /// Where matches are kept across restarts (and live demos recorded).
    #[arg(long, value_name = "DIR")]
    sessions: Option<PathBuf>,
    /// Where what the server says of itself is kept too, besides stderr.
    #[arg(long, value_name = "DIR")]
    logs: Option<PathBuf>,
    /// The site matches start from (`/start?t=` links, sign-in).
    #[arg(long, value_name = "URL")]
    home: Option<String>,
    /// Watch any server: a connect string pasted on the page.
    #[arg(long)]
    public: bool,
    /// Leave building the most played maps to their first viewers.
    #[arg(long)]
    no_warm: bool,
    /// Each room is also a `SourceTV` address for TF2 clients, on a port of this range.
    #[arg(long, value_name = "A-B", value_parser = port_range, requires = "relay_host")]
    relay_ports: Option<RangeInclusive<u16>>,
    /// The host of the rooms' `SourceTV` addresses.
    #[arg(long, value_name = "HOST", requires = "relay_ports")]
    relay_host: Option<String>,
    /// The relays run in a process of their own, on this socket (deploys leave them be).
    #[arg(long, value_name = "SOCKET", conflicts_with = "relay_ports")]
    relay_socket: Option<PathBuf>,
    /// Be that process, on this socket.
    #[arg(long, value_name = "SOCKET", requires = "relay_ports")]
    relay_daemon: Option<PathBuf>,
    /// Say how many TF2 clients the relay process on this socket has.
    #[arg(long, value_name = "SOCKET")]
    relay_clients: Option<PathBuf>,
}

/// The command line's options, or their defaults.
pub(crate) struct Options {
    /// The server to watch, and the password its connect string gives: one match at `/`.
    pub(crate) connect: Option<(String, Option<String>)>,
    pub(crate) password: Option<String>,
    pub(crate) listen: String,
    pub(crate) hz: f32,
    pub(crate) cache: PathBuf,
    pub(crate) backpack: PathBuf,
    pub(crate) replay: Option<PathBuf>,
    pub(crate) max_matches: usize,
    /// Where a proxy serves us, like `/live`, without a trailing `/`; empty at the root.
    pub(crate) base_path: String,
    pub(crate) demo: Option<PathBuf>,
    pub(crate) demos: Option<PathBuf>,
    pub(crate) sessions: Option<PathBuf>,
    /// Where what the server says of itself is kept too, besides stderr.
    pub(crate) logs: Option<PathBuf>,
    pub(crate) home: Option<String>,
    pub(crate) public: bool,
    /// Leave building the most played maps to their first viewers.
    pub(crate) no_warm: bool,
    /// Rooms relayed to TF2 clients: on these ports, at this host.
    pub(crate) relays: Option<(String, RangeInclusive<u16>)>,
    /// The relay process to ask for rooms' relays, instead of running them here.
    pub(crate) relay_socket: Option<PathBuf>,
    /// Be that process, on this socket.
    pub(crate) relay_daemon: Option<PathBuf>,
    /// Ask the relay process on this socket how many TF2 clients it has, and say.
    pub(crate) relay_clients: Option<PathBuf>,
}

impl Options {
    /// The options in `args` (without the program's name).
    pub(crate) fn parse_from(args: impl IntoIterator<Item = String>) -> Result<Self, clap::Error> {
        let cli = Cli::try_parse_from(std::iter::once("stvtf-web".to_string()).chain(args))?;
        let connect =
            if cli.connect.is_empty() {
                None
            } else {
                let line = cli.connect.join(" ");
                let parsed = parse_connect(&line);
                Some(parsed.ok_or_else(|| {
                    clap::Error::raw(clap::error::ErrorKind::ValueValidation, format!("cannot read a server in {line:?}\n"))
                })?)
            };
        Ok(Options {
            connect,
            password: cli.password,
            listen: cli.listen,
            hz: cli.hz,
            cache: cli.cache,
            backpack: cli.backpack,
            replay: cli.replay,
            max_matches: cli.max_matches,
            base_path: cli.base_path,
            demo: cli.demo,
            demos: cli.demos,
            sessions: cli.sessions,
            logs: cli.logs,
            home: cli.home,
            public: cli.public,
            no_warm: cli.no_warm,
            relays: cli.relay_host.zip(cli.relay_ports),
            relay_socket: cli.relay_socket,
            relay_daemon: cli.relay_daemon,
            relay_clients: cli.relay_clients,
        })
    }
}

/// A base path like `/live`, without its trailing `/`.
fn base_path(s: &str) -> Result<String> {
    let path = s.trim_end_matches('/');
    if !path.is_empty() && !path.starts_with('/') {
        bail!("cannot serve under a path that does not start with /");
    }
    Ok(path.to_string())
}

/// `A-B`: ports A to B.
fn port_range(s: &str) -> Result<RangeInclusive<u16>> {
    let range = s.split_once('-').and_then(|(a, b)| Some((a.trim().parse::<u16>().ok()?, b.trim().parse::<u16>().ok()?)));
    match range {
        Some((a, b)) if a != 0 && a <= b => Ok(a..=b),
        _ => bail!("cannot read a range of ports like 27100-27199"),
    }
}

/// Liberal in what it takes: `connect HOST:PORT; password "tv"`, `HOST:PORT tv`, `HOST; password
/// tv` or just `HOST`. Without a port, `SourceTV`'s default 27020; quotes and `connect` are
/// optional.
pub(crate) fn parse_connect(line: &str) -> Option<(String, Option<String>)> {
    const CONNECT: &str = "connect ";
    let mut line = line.trim();
    if line.get(..CONNECT.len()).is_some_and(|c| c.eq_ignore_ascii_case(CONNECT)) {
        line = line[CONNECT.len()..].trim_start();
    }
    let unquote = |s: &str| s.trim().trim_matches('"').to_string();
    let mut parts = line.split(';');
    let first = parts.next()?.trim();
    let (host, rest) = first.split_once(char::is_whitespace).unwrap_or((first, ""));
    let mut password = Some(unquote(rest.trim().strip_prefix("password").unwrap_or(rest)));
    for p in parts {
        if let Some(pw) = p.trim().strip_prefix("password") {
            password = Some(unquote(pw));
        }
    }
    let host = host.trim_matches('"');
    if host.is_empty() {
        return None;
    }
    let target = if host.contains(':') { host.to_string() } else { format!("{host}:{DEFAULT_TV_PORT}") };
    Some((target, password.filter(|p| !p.is_empty())))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn parses_connect_strings() {
        let some = |t: &str, p: Option<&str>| Some((t.to_string(), p.map(str::to_string)));
        let tv = |t: &str| some(t, Some("tv"));
        assert_eq!(parse_connect("connect 1.2.3.4:27015; password \"tv\""), tv("1.2.3.4:27015"));
        assert_eq!(parse_connect("CONNECT 1.2.3.4:27015;password tv\n"), tv("1.2.3.4:27015"));
        assert_eq!(parse_connect("1.2.3.4:27015 tv"), tv("1.2.3.4:27015"));
        assert_eq!(parse_connect("1.2.3.4; password tv"), tv("1.2.3.4:27020"));
        assert_eq!(parse_connect("bolus.fakkelbrigade.eu tv"), tv("bolus.fakkelbrigade.eu:27020"));
        assert_eq!(parse_connect("bolus.fakkelbrigade.eu:27015 tv"), tv("bolus.fakkelbrigade.eu:27015"));
        assert_eq!(parse_connect("bolus.fakkelbrigade.eu:27015; password tv"), tv("bolus.fakkelbrigade.eu:27015"));
        assert_eq!(parse_connect("1.2.3.4:27015 password tv"), tv("1.2.3.4:27015"));
        assert_eq!(parse_connect("connect 1.2.3.4:27020; password \"Floofy Goat\""), some("1.2.3.4:27020", Some("Floofy Goat")));
        assert_eq!(parse_connect("1.2.3.4:27020"), some("1.2.3.4:27020", None));
        assert_eq!(parse_connect("  \n"), None);
    }

    fn options(a: &[&str]) -> Result<Options, clap::Error> {
        Options::parse_from(a.iter().map(ToString::to_string))
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_options() {
        let o = options(&["host:27020", "--listen", "0.0.0.0:80", "--hz=10", "--public"]).unwrap();
        assert_eq!(o.connect, Some(("host:27020".to_string(), None)));
        assert_eq!((o.listen.as_str(), o.hz, o.public), ("0.0.0.0:80", 10.0, true));
        let words = options(&["connect", "host:27015;", "password", "\"tv\""]).unwrap();
        assert_eq!(words.connect, Some(("host:27015".to_string(), Some("tv".to_string()))));
        let defaults = options(&[]).unwrap();
        assert_eq!((defaults.listen.as_str(), defaults.max_matches, defaults.public), ("127.0.0.1:8080", 20, false));
        assert_eq!(options(&["--base-path", "/live/"]).unwrap().base_path, "/live");
        assert!(!defaults.no_warm && options(&["--no-warm"]).unwrap().no_warm);
        let relayed = options(&["--relay-ports", "27100-27109", "--relay-host", "stv.tf"]).unwrap();
        assert_eq!(relayed.relays, Some(("stv.tf".to_string(), 27100..=27109)));
        assert!(defaults.relays.is_none());
        let daemon = options(&["--relay-daemon", "/data/r.sock", "--relay-ports", "28100-28101", "--relay-host", "h"]).unwrap();
        assert_eq!(daemon.relay_daemon, Some(PathBuf::from("/data/r.sock")));
        assert_eq!(options(&["--relay-clients", "/r.sock"]).unwrap().relay_clients, Some(PathBuf::from("/r.sock")));
        assert_eq!(options(&["--relay-socket", "/data/r.sock"]).unwrap().relay_socket, Some(PathBuf::from("/data/r.sock")));
    }

    #[test]
    fn unknown_repeated_or_incomplete_options_are_errors() {
        assert!(options(&["host:27020", "--lisen", "0.0.0.0:80"]).is_err());
        assert!(options(&["host:27020", "--listen"]).is_err());
        assert!(options(&["--hz", "1", "--hz", "2"]).is_err());
        assert!(options(&["--hz", "-1"]).is_err());
        assert!(options(&["--public=yes"]).is_err());
        assert!(options(&["--base-path", "live"]).is_err());
        assert!(options(&["--relay-ports", "27100-27109"]).is_err(), "without a host");
        assert!(options(&["--relay-daemon", "/r.sock"]).is_err(), "a relay process without ports");
        assert!(options(&["--relay-socket", "/r.sock", "--relay-ports", "1-2", "--relay-host", "h"]).is_err(), "both");
        for bad in ["27109-27100", "27100", "a-b", "0-5", "1-70000"] {
            assert!(options(&["--relay-ports", bad, "--relay-host", "h"]).is_err(), "{bad}");
        }
        assert!(options(&["--help"]).is_err_and(|e| e.kind() == clap::error::ErrorKind::DisplayHelp));
    }

    #[test]
    fn every_production_command_line_parses() {
        let web = "--public --max-matches 5 --demo /data/demo.jsonl.gz --sessions /data/sessions --logs /data/logs --home https://au.serveme.tf/stv --relay-socket /data/relay.sock";
        let relays = "--logs /data/logs --relay-daemon /data/relay.sock --relay-ports 28100-28103 --relay-host direct.au.serveme.tf";
        for line in [web, relays] {
            assert!(options(&line.split(' ').collect::<Vec<_>>()).is_ok(), "{line}");
        }
    }
}
