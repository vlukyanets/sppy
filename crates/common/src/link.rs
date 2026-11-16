//! Server addresses and invite links (spec §7.3).
//!
//! `sppy://host[:port]/<token>` → `ws://…/ws` + `http://…`;
//! `sppys://host[:port]/<token>` → `wss://…/ws` + `https://…`; a bare `host[:port]` has no token.

use std::net::{IpAddr, UdpSocket};

use sppy_proto::DEFAULT_PORT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid server address: {0}")]
pub struct LinkError(String);

pub fn parse_address(s: &str) -> Result<ServerAddress, LinkError> {
    let err = || LinkError(s.to_owned());
    let s = s.trim();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return Err(err());
    }
    let (secure, rest) = if let Some(r) = s.strip_prefix("sppys://") {
        (true, r)
    } else if let Some(r) = s.strip_prefix("sppy://") {
        (false, r)
    } else if s.contains("://") {
        return Err(err());
    } else {
        (false, s)
    };
    let (authority, token) = match rest.split_once('/') {
        Some((_, t)) if t.contains('/') => return Err(err()),
        Some((a, "")) => (a, None),
        Some((a, t)) => (a, Some(t.to_owned())),
        None => (rest, None),
    };
    let (host, port) = if let Some(after) = authority.strip_prefix('[') {
        let (host, tail) = after.split_once(']').ok_or_else(err)?;
        let port = match tail {
            "" => DEFAULT_PORT,
            t => t.strip_prefix(':').ok_or_else(err)?.parse().map_err(|_| err())?,
        };
        (host.to_owned(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), p.parse().map_err(|_| err())?),
            None => (authority.to_owned(), DEFAULT_PORT),
        }
    };
    if host.is_empty() {
        return Err(err());
    }
    Ok(ServerAddress { secure, host, port, token })
}

impl ServerAddress {
    fn host_port(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn ws_url(&self) -> String {
        let scheme = if self.secure { "wss" } else { "ws" };
        format!("{scheme}://{}/ws", self.host_port())
    }

    pub fn http_base(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}", self.host_port())
    }

    pub fn to_link(&self) -> String {
        let scheme = if self.secure { "sppys" } else { "sppy" };
        match &self.token {
            Some(t) => format!("{scheme}://{}/{t}", self.host_port()),
            None => format!("{scheme}://{}", self.host_port()),
        }
    }
}

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
    use super::*;

    #[test]
    fn parse_sppy_link() {
        let a = parse_address("sppy://box.ts.net:9000/abc").unwrap();
        assert_eq!(
            a,
            ServerAddress {
                secure: false,
                host: "box.ts.net".into(),
                port: 9000,
                token: Some("abc".into())
            }
        );
        assert_eq!(a.ws_url(), "ws://box.ts.net:9000/ws");
        assert_eq!(a.http_base(), "http://box.ts.net:9000");
        assert_eq!(a.to_link(), "sppy://box.ts.net:9000/abc");
    }

    #[test]
    fn parse_sppys_default_port() {
        let a = parse_address("sppys://h/tok").unwrap();
        assert_eq!(a.ws_url(), "wss://h:8740/ws");
        assert_eq!(a.http_base(), "https://h:8740");
        assert_eq!(a.token.as_deref(), Some("tok"));
    }

    #[test]
    fn parse_bare_host() {
        let a = parse_address("  10.0.0.5  ").unwrap();
        assert_eq!(
            a,
            ServerAddress { secure: false, host: "10.0.0.5".into(), port: 8740, token: None }
        );
        assert_eq!(parse_address("myhost:1234").unwrap().port, 1234);
        assert_eq!(a.to_link(), "sppy://10.0.0.5:8740");
    }

    #[test]
    fn parse_ipv6_bracketed() {
        let a = parse_address("sppy://[fd7a::1]:8740/t").unwrap();
        assert_eq!(a.host, "fd7a::1");
        assert_eq!(a.ws_url(), "ws://[fd7a::1]:8740/ws");
        assert_eq!(a.to_link(), "sppy://[fd7a::1]:8740/t");
    }

    #[test]
    fn trailing_slash_means_no_token() {
        assert_eq!(parse_address("sppy://h:1/").unwrap().token, None);
    }

    #[test]
    fn reject_garbage() {
        for bad in [
            "",
            "sppy://",
            "http://h/t",
            "sppy://h:notaport/t",
            "sppy://h:70000/t",
            "a b",
            "sppy://[::1/t",
            "sppy://h/t/extra",
        ] {
            assert!(parse_address(bad).is_err(), "{bad:?} should be rejected");
        }
    }
}

#[cfg(test)]
mod invite_tests {
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
