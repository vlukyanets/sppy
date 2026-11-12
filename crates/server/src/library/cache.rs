//! Persistent fingerprint cache (`library-cache.json`), keyed by path + size + mtime.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sppy_proto::Fingerprint;

const FILE_NAME: &str = "library-cache.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    path: String,
    size: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    fingerprint: Fingerprint,
}

#[derive(Debug, Clone, Default)]
pub struct FingerprintCache {
    entries: HashMap<String, Entry>,
}

fn split(mtime: SystemTime) -> (u64, u32) {
    let d = mtime.duration_since(UNIX_EPOCH).unwrap_or_default();
    (d.as_secs(), d.subsec_nanos())
}

impl FingerprintCache {
    /// Load from the data dir; a missing or corrupt file gives an empty cache.
    pub fn load(data_dir: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(data_dir.join(FILE_NAME)) else {
            return Self::default();
        };
        match serde_json::from_str::<Vec<Entry>>(&text) {
            Ok(list) => FingerprintCache {
                entries: list.into_iter().map(|e| (e.path.clone(), e)).collect(),
            },
            Err(e) => {
                tracing::warn!("ignoring corrupt {FILE_NAME}: {e}");
                Self::default()
            }
        }
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let mut list: Vec<&Entry> = self.entries.values().collect();
        list.sort_by(|a, b| a.path.cmp(&b.path));
        let json = serde_json::to_vec(&list).expect("cache always serializes");
        crate::tokens::write_private_atomic(&data_dir.join(FILE_NAME), &json)
    }

    pub fn get(&self, path: &Path, size: u64, mtime: SystemTime) -> Option<&Fingerprint> {
        let (secs, nanos) = split(mtime);
        self.entries
            .get(path.to_string_lossy().as_ref())
            .filter(|e| e.size == size && e.mtime_secs == secs && e.mtime_nanos == nanos)
            .map(|e| &e.fingerprint)
    }

    pub fn insert(&mut self, path: &Path, size: u64, mtime: SystemTime, fingerprint: Fingerprint) {
        let (mtime_secs, mtime_nanos) = split(mtime);
        let path = path.to_string_lossy().into_owned();
        self.entries
            .insert(path.clone(), Entry { path, size, mtime_secs, mtime_nanos, fingerprint });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    #[test]
    fn fingerprint_cache_reused_when_mtime_same() {
        let dir = tempfile::tempdir().unwrap();
        let mtime = UNIX_EPOCH + Duration::from_secs(1000);
        let fp = Fingerprint { size: 3, hash: "ab".repeat(32) };
        let mut c = FingerprintCache::default();
        c.insert(Path::new("/m/a.mkv"), 3, mtime, fp.clone());
        c.save(dir.path()).unwrap();
        let c = FingerprintCache::load(dir.path());
        assert_eq!(c.get(Path::new("/m/a.mkv"), 3, mtime), Some(&fp));
        assert_eq!(c.get(Path::new("/m/a.mkv"), 3, mtime + Duration::from_secs(1)), None);
        assert_eq!(c.get(Path::new("/m/a.mkv"), 4, mtime), None);
    }

    #[test]
    fn corrupt_cache_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("library-cache.json"), "nope").unwrap();
        assert!(FingerprintCache::load(dir.path()).is_empty());
    }

    #[test]
    fn fill_fingerprints_uses_cache_and_updates_library() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.mkv"), b"abc").unwrap();
        let roots = [sppy_common::config::MediaRoot { name: "M".into(), path: dir.path().into() }];
        let mut lib = crate::library::Library::scan(&roots);
        let mut cache = FingerprintCache::default();
        assert_eq!(lib.fill_fingerprints(&mut cache), 1);
        assert_eq!(cache.len(), 1);
        let f = lib.files().next().unwrap();
        assert_eq!(f.fingerprint.as_ref().unwrap().size, 3);
        let mut lib2 = crate::library::Library::scan(&roots);
        assert_eq!(lib2.fill_fingerprints(&mut cache), 0, "served from cache");
        assert!(lib2.files().next().unwrap().fingerprint.is_some());
    }
}
