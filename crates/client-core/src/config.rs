//! Client configuration (`client.toml`, spec §10) and file locations.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sppy_common::config::ServerConfig;
use sppy_sync::SyncParams;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryMode {
    #[default]
    Stream,
    Preload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedServer {
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerConfig {
    pub kind: String,
    pub path: PathBuf,
    pub extra_args: Vec<String>,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        PlayerConfig { kind: "mpv".into(), path: PathBuf::from("mpv"), extra_args: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub nickname: String,
    pub mode: DeliveryMode,
    pub prefetch_next: bool,
    pub local_media: Vec<PathBuf>,
    /// Preferred subtitle languages, most wanted first (e.g. `["ru", "en"]`).
    pub subtitle_languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,
    pub cache_limit_gb: u64,
    pub servers: Vec<SavedServer>,
    pub player: PlayerConfig,
    pub advanced: SyncParams,
    /// Embedded server ("Become host").
    pub host: ServerConfig,
    /// Undocumented: pauses between download attempts (empty = defaults); tests shorten it.
    #[doc(hidden)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub download_backoff_ms: Vec<u64>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            nickname: String::new(),
            mode: DeliveryMode::Stream,
            prefetch_next: true,
            local_media: Vec::new(),
            subtitle_languages: Vec::new(),
            cache_dir: None,
            cache_limit_gb: 30,
            servers: Vec::new(),
            player: PlayerConfig::default(),
            advanced: SyncParams::default(),
            host: ServerConfig::default(),
            download_backoff_ms: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot access {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("invalid config {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
}

impl ClientConfig {
    /// Where downloaded media is kept.
    pub fn media_cache_dir(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(|| client_paths().cache_dir).join("media")
    }

    /// Load the config; a missing file gives the defaults.
    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|source| ConfigError::Parse { path: path.to_owned(), source }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io { path: path.to_owned(), source }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |source| ConfigError::Io { path: path.to_owned(), source };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = toml::to_string_pretty(self).expect("client config always serializes");
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }
}

/// Where the client keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPaths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub host_data_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl ClientPaths {
    /// Everything under one directory (used when `SPPY_HOME` is set, e.g. to run two clients).
    pub fn under(root: &Path) -> Self {
        let data = root.join("data");
        ClientPaths {
            config_file: root.join("config").join("client.toml"),
            cache_dir: root.join("cache"),
            host_data_dir: data.join("host"),
            logs_dir: data.join("logs"),
            data_dir: data,
        }
    }
}

/// Platform directories for "sppy", or everything under `$SPPY_HOME` when it is set.
pub fn client_paths() -> ClientPaths {
    if let Some(home) = std::env::var_os("SPPY_HOME") {
        return ClientPaths::under(Path::new(&home));
    }
    match directories::ProjectDirs::from("", "", "sppy") {
        Some(d) => ClientPaths {
            config_file: d.config_dir().join("client.toml"),
            data_dir: d.data_dir().to_owned(),
            cache_dir: d.cache_dir().to_owned(),
            host_data_dir: d.data_dir().join("host"),
            logs_dir: d.data_dir().join("logs"),
        },
        None => ClientPaths::under(Path::new(".sppy")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let c = ClientConfig::default();
        assert_eq!(c.mode, DeliveryMode::Stream);
        assert!(c.prefetch_next);
        assert_eq!(c.cache_limit_gb, 30);
        assert_eq!(c.player.kind, "mpv");
        assert_eq!(c.player.path, std::path::PathBuf::from("mpv"));
        assert_eq!(c.advanced, sppy_sync::SyncParams::default());
        assert_eq!(c.host, sppy_common::config::ServerConfig::default());
    }

    #[test]
    fn config_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/client.toml");
        assert_eq!(ClientConfig::load_or_default(&path).unwrap(), ClientConfig::default());
        let mut c = ClientConfig {
            nickname: "alice".into(),
            mode: DeliveryMode::Preload,
            ..ClientConfig::default()
        };
        c.servers.push(SavedServer { address: "sppy://h:1/t".into() });
        c.advanced.dead_band_ms = 100;
        c.host.media.push(sppy_common::config::MediaRoot { name: "M".into(), path: "/m".into() });
        c.save(&path).unwrap();
        assert_eq!(ClientConfig::load_or_default(&path).unwrap(), c);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let c: ClientConfig = toml::from_str(
            "nickname = \"bob\"\nmode = \"preload\"\n[advanced]\nseek_threshold_ms = 2000\n",
        )
        .unwrap();
        assert_eq!(c.nickname, "bob");
        assert_eq!(c.mode, DeliveryMode::Preload);
        assert_eq!(c.advanced.seek_threshold_ms, 2000);
        assert_eq!(c.advanced.dead_band_ms, 80);
        assert_eq!(c.cache_limit_gb, 30);
    }

    #[test]
    fn invalid_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.toml");
        std::fs::write(&path, "cache_limit_gb = \"lots\"").unwrap();
        assert!(ClientConfig::load_or_default(&path).is_err());
    }

    #[test]
    fn paths_under_sppy_home() {
        let p = ClientPaths::under(std::path::Path::new("/x"));
        assert_eq!(p.config_file, std::path::PathBuf::from("/x/config/client.toml"));
        assert_eq!(p.cache_dir, std::path::PathBuf::from("/x/cache"));
        assert_eq!(p.host_data_dir, std::path::PathBuf::from("/x/data/host"));
        assert_eq!(p.logs_dir, std::path::PathBuf::from("/x/data/logs"));
    }
}
