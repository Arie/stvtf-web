//! Who is asking: the visitor's address behind our proxy and Cloudflare, and who a start counts
//! against.

use super::token;
use crate::matches::sessions;
use crate::rooms::people::steam_key;
use std::net::IpAddr;

/// Only a proxy on our own network (kamal-proxy) is believed about who the client is.
pub(super) fn on_our_network(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        v4 @ IpAddr::V4(_) => !sessions::is_public(v4),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
    }
}

/// Who is asking: walking `X-Forwarded-For` back from whoever connected, the first address that
/// is neither ours nor Cloudflare's, as serveme does. Each hop wrote the one before it, so only
/// a trusted hop's word is taken.
pub(super) fn client_of(forwarded: &[IpAddr], peer: IpAddr) -> IpAddr {
    let mut client = peer;
    for &hop in forwarded.iter().rev() {
        if !trusted(client) {
            break;
        }
        client = hop;
    }
    client_key(client)
}

/// Our proxy, or Cloudflare's edge in front of it.
pub(super) fn trusted(ip: IpAddr) -> bool {
    on_our_network(ip) || is_cloudflare(ip)
}

/// Cloudflare's addresses, from <https://www.cloudflare.com/ips/> (2026-11; serveme has the same).
pub(super) const CLOUDFLARE: [&str; 22] = [
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
    "2400:cb00::/32",
    "2606:4700::/32",
    "2803:f800::/32",
    "2405:b500::/32",
    "2405:8100::/32",
    "2a06:98c0::/29",
    "2c0f:f248::/32",
];

pub(super) fn is_cloudflare(ip: IpAddr) -> bool {
    let bits = |ip: IpAddr| match ip.to_canonical() {
        IpAddr::V4(v4) => (u128::from(v4.to_bits()), 32),
        IpAddr::V6(v6) => (v6.to_bits(), 128),
    };
    let (ip, width) = bits(ip);
    CLOUDFLARE.iter().any(|range| {
        let Some((net, len)) = range.split_once('/') else { return false };
        let (Ok(net), Ok(len)) = (net.parse::<IpAddr>(), len.parse::<u32>()) else { return false };
        let (net, net_width) = bits(net);
        net_width == width && len <= width && (ip ^ net) >> (width - len) == 0
    })
}

/// What a client is counted by: its address, or for IPv6 its /64, which one user usually has all
/// of.
pub(super) fn client_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6((v6.to_bits() & !u128::from(u64::MAX)).into()),
        v4 @ IpAddr::V4(_) => v4,
    }
}

/// Who a start counts against, for the limits per person: a Steam id (`steam`, else the
/// signed-in user's), else the client (see [`client_of`]); nobody for a streamer or more, or on
/// this machine.
pub(super) fn counted(user: Option<&token::User>, steam: Option<&str>, client: IpAddr) -> Option<String> {
    if user.is_some_and(|u| u.streamer) || client.to_canonical().is_loopback() {
        return None;
    }
    if let Some(id) = steam.or(user.map(|u| u.uid.as_str())) {
        return Some(steam_key(id));
    }
    Some(format!("ip:{client}"))
}
