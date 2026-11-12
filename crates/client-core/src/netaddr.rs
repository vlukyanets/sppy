//! Invite links for an embedded host (spec §7.3).

use std::net::{IpAddr, UdpSocket};

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

/// The address other machines most likely reach us at (no packets are sent).
pub fn first_non_loopback_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
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
