//! Invite links for an embedded host (spec §7.3).

use std::net::{IpAddr, SocketAddr, UdpSocket};

use crate::link::{ServerAddress, parse_address};

/// `sppy://<public address or fallback ip>:<port>/<token>`.
/// `public_address` may be `host`, `host:port` or a full `sppy(s)://host[:port]` link.
pub fn invite_link(
    public_address: Option<&str>,
    port: u16,
    token: &str,
    fallback_ip: Option<IpAddr>,
) -> String {
    let public = public_address.map(str::trim).filter(|s| !s.is_empty());
    let mut addr = match public.and_then(|p| parse_address(p).ok()) {
        Some(mut a) => {
            let explicit_port = public.is_some_and(|p| {
                let authority = p.split("://").last().unwrap_or(p);
                authority.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok())
            });
            if !explicit_port {
                a.port = port;
            }
            a
        }
        None => ServerAddress {
            secure: false,
            host: fallback_ip.map_or_else(|| "127.0.0.1".to_owned(), |ip| ip.to_string()),
            port,
            token: None,
        },
    };
    addr.token = Some(token.to_owned());
    addr.to_link()
}

/// Where the local client should connect to reach a server listening on `listen`
/// (`0.0.0.0` / `::` are not connectable: use the matching loopback).
pub fn local_connect_addr(listen: std::net::SocketAddr) -> std::net::SocketAddr {
    use std::net::{Ipv4Addr, Ipv6Addr};
    let ip: IpAddr = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    };
    std::net::SocketAddr::new(ip, listen.port())
}

/// The address other machines most likely reach us at (no packets are sent).
pub fn first_non_loopback_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// An address of this machine on one of its interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIp {
    pub ip: IpAddr,
    pub interface: String,
}

/// What the host of an invite link can be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteHost {
    /// The configured public address (a name or `host:port`).
    Public(String),
    Local(LocalIp),
}

impl InviteHost {
    /// Stable key to remember the choice in the config.
    pub fn key(&self) -> String {
        match self {
            InviteHost::Public(p) => p.clone(),
            InviteHost::Local(l) => l.ip.to_string(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            InviteHost::Public(p) => format!("{p} (public address)"),
            InviteHost::Local(l) if l.ip.is_loopback() => format!("{} (this computer only)", l.ip),
            InviteHost::Local(l) if l.interface.is_empty() => l.ip.to_string(),
            InviteHost::Local(l) => format!("{} ({})", l.ip, l.interface),
        }
    }

    /// The invite link for a server on `port`.
    pub fn link(&self, port: u16, token: &str) -> String {
        match self {
            InviteHost::Public(p) => invite_link(Some(p), port, token, None),
            InviteHost::Local(l) => invite_link(None, port, token, Some(l.ip)),
        }
    }
}

/// Addresses of this machine a server listening on `listen` can be reached at, best guess first.
pub fn local_candidates(listen: SocketAddr) -> Vec<LocalIp> {
    use if_addrs::IfOperStatus;
    let ips = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| {
            !matches!(
                i.oper_status,
                IfOperStatus::Down | IfOperStatus::NotPresent | IfOperStatus::LowerLayerDown
            )
        })
        .map(|i| LocalIp { ip: i.ip(), interface: i.name })
        .collect();
    rank_candidates(ips, listen, first_non_loopback_ip())
}

/// Drop addresses the listener does not accept or a link cannot carry, then order them:
/// the default-route address, private and VPN IPv4, other IPv4, global IPv6,
/// unique-local IPv6, loopback last.
pub fn rank_candidates(
    ips: Vec<LocalIp>,
    listen: SocketAddr,
    default_route: Option<IpAddr>,
) -> Vec<LocalIp> {
    let bound = listen.ip();
    let mut out: Vec<LocalIp> = Vec::new();
    if !bound.is_unspecified() {
        let interface = ips.into_iter().find(|l| l.ip == bound).map(|l| l.interface);
        return vec![LocalIp { ip: bound, interface: interface.unwrap_or_default() }];
    }
    for l in ips {
        let usable = match l.ip {
            IpAddr::V4(ip) => !ip.is_unspecified() && !ip.is_link_local() && !ip.is_multicast(),
            // A `0.0.0.0` listener does not accept IPv6.
            IpAddr::V6(ip) => {
                bound.is_ipv6()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
                    && !ip.is_unicast_link_local()
            }
        };
        if usable && !out.iter().any(|o| o.ip == l.ip) {
            out.push(l);
        }
    }
    let class = |ip: &IpAddr| match ip {
        _ if ip.is_loopback() => 5,
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            let cgnat = a == 100 && (64..128).contains(&b);
            if v4.is_private() || cgnat { 1 } else { 2 }
        }
        IpAddr::V6(v6) if v6.is_unique_local() => 4,
        IpAddr::V6(_) => 3,
    };
    out.sort_by_key(|l| (Some(l.ip) != default_route, class(&l.ip)));
    out
}

/// Choices for the invite link host: the public address (if set) first, then local addresses.
pub fn invite_options(public_address: Option<&str>, local: &[LocalIp]) -> Vec<InviteHost> {
    let public = public_address.map(str::trim).filter(|s| !s.is_empty());
    public
        .map(|p| InviteHost::Public(p.to_owned()))
        .into_iter()
        .chain(local.iter().cloned().map(InviteHost::Local))
        .collect()
}

/// Index of the remembered choice, or of the first option when it is gone.
pub fn pick_option(options: &[InviteHost], saved: Option<&str>) -> usize {
    saved.and_then(|k| options.iter().position(|o| o.key() == k)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    #[test]
    fn link_prefers_public_address() {
        let ip = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        assert_eq!(
            invite_link(Some("box.ts.net:9000"), 8740, "tok", ip),
            "sppy://box.ts.net:9000/tok"
        );
    }

    #[test]
    fn public_address_without_port_uses_listen_port() {
        assert_eq!(
            invite_link(Some("box.ts.net"), 8741, "tok", None),
            "sppy://box.ts.net:8741/tok"
        );
        assert_eq!(invite_link(Some("  "), 8741, "tok", None), "sppy://127.0.0.1:8741/tok");
    }

    #[test]
    fn public_address_may_be_a_link() {
        assert_eq!(
            invite_link(Some("sppys://proxy.example.org"), 8740, "t", None),
            "sppys://proxy.example.org:8740/t"
        );
    }

    #[test]
    fn link_falls_back_to_ip() {
        let v4 = Some(IpAddr::V4(Ipv4Addr::new(100, 64, 1, 2)));
        assert_eq!(invite_link(None, 8740, "tok", v4), "sppy://100.64.1.2:8740/tok");
        let v6 = Some(IpAddr::V6(Ipv6Addr::new(0xfd7a, 0, 0, 0, 0, 0, 0, 1)));
        assert_eq!(invite_link(None, 8740, "tok", v6), "sppy://[fd7a::1]:8740/tok");
        assert_eq!(invite_link(None, 8740, "tok", None), "sppy://127.0.0.1:8740/tok");
    }
}

#[cfg(test)]
mod connect_tests {
    use std::net::SocketAddr;

    use super::*;

    #[test]
    fn unspecified_listen_connects_via_loopback() {
        let a: SocketAddr = "0.0.0.0:8740".parse().unwrap();
        assert_eq!(local_connect_addr(a), "127.0.0.1:8740".parse::<SocketAddr>().unwrap());
        let b: SocketAddr = "[::]:8740".parse().unwrap();
        assert_eq!(local_connect_addr(b), "[::1]:8740".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn specific_listen_address_is_used_as_is() {
        let a: SocketAddr = "100.64.1.2:9000".parse().unwrap();
        assert_eq!(local_connect_addr(a), a);
    }
}

#[cfg(test)]
mod choice_tests {
    use std::net::{IpAddr, SocketAddr};

    use super::*;

    fn l(ip: &str, iface: &str) -> LocalIp {
        LocalIp { ip: ip.parse().unwrap(), interface: iface.into() }
    }

    fn ips(v: &[LocalIp]) -> Vec<String> {
        v.iter().map(|l| l.ip.to_string()).collect()
    }

    fn all() -> Vec<LocalIp> {
        vec![
            l("127.0.0.1", "lo"),
            l("::1", "lo"),
            l("203.0.113.7", "eth1"),
            l("fd7a:115c:a1e0::1", "tailscale0"),
            l("fe80::1", "wlan0"),
            l("169.254.3.4", "eth2"),
            l("100.101.1.2", "tailscale0"),
            l("2001:db8::5", "wlan0"),
            l("192.168.1.5", "wlan0"),
            l("172.17.0.1", "docker0"),
        ]
    }

    #[test]
    fn default_route_first_then_private_public_v6_loopback() {
        let any6: SocketAddr = "[::]:8740".parse().unwrap();
        let route: IpAddr = "192.168.1.5".parse().unwrap();
        assert_eq!(
            ips(&rank_candidates(all(), any6, Some(route))),
            [
                "192.168.1.5",
                "100.101.1.2",
                "172.17.0.1",
                "203.0.113.7",
                "2001:db8::5",
                "fd7a:115c:a1e0::1",
                "127.0.0.1",
                "::1"
            ]
        );
    }

    #[test]
    fn ipv4_listener_offers_ipv4_only() {
        let any4: SocketAddr = "0.0.0.0:8740".parse().unwrap();
        let ranked = rank_candidates(all(), any4, None);
        assert!(ranked.iter().all(|l| l.ip.is_ipv4()), "{ranked:?}");
        assert_eq!(ips(&ranked).first().map(String::as_str), Some("100.101.1.2"));
        assert_eq!(ips(&ranked).last().map(String::as_str), Some("127.0.0.1"));
    }

    #[test]
    fn specific_listener_offers_only_its_address() {
        let bound: SocketAddr = "100.101.1.2:8740".parse().unwrap();
        assert_eq!(rank_candidates(all(), bound, None), [l("100.101.1.2", "tailscale0")]);
        let unknown: SocketAddr = "10.9.9.9:8740".parse().unwrap();
        assert_eq!(rank_candidates(all(), unknown, None), [l("10.9.9.9", "")]);
    }

    #[test]
    fn duplicates_are_dropped() {
        let any4: SocketAddr = "0.0.0.0:1".parse().unwrap();
        let v = vec![l("10.0.0.2", "a"), l("10.0.0.2", "b")];
        assert_eq!(rank_candidates(v, any4, None), [l("10.0.0.2", "a")]);
    }

    #[test]
    fn options_put_public_address_first_and_remember_choice() {
        let local = vec![l("192.168.1.5", "wlan0"), l("127.0.0.1", "lo")];
        let opts = invite_options(Some(" box.ts.net "), &local);
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0], InviteHost::Public("box.ts.net".into()));
        assert_eq!(pick_option(&opts, None), 0);
        assert_eq!(pick_option(&opts, Some("127.0.0.1")), 2);
        assert_eq!(pick_option(&opts, Some("10.1.1.1")), 0, "a vanished address falls back");
        assert_eq!(invite_options(Some(""), &local).len(), 2);
    }

    #[test]
    fn labels_and_links() {
        let lan = InviteHost::Local(l("192.168.1.5", "wlan0"));
        assert_eq!(lan.label(), "192.168.1.5 (wlan0)");
        assert_eq!(lan.link(8740, "t"), "sppy://192.168.1.5:8740/t");
        let lo = InviteHost::Local(l("127.0.0.1", "lo"));
        assert_eq!(lo.label(), "127.0.0.1 (this computer only)");
        let v6 = InviteHost::Local(l("fd7a::1", "tailscale0"));
        assert_eq!(v6.link(8740, "t"), "sppy://[fd7a::1]:8740/t");
        let public = InviteHost::Public("box.ts.net:9000".into());
        assert_eq!(public.label(), "box.ts.net:9000 (public address)");
        assert_eq!(public.link(8740, "t"), "sppy://box.ts.net:9000/t");
        assert_eq!(public.key(), "box.ts.net:9000");
        assert_eq!(lan.key(), "192.168.1.5");
    }
}
