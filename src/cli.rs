//! The command line and connect strings.

use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;

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

/// Positional arguments and `--name value` options; a mistyped option is an error rather than
/// silently falling back to its default.
pub(crate) fn parse_args(args: impl IntoIterator<Item = String>) -> Result<(Vec<String>, HashMap<String, String>)> {
    let (mut positional, mut flags) = (Vec::new(), HashMap::new());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.strip_prefix("--") {
            Some(name) if FLAGS.contains(&name) => {
                let value = it.next().ok_or_else(|| anyhow!("--{name} needs a value"))?;
                flags.insert(name.to_string(), value);
            }
            Some(name) if SWITCHES.contains(&name) => {
                flags.insert(name.to_string(), String::new());
            }
            Some(name) => bail!("unknown option --{name}\n\n{USAGE}"),
            None => positional.push(a),
        }
    }
    Ok((positional, flags))
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

    fn args(a: &[&str]) -> Result<(Vec<String>, HashMap<String, String>)> {
        parse_args(a.iter().map(ToString::to_string))
    }

    #[test]
    fn parses_options() {
        let (positional, flags) = args(&["host:27020", "--listen", "0.0.0.0:80", "--hz", "10"]).unwrap();
        assert_eq!(positional, ["host:27020"]);
        assert_eq!(flags["listen"], "0.0.0.0:80");
        assert_eq!(flags["hz"], "10");
    }

    #[test]
    fn unknown_or_incomplete_options_are_errors() {
        assert!(args(&["host:27020", "--lisen", "0.0.0.0:80"]).is_err());
        assert!(args(&["host:27020", "--listen"]).is_err());
    }
}
