//! Download cache with an LRU size limit (spec §7.1).
//!
//! Files: `<id>` (complete), `<id>.part` (in progress), `<id>.sub<N>.<ext>` (subtitles).
//! Everything starting with `<id>` belongs to one entry; recency is the newest mtime.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sppy_proto::FileId;

use crate::download::part_path;

#[derive(Debug, Clone)]
pub struct MediaCache {
    dir: PathBuf,
    limit_bytes: u64,
}

struct Entry {
    files: Vec<PathBuf>,
    size: u64,
    newest: SystemTime,
}

impl MediaCache {
    pub fn new(dir: PathBuf, limit_bytes: u64) -> Self {
        MediaCache { dir, limit_bytes }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path_for(&self, id: &FileId) -> PathBuf {
        self.dir.join(&id.0)
    }

    /// The cached file, if fully downloaded.
    pub fn complete(&self, id: &FileId) -> Option<PathBuf> {
        let p = self.path_for(id);
        (p.is_file() && !part_path(&p).exists()).then_some(p)
    }

    /// Mark an entry as just used.
    pub fn touch(&self, id: &FileId) {
        let p = self.path_for(id);
        if let Ok(f) = std::fs::File::options().write(true).open(&p)
            && let Err(e) = f.set_modified(SystemTime::now())
        {
            tracing::debug!("cannot touch {}: {e}", p.display());
        }
    }

    fn entries(&self) -> HashMap<String, Entry> {
        let mut out: HashMap<String, Entry> = HashMap::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        for e in rd.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.split('.').next().unwrap_or_default().to_owned();
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let entry = out.entry(id).or_insert(Entry {
                files: Vec::new(),
                size: 0,
                newest: SystemTime::UNIX_EPOCH,
            });
            entry.files.push(e.path());
            entry.size += meta.len();
            entry.newest = entry.newest.max(mtime);
        }
        out
    }

    pub fn used_bytes(&self) -> u64 {
        self.entries().values().map(|e| e.size).sum()
    }

    /// Delete least recently used entries until under the limit. Returns bytes freed.
    pub fn evict(&self, protected: &HashSet<FileId>) -> io::Result<u64> {
        let entries = self.entries();
        let mut used: u64 = entries.values().map(|e| e.size).sum();
        let mut candidates: Vec<(&String, &Entry)> =
            entries.iter().filter(|(id, _)| !protected.contains(&FileId((*id).clone()))).collect();
        candidates.sort_by_key(|(_, e)| e.newest);
        let mut freed = 0;
        for (_, entry) in candidates {
            if used <= self.limit_bytes {
                break;
            }
            for f in &entry.files {
                std::fs::remove_file(f)?;
            }
            used -= entry.size;
            freed += entry.size;
        }
        Ok(freed)
    }

    /// Delete everything except protected entries.
    pub fn clear(&self, protected: &HashSet<FileId>) -> io::Result<()> {
        for (id, entry) in self.entries() {
            if !protected.contains(&FileId(id)) {
                for f in &entry.files {
                    std::fs::remove_file(f)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    fn put(dir: &Path, name: &str, size: usize, age_secs: u64) {
        let p = dir.join(name);
        std::fs::write(&p, vec![0u8; size]).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
    }

    fn ids(set: &[&str]) -> HashSet<FileId> {
        set.iter().map(|s| FileId((*s).into())).collect()
    }

    #[test]
    fn evicts_oldest_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "old", 400, 300);
        put(dir.path(), "old.sub0.srt", 10, 300);
        put(dir.path(), "mid", 400, 200);
        put(dir.path(), "new", 400, 100);
        let c = MediaCache::new(dir.path().into(), 1000);
        assert_eq!(c.used_bytes(), 1210);
        let freed = c.evict(&ids(&[])).unwrap();
        assert_eq!(freed, 410);
        assert!(!dir.path().join("old").exists());
        assert!(!dir.path().join("old.sub0.srt").exists(), "subtitles go with their video");
        assert!(dir.path().join("mid").exists());
        assert_eq!(c.used_bytes(), 800);
    }

    #[test]
    fn never_evicts_protected() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "old", 600, 300);
        put(dir.path(), "new", 600, 100);
        let c = MediaCache::new(dir.path().into(), 500);
        c.evict(&ids(&["old"])).unwrap();
        assert!(dir.path().join("old").exists());
        assert!(!dir.path().join("new").exists());
    }

    #[test]
    fn parts_count_toward_usage() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "a.part", 700, 300);
        put(dir.path(), "b", 700, 100);
        let c = MediaCache::new(dir.path().into(), 1000);
        assert_eq!(c.used_bytes(), 1400);
        c.evict(&ids(&[])).unwrap();
        assert!(!dir.path().join("a.part").exists());
        assert!(c.complete(&FileId("b".into())).is_some());
    }

    #[test]
    fn touch_makes_entry_recent() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "a", 600, 300);
        put(dir.path(), "b", 600, 100);
        let c = MediaCache::new(dir.path().into(), 700);
        c.touch(&FileId("a".into()));
        c.evict(&ids(&[])).unwrap();
        assert!(dir.path().join("a").exists());
        assert!(!dir.path().join("b").exists());
    }

    #[test]
    fn clear_keeps_protected() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "a", 10, 0);
        put(dir.path(), "b", 10, 0);
        put(dir.path(), "c.part", 10, 0);
        let c = MediaCache::new(dir.path().into(), u64::MAX);
        c.clear(&ids(&["b"])).unwrap();
        assert!(!dir.path().join("a").exists());
        assert!(dir.path().join("b").exists());
        assert!(!dir.path().join("c.part").exists());
    }

    #[test]
    fn complete_requires_no_part() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "a", 10, 0);
        put(dir.path(), "a.part", 10, 0);
        let c = MediaCache::new(dir.path().into(), u64::MAX);
        assert!(c.complete(&FileId("a".into())).is_none());
        assert!(c.complete(&FileId("zz".into())).is_none());
    }

    #[test]
    fn missing_dir_is_empty() {
        let c = MediaCache::new("/nonexistent/sppy-cache".into(), 1);
        assert_eq!(c.used_bytes(), 0);
        assert_eq!(c.evict(&HashSet::new()).unwrap(), 0);
    }
}
