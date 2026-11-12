//! Discord users and the Steam accounts they linked, as Discord says, kept in the store.

use super::ids::is_snowflake;
use crate::lock;
use crate::maps::cache::write_atomic;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// How long Discord gets to answer.
const TIMEOUT: Duration = Duration::from_secs(10);
/// What Steam's 64-bit ids of people start from.
const STEAM64_BASE: u64 = 76_561_197_960_265_728;
const FILE: &str = "voice-links.json";

pub(crate) struct Discord {
    pub(crate) api: String,
    pub(crate) client_id: String,
    secret: String,
    /// Where Discord sends the browser back: our `/voice/linked`, as its application has it.
    pub(crate) redirect: String,
}

/// Discord ids and the Steam ids (`[U:1:N]`) linked to them.
#[derive(Default)]
pub(crate) struct Links {
    pub(crate) discord: Option<Discord>,
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

    pub(crate) fn link(&self, discord: String, steamid: String) {
        let mut known = lock(&self.known);
        known.insert(discord, steamid);
        if let Some(file) = &self.file
            && let Err(e) = write_atomic(file, serde_json::json!(*known).to_string().as_bytes())
        {
            log!("voice links not kept: {e}");
        }
    }
}

/// `[U:1:N]` of a 64-bit Steam id of a person.
fn steam3(id64: &str) -> Option<String> {
    let account = u32::try_from(id64.parse::<u64>().ok()?.checked_sub(STEAM64_BASE)?).ok()?;
    Some(format!("[U:1:{account}]"))
}

/// The Discord user `code` is of, and the Steam account Discord verified as theirs, if any.
pub(crate) fn ask_discord(d: &Discord, code: &str) -> Result<(String, Option<String>)> {
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
