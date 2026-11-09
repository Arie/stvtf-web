//! Which servers a public stv.tf may watch.

use std::net::{IpAddr, Ipv4Addr};

/// IPv4 ranges that are not on the internet and that `Ipv4Addr`'s own checks leave out: "this
/// network", carrier-grade NAT (Tailscale), IETF protocol assignments, benchmarking and reserved.
pub(super) const NOT_PUBLIC_V4: [(Ipv4Addr, u32); 5] = [
    (Ipv4Addr::UNSPECIFIED, 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// Whether `ip` is in the network `net`/`prefix`.
pub(super) fn in_network(ip: Ipv4Addr, (net, prefix): (Ipv4Addr, u32)) -> bool {
    ip.to_bits() >> (u32::BITS - prefix) == net.to_bits() >> (u32::BITS - prefix)
}

/// Not loopback, private, link-local, carrier-grade NAT (Tailscale), IETF protocol
/// assignments, benchmarking, multicast or reserved.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || NOT_PUBLIC_V4.into_iter().any(|net| in_network(v4, net)))
        }
        IpAddr::V6(_) => false,
    }
}
