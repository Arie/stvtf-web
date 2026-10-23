//! The command line and connect strings.

use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;
use std::path::PathBuf;

/// The `tv_password` tried when a connect string has none.
pub(crate) const DEFAULT_TV_PASSWORD: &str = "tv";
/// `SourceTV`'s port when a connect string has none.
const DEFAULT_TV_PORT: u16 = 27020;

pub(crate) const USAGE: &str = "\
stvtf-web: live top-down viewer of a TF2 match in the browser

  stvtf-web HOST:TVPORT [--password tv] [--listen 127.0.0.1:8080] [--hz 20] [--cache DIR] [--backpack DIR]
  stvtf-web HOST[:PORT] [tv] / 'connect HOST:PORT; password \"tv\"'
  stvtf-web [--public] [--max-matches 20] [--base-path /live] [--demo MATCH.jsonl[.gz]] [--demos DIR] [--sessions DIR]   (watch any server: paste a connect string on the page)
     With STVTF_TOKEN_KEY (64 hex digits) set, matches start only from a site's encrypted /start?t= links;
     --home URL then sends visitors of / to that site.
  stvtf-web --replay FILE.jsonl [--listen 127.0.0.1:8080]   (stvtf output, at game speed)";
const FLAGS: [&str; 12] =
    ["password", "listen", "hz", "cache", "backpack", "replay", "max-matches", "base-path", "demo", "demos", "sessions", "home"];
const SWITCHES: [&str; 1] = ["public"];
const DEFAULT_LISTEN: &str = "127.0.0.1:8080";
const DEFAULT_MAX_MATCHES: usize = 20;
const DEFAULT_CACHE: &str = "cache";
const DEFAULT_BACKPACK: &str = "backpack";

/// The command line's options, or their defaults.
pub(crate) struct Options {
    /// The server to watch, as given (`HOST:PORT tv`, a connect string): one match at `/`.
    pub(crate) connect: Option<String>,
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
    pub(crate) home: Option<String>,
    pub(crate) public: bool,
}

impl Options {
    /// The options in `args`; None when they ask for help.
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>> {
        let Some(Args { positional, mut flags }) = parse_args(args)? else { return Ok(None) };
        let mut take = |name: &str| flags.remove(name);
        let base_path = take("base-path").unwrap_or_default().trim_end_matches('/').to_string();
        if !base_path.is_empty() && !base_path.starts_with('/') {
            bail!("--base-path {base_path}: must start with /");
        }
        Ok(Some(Options {
            connect: (!positional.is_empty()).then(|| positional.join(" ")),
            password: take("password"),
            listen: take("listen").unwrap_or_else(|| DEFAULT_LISTEN.into()),
            hz: take("hz").map_or(Ok(stvtf::DEFAULT_SNAPSHOT_HZ), |hz| stvtf::parse_snapshot_hz(&hz))?,
            cache: take("cache").unwrap_or_else(|| DEFAULT_CACHE.into()).into(),
            backpack: take("backpack").unwrap_or_else(|| DEFAULT_BACKPACK.into()).into(),
            replay: take("replay").map(PathBuf::from),
            max_matches: take("max-matches")
                .map(|n| n.parse().with_context(|| format!("--max-matches {n}: not a number")))
                .transpose()?
                .unwrap_or(DEFAULT_MAX_MATCHES),
            base_path,
            demo: take("demo").map(PathBuf::from),
            demos: take("demos").map(PathBuf::from),
            sessions: take("sessions").map(PathBuf::from),
            home: take("home"),
            public: take("public").is_some(),
        }))
    }
}

/// The command line as given: positional arguments and options by name.
struct Args {
    positional: Vec<String>,
    flags: HashMap<String, String>,
}

/// Positional arguments and `--name value` (or `--name=value`) options; None for `--help`. A
/// mistyped or repeated option is an error rather than silently falling back to its default.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>> {
    let (mut positional, mut flags) = (Vec::new(), HashMap::new());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "-h" || a == "--help" {
            return Ok(None);
        }
        let Some(option) = a.strip_prefix("--") else {
            positional.push(a);
            continue;
        };
        let (name, inline) = option.split_once('=').map_or((option, None), |(n, v)| (n, Some(v.to_string())));
        let value = if FLAGS.contains(&name) {
            match inline {
                Some(v) => v,
                None => it.next().ok_or_else(|| anyhow!("--{name} needs a value"))?,
            }
        } else if SWITCHES.contains(&name) && inline.is_none() {
            String::new()
        } else {
            bail!("unknown option --{option}\n\n{USAGE}");
        };
        if flags.insert(name.to_string(), value).is_some() {
            bail!("--{name} given twice");
        }
    }
    Ok(Some(Args { positional, flags }))
}

/// Liberal in what it takes: `connect HOST:PORT; password "tv"`, `HOST:PORT tv`, `HOST; password tv`
/// or just `HOST`. Without a port, `SourceTV`'s default 27020; quotes and `connect` are optional.
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

    fn options(a: &[&str]) -> Result<Option<Options>> {
        Options::parse(a.iter().map(ToString::to_string))
    }

    #[test]
    fn parses_options() {
        let o = options(&["host:27020", "--listen", "0.0.0.0:80", "--hz=10", "--public"]).unwrap().unwrap();
        assert_eq!(o.connect.as_deref(), Some("host:27020"));
        assert_eq!((o.listen.as_str(), o.hz, o.public), ("0.0.0.0:80", 10.0, true));
        let defaults = options(&[]).unwrap().unwrap();
        assert_eq!((defaults.listen.as_str(), defaults.max_matches, defaults.public), (DEFAULT_LISTEN, DEFAULT_MAX_MATCHES, false));
        assert_eq!(options(&["--base-path", "/live/"]).unwrap().unwrap().base_path, "/live");
        assert!(options(&["--help"]).unwrap().is_none());
    }

    #[test]
    fn unknown_repeated_or_incomplete_options_are_errors() {
        assert!(options(&["host:27020", "--lisen", "0.0.0.0:80"]).is_err());
        assert!(options(&["host:27020", "--listen"]).is_err());
        assert!(options(&["--hz", "1", "--hz", "2"]).is_err());
        assert!(options(&["--hz", "-1"]).is_err());
        assert!(options(&["--public=yes"]).is_err());
        assert!(options(&["--base-path", "live"]).is_err());
    }

    #[test]
    fn the_usage_names_every_option_and_default() {
        for name in FLAGS.iter().chain(&SWITCHES) {
            assert!(USAGE.contains(&format!("--{name}")), "--{name}");
        }
        assert!(USAGE.contains(DEFAULT_LISTEN) && USAGE.contains(&format!("--max-matches {DEFAULT_MAX_MATCHES}")));
    }
}
