//! Default locations of the headless server's files.

use std::path::PathBuf;

fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "sppy")
}

/// `config_dir/server.toml` (Linux: `~/.config/sppy/server.toml`).
pub fn default_config_path() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().join("server.toml"))
        .unwrap_or_else(|| PathBuf::from("server.toml"))
}

/// `data_dir/server` (Linux: `~/.local/share/sppy/server`).
pub fn default_data_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.data_dir().join("server"))
        .unwrap_or_else(|| PathBuf::from("sppy-server-data"))
}
