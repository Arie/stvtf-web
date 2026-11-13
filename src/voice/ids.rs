//! Who someone is where they talk, and on Steam, as given.

/// What Steam's 64-bit ids of people start from.
pub(crate) const STEAM64_BASE: u64 = 76_561_197_960_265_728;

/// A Discord id: digits.
pub(crate) fn is_snowflake(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}

/// The account of `SteamID` `[U:1:<account>]`.
pub(crate) fn account_of(steamid: &str) -> Option<u32> {
    steamid.strip_prefix("[U:1:")?.strip_suffix(']')?.parse().ok()
}

/// Who someone is where they talk, or the channel they are in there: a Discord id, or for
/// Mumble what the bot makes of the server and who they are on it (`mumble:<server>:<user>`).
pub(crate) fn is_voice_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b':' | b'_' | b'-'))
}

/// A `SteamID` as people write them, as `[U:1:N]`: that itself, `STEAM_X:Y:Z`, the 64-bit number,
/// or a profile link with that number in it.
pub(crate) fn parse_steamid(text: &str) -> Option<String> {
    let t = text.trim().trim_end_matches('/');
    let account = if let Some(n) = t.strip_prefix("[U:1:").and_then(|r| r.strip_suffix(']')).or_else(|| t.strip_prefix("U:1:")) {
        n.parse::<u32>().ok()?
    } else if let Some(rest) = t.to_ascii_uppercase().strip_prefix("STEAM_") {
        let mut parts = rest.split(':').map(str::parse::<u32>);
        let (_, y, z) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
        if y > 1 || parts.next().is_some() {
            return None;
        }
        z.checked_mul(2)?.checked_add(y)?
    } else {
        let digits = t.rsplit('/').next()?;
        u32::try_from(digits.parse::<u64>().ok()?.checked_sub(STEAM64_BASE)?).ok()?
    };
    (account > 0).then(|| format!("[U:1:{account}]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steam_id_is_read_as_people_write_them() {
        for same in [
            "[U:1:9]",
            "U:1:9",
            "STEAM_0:1:4",
            "steam_1:1:4",
            " 76561197960265737 ",
            "https://steamcommunity.com/profiles/76561197960265737/",
        ] {
            assert_eq!(parse_steamid(same).as_deref(), Some("[U:1:9]"), "{same}");
        }
        for none in [
            "",
            "gaben",
            "https://steamcommunity.com/id/gaben",
            "STEAM_0:2:4",
            "STEAM_0:1",
            "[U:1:x]",
            "76561197960265728",
            "123",
            "[U:1:0]",
        ] {
            assert_eq!(parse_steamid(none), None, "{none}");
        }
    }
}
