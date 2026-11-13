//! The list of who is who between regions (`voice::known`): its routes, and asking a site.

use super::token::Keys;
use super::voice_in::is_key;
use super::{JSON, Request, TEXT, json_body, param, respond};
use crate::App;
use crate::voice::ids::{is_voice_id, steamid_of};
use crate::voice::known::{CASTER, MAX_NAME, Said, Site, TIMEOUT};
use anyhow::Context;
use std::io::{BufReader, Take};
use std::net::TcpStream;
use std::sync::Arc;

/// The site at `home` (as a stv.tf's `--home`), asked with tokens sealed with `keys`.
pub(crate) fn site(home: &str, keys: Arc<Keys>) -> Site {
    let url = format!("{}/discord_links/", home.trim_end_matches('/'));
    let name = home.split("://").last().unwrap_or(home).split('/').next().unwrap_or_default().to_string();
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build();
    let ask = move |discord: &str| {
        let bearer = format!("Bearer {}", keys.link_token(discord, super::unix_now()));
        match agent.get(&format!("{url}{discord}")).set("Authorization", &bearer).call() {
            Ok(answer) => {
                let said: serde_json::Value = serde_json::from_str(&answer.into_string()?)?;
                Ok(said["steam"].as_str().map(str::to_string))
            }
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(e) => Err(e).context("a site's Discord link"),
        }
    };
    Site { name, ask: Box::new(ask) }
}

/// `GET /voice/known?since=<version>`, another stv.tf's: what changed since.
pub(super) fn known(s: &mut TcpStream, app: &App, request: &Request) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let since = param(&request.query, "since").and_then(|v| v.parse().ok()).unwrap_or(0);
    let (version, known) = app.voice_who.since(since);
    respond(s, "200 OK", JSON, serde_json::json!({ "version": version, "known": known }).to_string().as_bytes())
}

/// `POST /voice/known`, another stv.tf's: what was said on a page there.
pub(super) fn learn(s: &mut TcpStream, reader: &mut BufReader<Take<TcpStream>>, app: &App, request: &Request) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let Ok(said) = json_body::<Said>(reader, request) else { return respond(s, "400 Bad Request", TEXT, b"Invalid") };
    let steamid = said.steamid.as_deref().map(steamid_of);
    if !is_voice_id(&said.id) || steamid == Some(None) {
        return respond(s, "400 Bad Request", TEXT, b"Invalid");
    }
    let name: String = said.name.chars().filter(|c| !c.is_control()).collect();
    if said.src.len() > MAX_NAME || !said.src.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':')) {
        return respond(s, "400 Bad Request", TEXT, b"Invalid");
    }
    if !matches!(said.by.as_str(), "" | CASTER) {
        return respond(s, "400 Bad Request", TEXT, b"Invalid");
    }
    app.voice_who.take_from_region(&said.id, steamid.flatten().as_deref(), &name, &said.src, &said.by);
    respond(s, "204 No Content", TEXT, b"")
}
