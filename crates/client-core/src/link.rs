//! Server addresses and invite links (spec §7.3).
//!
//! `sppy://host[:port]/<token>` → `ws://…/ws` + `http://…`;
//! `sppys://host[:port]/<token>` → `wss://…/ws` + `https://…`; a bare `host[:port]` has no token.

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
