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

const DEFAULT_IDENTIFY_WAIT_MS: u64 = 10_000;

fn is_default_identify_wait(v: &u64) -> bool {
    *v == DEFAULT_IDENTIFY_WAIT_MS
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub nickname: String,
    pub mode: DeliveryMode,
    pub prefetch_next: bool,
    /// Old separate list of "my media folders"; moved into `host.media` on load.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub local_media: Vec<PathBuf>,
    /// Preferred subtitle languages, most wanted first (e.g. `["ru", "en"]`).
    pub subtitle_languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,
    pub cache_limit_gb: u64,
    pub servers: Vec<SavedServer>,
    pub player: PlayerConfig,
    pub advanced: SyncParams,
    /// Embedded server ("Become host"). Its `media` are "my media folders": local copies
    /// are found there, and they are shared while hosting.
    pub host: ServerConfig,
    /// Host put in invite links: a local IP or the public address (unset = best guess).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_host: Option<String>,
    /// Undocumented: how long to wait for an item's fingerprint before downloading or
    /// streaming it (a local copy may turn up); tests shorten it.
    #[doc(hidden)]
    #[serde(skip_serializing_if = "is_default_identify_wait")]
    pub identify_wait_ms: u64,
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
            cache_limit_gb: 20,
            servers: Vec::new(),
            player: PlayerConfig::default(),
            advanced: SyncParams::default(),
            host: ServerConfig::default(),
            invite_host: None,
            identify_wait_ms: DEFAULT_IDENTIFY_WAIT_MS,
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
    /// One list of media folders: the old `local_media` joins `host.media`.
    pub fn migrated(mut self) -> Self {
        for path in std::mem::take(&mut self.local_media) {
            if let Ok(root) = sppy_common::folders::new_folder(&self.host.media, &path, None) {
                self.host.media.push(root);
            } else if !self.host.media.iter().any(|r| r.path == path) {
                // Missing right now (e.g. an unplugged disk): keep it anyway.
                let name = sppy_common::folders::default_name(&self.host.media, &path);
                self.host.media.push(sppy_common::config::MediaRoot { name, path });
            }
        }
        self
    }

    /// Folders searched for local copies: "my media folders" and the host's library.
    pub fn media_folders(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for p in self.local_media.iter().chain(self.host.media.iter().map(|r| &r.path)) {
            if !out.contains(p) {
                out.push(p.clone());
            }
        }
        out
    }

    pub fn media_cache_dir(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(|| client_paths().cache_dir).join("media")
    }

    /// Load the config; a missing file gives the defaults.
    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Self>(&text)
                .map(Self::migrated)
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
        // Only what differs from the defaults: changed defaults then reach existing users.
        let mut table = toml::Table::try_from(self).expect("client config always serializes");
        let defaults =
            toml::Table::try_from(Self::default()).expect("client config always serializes");
        strip_defaults(&mut table, &defaults);
        let text = toml::to_string_pretty(&table).expect("a toml table always serializes");
        let tmp = path.with_extension("toml.tmp");
        // Saved links may carry the host token: readable by this user only.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let _ = std::fs::remove_file(&tmp);
        std::io::Write::write_all(&mut options.open(&tmp).map_err(io)?, text.as_bytes())
            .map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }
}

/// Remove the values equal to `defaults`, descending into tables; emptied tables go too.
fn strip_defaults(table: &mut toml::Table, defaults: &toml::Table) {
    table.retain(|key, value| {
        let Some(default) = defaults.get(key) else { return true };
        if let (toml::Value::Table(t), toml::Value::Table(d)) = (&mut *value, default) {
            strip_defaults(t, d);
            return !t.is_empty();
        }
        value != default
    });
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
            cache_dir: root.join(".cache"),
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
    fn old_local_media_joins_the_host_folders() {
        let dir = tempfile::tempdir().unwrap();
        let (movies, extra) = (dir.path().join("Movies"), dir.path().join("Extra"));
        std::fs::create_dir(&movies).unwrap();
        std::fs::create_dir(&extra).unwrap();
        let gone = dir.path().join("Unplugged");
        let text = format!(
            "local_media = [{:?}, {:?}, {:?}]\n[[host.media]]\nname = \"Films\"\npath = {:?}\n",
            movies.display().to_string(),
            extra.display().to_string(),
            gone.display().to_string(),
            movies.display().to_string(),
        );
        let file = dir.path().join("client.toml");
        std::fs::write(&file, text).unwrap();
        let c = ClientConfig::load_or_default(&file).unwrap();
        assert!(c.local_media.is_empty());
        let got: Vec<_> = c.host.media.iter().map(|r| (r.name.as_str(), r.path.clone())).collect();
        assert_eq!(got, [("Films", movies), ("Extra", extra), ("Unplugged", gone)]);
        c.save(&file).unwrap();
        assert!(!std::fs::read_to_string(&file).unwrap().contains("local_media"));
    }

    #[cfg(unix)]
    #[test]
    fn config_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("client.toml");
        std::fs::write(&file, "nickname = \"old\"\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let c = ClientConfig { nickname: "bob".into(), ..ClientConfig::default() };
        c.save(&file).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "saved links may carry the host token");
    }

    #[test]
    fn only_changed_settings_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("client.toml");
        ClientConfig::default().save(&file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "");

        let mut c = ClientConfig { nickname: "bob".into(), ..ClientConfig::default() };
        c.advanced.dead_band_ms = 50;
        c.host.listen.set_port(9000);
        c.host.media.push(sppy_common::config::MediaRoot { name: "M".into(), path: "/m".into() });
        c.save(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        for wanted in ["nickname", "dead_band_ms", "listen", "[[host.media]]"] {
            assert!(text.contains(wanted), "{wanted} missing in:\n{text}");
        }
        for unwanted in ["cache_limit_gb", "prefetch_next", "seek_threshold_ms", "max_participants"]
        {
            assert!(!text.contains(unwanted), "{unwanted} saved in:\n{text}");
        }
        assert_eq!(ClientConfig::load_or_default(&file).unwrap(), c);
    }

    #[test]
    fn config_defaults() {
        let c = ClientConfig::default();
        assert_eq!(c.mode, DeliveryMode::Stream);
        assert!(c.prefetch_next);
        assert_eq!(c.cache_limit_gb, 20);
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
        assert_eq!(c.cache_limit_gb, 20);
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
        assert_eq!(p.cache_dir, std::path::PathBuf::from("/x/.cache"));
        assert_eq!(p.host_data_dir, std::path::PathBuf::from("/x/data/host"));
        assert_eq!(p.logs_dir, std::path::PathBuf::from("/x/data/logs"));
    }
}
