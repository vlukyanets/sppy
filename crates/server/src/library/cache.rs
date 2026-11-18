//! Persistent fingerprint cache (`library-cache.json`), keyed by path + size + mtime.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sppy_proto::Fingerprint;

const FILE_NAME: &str = "library-cache.json";
/// Raise when `sppy_common::duration` learns to read more files: the ones it found no
/// duration in before are read again.
const DURATION_PARSER: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    path: String,
    size: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    fingerprint: Fingerprint,
    /// From the container header; `None` when it does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    /// The header was read; entries from before durations were cached lack it.
    #[serde(default)]
    probed: bool,
    /// [`DURATION_PARSER`] that read it (0: before versions were kept).
    #[serde(default)]
    parser: u32,
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

    fn entry(&self, path: &Path, size: u64, mtime: SystemTime) -> Option<&Entry> {
        let (secs, nanos) = split(mtime);
        self.entries
            .get(path.to_string_lossy().as_ref())
            .filter(|e| e.size == size && e.mtime_secs == secs && e.mtime_nanos == nanos)
    }

    pub fn get(&self, path: &Path, size: u64, mtime: SystemTime) -> Option<&Fingerprint> {
        self.entry(path, size, mtime).map(|e| &e.fingerprint)
    }

    /// The cached duration: `Some(None)` when the current parser read the file and found
    /// none.
    pub fn duration(&self, path: &Path, size: u64, mtime: SystemTime) -> Option<Option<u64>> {
        self.entry(path, size, mtime)
            .filter(|e| e.probed && (e.duration_ms.is_some() || e.parser >= DURATION_PARSER))
            .map(|e| e.duration_ms)
    }

    pub fn insert(&mut self, path: &Path, size: u64, mtime: SystemTime, fingerprint: Fingerprint) {
        let (mtime_secs, mtime_nanos) = split(mtime);
        let path = path.to_string_lossy().into_owned();
        self.entries.insert(
            path.clone(),
            Entry {
                path,
                size,
                mtime_secs,
                mtime_nanos,
                fingerprint,
                duration_ms: None,
                probed: false,
                parser: 0,
            },
        );
    }

    /// Remember the duration read from the header of a cached file.
    pub fn set_duration(&mut self, path: &Path, duration_ms: Option<u64>) {
        if let Some(e) = self.entries.get_mut(path.to_string_lossy().as_ref()) {
            e.duration_ms = duration_ms;
            e.probed = true;
            e.parser = DURATION_PARSER;
        }
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
        assert_eq!(lib.fill_metadata(&mut cache), 1);
        assert_eq!(cache.len(), 1);
        let f = lib.files().next().unwrap();
        assert_eq!(f.fingerprint.as_ref().unwrap().size, 3);
        let mut lib2 = crate::library::Library::scan(&roots);
        assert_eq!(lib2.fill_metadata(&mut cache), 0, "served from cache");
        assert!(lib2.files().next().unwrap().fingerprint.is_some());
    }

    /// A minimal Matroska file whose header says 1.5 s.
    fn tiny_mkv() -> Vec<u8> {
        let el = |id: &[u8], payload: &[u8]| {
            let mut v = id.to_vec();
            v.push(0x80 | payload.len() as u8);
            v.extend_from_slice(payload);
            v
        };
        let info = el(&[0x15, 0x49, 0xA9, 0x66], &el(&[0x44, 0x89], &1_500.0f64.to_be_bytes()));
        [el(&[0x1A, 0x45, 0xDF, 0xA3], b""), el(&[0x18, 0x53, 0x80, 0x67], &info)].concat()
    }

    #[test]
    fn durations_are_read_from_headers_and_cached() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.mkv"), tiny_mkv()).unwrap();
        std::fs::write(dir.path().join("b.avi"), b"RIFF....AVI ").unwrap();
        let roots = [sppy_common::config::MediaRoot { name: "M".into(), path: dir.path().into() }];
        let mut lib = crate::library::Library::scan(&roots);
        let mut cache = FingerprintCache::default();
        assert_eq!(lib.fill_metadata(&mut cache), 2);
        let duration = |lib: &crate::library::Library, name: &str| {
            lib.files().find(|f| f.title == name).unwrap().duration_ms
        };
        assert_eq!(duration(&lib, "a"), Some(1_500));
        assert_eq!(duration(&lib, "b"), None, "AVI: left to the players");
        assert_eq!(
            lib.file_infos().iter().find(|i| i.title == "a").unwrap().duration_ms,
            Some(1_500)
        );

        let data = tempfile::tempdir().unwrap();
        cache.save(data.path()).unwrap();
        let mut cache = FingerprintCache::load(data.path());
        let mut lib2 = crate::library::Library::scan(&roots);
        assert_eq!(lib2.fill_metadata(&mut cache), 0, "nothing read again");
        assert_eq!(duration(&lib2, "a"), Some(1_500));
    }

    #[test]
    fn old_cache_entries_get_their_duration_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mkv");
        std::fs::write(&path, tiny_mkv()).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let (secs, nanos) = split(meta.modified().unwrap());
        let fp = sppy_common::media::fingerprint(&path).unwrap();
        // An entry written before durations were cached.
        let old = format!(
            r#"[{{"path":{:?},"size":{},"mtime_secs":{secs},"mtime_nanos":{nanos},"fingerprint":{}}}]"#,
            path.to_string_lossy(),
            meta.len(),
            serde_json::to_string(&fp).unwrap()
        );
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("library-cache.json"), old).unwrap();
        let mut cache = FingerprintCache::load(data.path());
        let roots = [sppy_common::config::MediaRoot { name: "M".into(), path: dir.path().into() }];
        let mut lib = crate::library::Library::scan(&roots);
        assert_eq!(lib.fill_metadata(&mut cache), 1, "only the duration is read");
        assert_eq!(lib.files().next().unwrap().duration_ms, Some(1_500));
        assert_eq!(lib.files().next().unwrap().fingerprint.as_ref(), Some(&fp));
    }

    /// Files an older parser could not read (MPEG-TS before it learned that) are read once
    /// more; files whose duration is known, or that the current parser read, are not.
    #[test]
    fn files_without_a_duration_are_read_again_by_a_newer_parser() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mkv");
        std::fs::write(&path, tiny_mkv()).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let (secs, nanos) = split(meta.modified().unwrap());
        let fp = sppy_common::media::fingerprint(&path).unwrap();
        let entry = |extra: &str| {
            format!(
                r#"[{{"path":{:?},"size":{},"mtime_secs":{secs},"mtime_nanos":{nanos},"fingerprint":{},"probed":true{extra}}}]"#,
                path.to_string_lossy(),
                meta.len(),
                serde_json::to_string(&fp).unwrap()
            )
        };
        let roots = [sppy_common::config::MediaRoot { name: "M".into(), path: dir.path().into() }];
        let data = tempfile::tempdir().unwrap();
        let fill = |json: String| {
            std::fs::write(data.path().join("library-cache.json"), json).unwrap();
            let mut cache = FingerprintCache::load(data.path());
            let mut lib = crate::library::Library::scan(&roots);
            let read = lib.fill_metadata(&mut cache);
            (read, lib.files().next().unwrap().duration_ms)
        };
        assert_eq!(fill(entry("")), (1, Some(1_500)), "an older parser found nothing");
        let current = format!(r#","parser":{DURATION_PARSER}"#);
        assert_eq!(fill(entry(&current)), (0, None), "the current one found nothing");
        assert_eq!(fill(entry(r#","duration_ms":7"#)), (0, Some(7)), "known");
    }
}
