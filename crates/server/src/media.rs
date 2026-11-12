//! State shared between the room hub and the HTTP media routes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use sppy_common::config::MediaRoot;

use crate::library::{FingerprintCache, Library};

#[derive(Default)]
pub struct SharedMedia {
    pub library: RwLock<Library>,
    /// Session tokens accepted on `/media` (kept in sync by the hub).
    pub sessions: RwLock<HashSet<String>>,
}

impl SharedMedia {
    pub fn session_valid(&self, token: &str) -> bool {
        self.sessions.read().expect("sessions lock").contains(token)
    }

    /// Path of a library file, or of one of its subtitles.
    pub fn resolve(&self, file_id: &str, subtitle: Option<u32>) -> Option<PathBuf> {
        let lib = self.library.read().expect("library lock");
        let f = lib.get(&sppy_proto::FileId(file_id.to_owned()))?;
        match subtitle {
            None => Some(f.abs_path.clone()),
            Some(i) => f.subtitles.iter().find(|(s, _)| s.index == i).map(|(_, p)| p.clone()),
        }
    }
}

/// Full scan plus fingerprints (blocking; run in `spawn_blocking`).
pub fn scan_with_fingerprints(roots: &[MediaRoot], data_dir: &Path) -> Library {
    let mut lib = Library::scan(roots);
    let mut cache = FingerprintCache::load(data_dir);
    if lib.fill_fingerprints(&mut cache) > 0
        && let Err(e) = cache.save(data_dir)
    {
        tracing::warn!("cannot save fingerprint cache: {e}");
    }
    lib
}
