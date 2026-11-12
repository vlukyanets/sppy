//! Server configuration (`server.toml`, or the `[host]` section of the client config).

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One configured media folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRoot {
    pub name: String,
    pub path: PathBuf,
}

/// Server settings (spec §10). Every field has a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_address: Option<String>,
    pub max_participants: usize,
    pub resume_grace_secs: u64,
    /// 0 = never hold the room for a stalled participant.
    pub stall_hold_secs: u32,
    pub resume_buffer_secs: u32,
    pub media: Vec<MediaRoot>,
    /// Undocumented; tests shorten it.
    #[doc(hidden)]
    #[serde(skip_serializing_if = "is_default_ping_timeout")]
    pub ping_timeout_ms: u64,
}

const DEFAULT_PING_TIMEOUT_MS: u64 = 10_000;

fn is_default_ping_timeout(v: &u64) -> bool {
    *v == DEFAULT_PING_TIMEOUT_MS
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            listen: SocketAddr::from(([0, 0, 0, 0], 8740)),
            public_address: None,
            max_participants: 8,
            resume_grace_secs: 60,
            stall_hold_secs: 10,
            resume_buffer_secs: 10,
            media: Vec::new(),
            ping_timeout_ms: DEFAULT_PING_TIMEOUT_MS,
        }
    }
}

impl ServerConfig {
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("server config always serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_from_empty_toml() {
        let c = ServerConfig::from_toml("").unwrap();
        assert_eq!(c.listen, "0.0.0.0:8740".parse().unwrap());
        assert_eq!(c.public_address, None);
        assert_eq!(c.max_participants, 8);
        assert_eq!(c.resume_grace_secs, 60);
        assert_eq!(c.stall_hold_secs, 10);
        assert_eq!(c.resume_buffer_secs, 10);
        assert!(c.media.is_empty());
        assert_eq!(c.ping_timeout_ms, 10_000);
    }

    #[test]
    fn config_parses_media_roots() {
        let c = ServerConfig::from_toml(
            r#"
            listen = "127.0.0.1:9000"
            public_address = "box.ts.net:9000"
            stall_hold_secs = 0

            [[media]]
            name = "Movies"
            path = "/srv/movies"

            [[media]]
            name = "Series"
            path = "/srv/series"
            "#,
        )
        .unwrap();
        assert_eq!(c.listen.port(), 9000);
        assert_eq!(c.public_address.as_deref(), Some("box.ts.net:9000"));
        assert_eq!(c.stall_hold_secs, 0);
        assert_eq!(
            c.media,
            vec![
                MediaRoot { name: "Movies".into(), path: "/srv/movies".into() },
                MediaRoot { name: "Series".into(), path: "/srv/series".into() },
            ]
        );
    }

    #[test]
    fn config_rejects_unknown_types() {
        assert!(ServerConfig::from_toml("max_participants = \"many\"").is_err());
    }

    #[test]
    fn config_roundtrips_through_toml() {
        let c = ServerConfig::from_toml("[[media]]\nname = \"M\"\npath = \"/m\"\n").unwrap();
        assert_eq!(ServerConfig::from_toml(&c.to_toml()).unwrap(), c);
    }
}
