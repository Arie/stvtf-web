//! Who a Discord user is on Steam, for voice from Discord: `GET /voice/link` signs them in with
//! Discord, which says which Steam account they connected to it (`/voice/linked`); kept, and told
//! to the voice bot (`GET /voice/who?discord=<id>`, with its key). Linking is off without a
//! Discord application (`STVTF_DISCORD_CLIENT_ID`, `_SECRET` and `_REDIRECT`).

use super::voice_in::{is_key, is_snowflake};
use super::{JSON, Request, TEXT, form_decode, param, respond, respond_with, set_cookie, url_encode};
use crate::maps::cache::write_atomic;
use crate::matches::sessions::random_id;
use crate::{App, lock};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The cookie that says which sign-in a browser began, and how long it has to end it.
pub(super) const STATE_COOKIE: &str = "voice_link";
const STATE_FOR: u64 = 600;
/// How long Discord gets to answer.
const TIMEOUT: Duration = Duration::from_secs(10);
/// What Steam's 64-bit ids of people start from.
const STEAM64_BASE: u64 = 76_561_197_960_265_728;
const FILE: &str = "voice-links.json";

struct Discord {
    api: String,
    client_id: String,
    secret: String,
    /// Where Discord sends the browser back: our `/voice/linked`, as its application has it.
    redirect: String,
}

/// Discord ids and the Steam ids (`[U:1:N]`) linked to them.
#[derive(Default)]
pub(crate) struct Links {
    discord: Option<Discord>,
    file: Option<PathBuf>,
    known: Mutex<HashMap<String, String>>,
}

impl Links {
    /// Those kept in `dir`, and the Discord application the environment names, if it does.
    pub(crate) fn from_env(dir: Option<&Path>) -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let discord = match (var("STVTF_DISCORD_CLIENT_ID"), var("STVTF_DISCORD_CLIENT_SECRET"), var("STVTF_DISCORD_REDIRECT")) {
            (Some(client_id), Some(secret), Some(redirect)) => {
                Some(Discord { api: "https://discord.com/api".into(), client_id, secret, redirect })
            }
            _ => None,
        };
        let file = dir.map(|d| d.join(FILE));
        let known = file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
        Self { discord, file, known: Mutex::new(known) }
    }

    #[cfg(test)]
    pub(crate) fn with_discord(api: &str, client_id: &str, secret: &str, redirect: &str) -> Self {
        let discord = Discord { api: api.into(), client_id: client_id.into(), secret: secret.into(), redirect: redirect.into() };
        Self { discord: Some(discord), ..Self::default() }
    }

    pub(crate) fn steamid(&self, discord: &str) -> Option<String> {
        lock(&self.known).get(discord).cloned()
    }

    fn link(&self, discord: String, steamid: String) {
        let mut known = lock(&self.known);
        known.insert(discord, steamid);
        if let Some(file) = &self.file
            && let Err(e) = write_atomic(file, serde_json::json!(*known).to_string().as_bytes())
        {
            eprintln!("stvtf-web: voice links not kept: {e}");
        }
    }
}

/// `[U:1:N]` of a 64-bit Steam id of a person.
fn steam3(id64: &str) -> Option<String> {
    let account = u32::try_from(id64.parse::<u64>().ok()?.checked_sub(STEAM64_BASE)?).ok()?;
    Some(format!("[U:1:{account}]"))
}

/// The Discord user `code` is of, and the Steam account Discord verified as theirs, if any.
fn ask_discord(d: &Discord, code: &str) -> Result<(String, Option<String>)> {
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let json = |r: ureq::Response| -> Result<serde_json::Value> { Ok(serde_json::from_str(&r.into_string()?)?) };
    let form = [
        ("client_id", d.client_id.as_str()),
        ("client_secret", d.secret.as_str()),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", d.redirect.as_str()),
    ];
    let token = json(agent.post(&format!("{}/oauth2/token", d.api)).send_form(&form).context("Discord refused the sign-in")?)?;
    let Some(token) = token["access_token"].as_str() else { bail!("Discord gave no token") };
    let get = |path: &str| json(agent.get(&format!("{}{path}", d.api)).set("Authorization", &format!("Bearer {token}")).call()?);
    let me = get("/users/@me")?;
    let Some(id) = me["id"].as_str().filter(|id| is_snowflake(id)) else { bail!("Discord did not say who this is") };
    let connections = get("/users/@me/connections")?;
    let steam = connections.as_array().into_iter().flatten().find(|c| c["type"] == "steam" && c["verified"] == true);
    Ok((id.to_string(), steam.and_then(|c| c["id"].as_str()).and_then(steam3)))
}

/// `/voice/link`: off to Discord, to come back to `/voice/linked`.
pub(super) fn link(s: &mut TcpStream, app: &App) -> std::io::Result<()> {
    let Some(d) = &app.voice_links.discord else { return respond(s, "404 Not Found", TEXT, b"No linking here") };
    let state = random_id();
    let to = format!(
        "{}/oauth2/authorize?response_type=code&client_id={}&scope=identify+connections&redirect_uri={}&state={state}",
        d.api,
        url_encode(&d.client_id),
        url_encode(&d.redirect),
    );
    respond_with(s, "302 Found", &[("Location", &to), ("Set-Cookie", &set_cookie(app, STATE_COOKIE, &state, STATE_FOR))], b"")
}

/// `/voice/linked`: back from Discord with a code, in the browser that set out.
pub(super) fn linked(s: &mut TcpStream, app: &App, request: &Request) -> std::io::Result<()> {
    let Some(d) = &app.voice_links.discord else { return respond(s, "404 Not Found", TEXT, b"No linking here") };
    let state = param(&request.query, "state").unwrap_or_default();
    let code = param(&request.query, "code").map(form_decode).unwrap_or_default();
    if state.is_empty() || code.is_empty() || request.voice_link.as_deref() != Some(state) {
        return respond(s, "400 Bad Request", TEXT, b"Expired. Try again.");
    }
    match ask_discord(d, &code) {
        Ok((discord, Some(steamid))) => {
            let done = format!("Linked to {steamid}.");
            app.voice_links.link(discord, steamid);
            respond(s, "200 OK", TEXT, done.as_bytes())
        }
        Ok((_, None)) => respond(s, "409 Conflict", TEXT, b"No Steam on your Discord. Add it under Connections."),
        Err(e) => {
            eprintln!("stvtf-web: voice link failed: {e:#}");
            respond(s, "502 Bad Gateway", TEXT, b"Discord error. Try again.")
        }
    }
}

/// `/voice/who?discord=<id>`: the Steam id linked to a Discord user, to the bot.
pub(super) fn who(s: &mut TcpStream, app: &App, request: &Request) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let Some(discord) = param(&request.query, "discord").filter(|d| is_snowflake(d)) else {
        return respond(s, "400 Bad Request", TEXT, b"a Discord id");
    };
    let who = serde_json::json!({ "steamid": app.voice_links.steamid(discord) });
    respond(s, "200 OK", JSON, who.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn links_are_kept_in_a_file_and_read_back() {
        let dir = TempDir::new("voicelinks");
        let links = Links::from_env(Some(&dir));
        assert_eq!(links.steamid("77"), None);
        links.link("77".into(), "[U:1:5]".into());
        assert_eq!(Links::from_env(Some(&dir)).steamid("77").as_deref(), Some("[U:1:5]"));
    }

    #[test]
    fn only_a_persons_steam_id_becomes_a_steam3() {
        assert_eq!(steam3("76561197960265733").as_deref(), Some("[U:1:5]"));
        assert_eq!(steam3("5"), None);
        assert_eq!(steam3("103582791429521408"), None, "a group");
        assert_eq!(steam3("x"), None);
    }
}
