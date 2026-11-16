//! Invite links for an embedded host (spec §7.3).

use std::net::{IpAddr, SocketAddr};

use crate::link::parse_address;
pub use sppy_common::link::{first_non_loopback_ip, invite_link};

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

/// Usable addresses of this machine (see [`rank_candidates`]), best guess first.
pub fn local_ips() -> Vec<LocalIp> {
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
    let any = SocketAddr::new(std::net::Ipv6Addr::UNSPECIFIED.into(), 0);
    rank_candidates(ips, any, first_non_loopback_ip())
}

/// Drop addresses a link cannot carry (link-local, multicast) and order the rest:
/// the default-route address, private and VPN IPv4, other IPv4, global IPv6,
/// unique-local IPv6, loopback last. Then keep what `listen` accepts ([`reachable_via`]).
pub fn rank_candidates(
    ips: Vec<LocalIp>,
    listen: SocketAddr,
    default_route: Option<IpAddr>,
) -> Vec<LocalIp> {
    let mut out: Vec<LocalIp> = Vec::new();
    for l in ips {
        let usable = match l.ip {
            IpAddr::V4(ip) => !ip.is_unspecified() && !ip.is_link_local() && !ip.is_multicast(),
            IpAddr::V6(ip) => {
                !ip.is_unspecified() && !ip.is_multicast() && !ip.is_unicast_link_local()
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
    reachable_via(&out, listen)
}

/// The addresses a server listening on `listen` accepts, in the given order.
pub fn reachable_via(ips: &[LocalIp], listen: SocketAddr) -> Vec<LocalIp> {
    let bound = listen.ip();
    if !bound.is_unspecified() {
        let interface = ips.iter().find(|l| l.ip == bound).map(|l| l.interface.clone());
        return vec![LocalIp { ip: bound, interface: interface.unwrap_or_default() }];
    }
    // A `0.0.0.0` listener does not accept IPv6.
    ips.iter().filter(|l| bound.is_ipv6() || l.ip.is_ipv4()).cloned().collect()
}

/// Addresses the embedded server may listen on: all IPv4 addresses (`0.0.0.0`), each local
/// address, and the configured one even if it is gone. `(address, label)`.
pub fn listen_options(local: &[LocalIp], current: IpAddr) -> Vec<(IpAddr, String)> {
    let all: IpAddr = std::net::Ipv4Addr::UNSPECIFIED.into();
    let mut out = vec![(all, "All addresses (0.0.0.0)".to_owned())];
    out.extend(local.iter().map(|l| (l.ip, InviteHost::Local(l.clone()).label())));
    if !out.iter().any(|(ip, _)| *ip == current) {
        let label = if current.is_unspecified() {
            format!("All addresses ({current})")
        } else {
            format!("{current} (not found on this computer)")
        };
        out.push((current, label));
    }
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

/// Check an address typed by the user for invite links (`host`, `host:port` or a
/// `sppy(s)://` link); returns it trimmed, without a token.
pub fn parse_custom_host(input: &str) -> Result<String, &'static str> {
    let s = input.trim();
    if s.is_empty() {
        return Err("Enter a name or an IP address");
    }
    let addr = parse_address(s).map_err(|_| "Not an address: use host or host:port")?;
    if addr.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified()) {
        return Err("Nobody can connect to this address: enter one others can reach");
    }
    let scheme_end = s.find("://").map_or(0, |i| i + 3);
    Ok(match s[scheme_end..].find('/') {
        Some(slash) => s[..scheme_end + slash].to_owned(),
        None => s.to_owned(),
    })
}

/// Index of the remembered choice, or of the first option when it is gone.
pub fn pick_option(options: &[InviteHost], saved: Option<&str>) -> usize {
    saved.and_then(|k| options.iter().position(|o| o.key() == k)).unwrap_or(0)
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
    fn listen_options_start_with_all_and_keep_the_current_one() {
        let local = vec![l("192.168.1.5", "wlan0"), l("127.0.0.1", "lo")];
        let any: IpAddr = "0.0.0.0".parse().unwrap();
        let opts = listen_options(&local, any);
        let labels: Vec<_> = opts.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(
            labels,
            ["All addresses (0.0.0.0)", "192.168.1.5 (wlan0)", "127.0.0.1 (this computer only)"]
        );
        let gone: IpAddr = "10.9.9.9".parse().unwrap();
        let opts = listen_options(&local, gone);
        assert_eq!(opts.last().unwrap(), &(gone, "10.9.9.9 (not found on this computer)".into()));
        let v6any: IpAddr = "::".parse().unwrap();
        assert_eq!(listen_options(&local, v6any).last().unwrap().1, "All addresses (::)");
    }

    #[test]
    fn custom_host_is_checked_and_trimmed() {
        assert_eq!(parse_custom_host(" box.ts.net "), Ok("box.ts.net".into()));
        assert_eq!(parse_custom_host("203.0.113.7:9000"), Ok("203.0.113.7:9000".into()));
        assert_eq!(parse_custom_host("[2001:db8::1]"), Ok("[2001:db8::1]".into()));
        assert_eq!(parse_custom_host("sppy://box:9000/oldtoken"), Ok("sppy://box:9000".into()));
        assert_eq!(parse_custom_host("box:9000/"), Ok("box:9000".into()));
        assert!(parse_custom_host("").is_err());
        assert!(parse_custom_host("my box").is_err());
        assert!(parse_custom_host("box:notaport").is_err());
        assert!(parse_custom_host("http://box").is_err());
        assert!(parse_custom_host("0.0.0.0").is_err());
        assert!(parse_custom_host("[::]:8740").is_err());
        let link = InviteHost::Public(parse_custom_host("sppy://box:9000/old").unwrap());
        assert_eq!(link.link(8740, "new"), "sppy://box:9000/new");
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
