//! Index of "my media folders": finds a local copy of a server file by fingerprint,
//! whatever its name or folder (spec §7.2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sppy_common::media::{fingerprint, is_video};
use sppy_proto::Fingerprint;

const MAX_DEPTH: usize = 32;

#[derive(Debug, Default)]
pub struct LocalIndex {
    by_size: HashMap<u64, Vec<(PathBuf, SystemTime)>>,
    fp_cache: HashMap<(PathBuf, u64, SystemTime), Fingerprint>,
}

impl LocalIndex {
    /// Recursive scan (any depth, video extensions only, hidden entries skipped). Blocking.
    pub fn scan(roots: &[PathBuf]) -> LocalIndex {
        let mut idx = LocalIndex::default();
        let mut visited = std::collections::HashSet::new();
        for root in roots {
            idx.walk(root, 0, &mut visited);
        }
        idx
    }

    /// Rescan, keeping fingerprints of unchanged files.
    pub fn rescan(&mut self, roots: &[PathBuf]) {
        let fp_cache = std::mem::take(&mut self.fp_cache);
        *self = LocalIndex::scan(roots);
        self.fp_cache = fp_cache;
    }

    fn walk(&mut self, dir: &Path, depth: usize, visited: &mut std::collections::HashSet<PathBuf>) {
        // Symlinked directories may form loops: visit each real directory once.
        if !visited.insert(std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = e.path();
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if meta.is_dir() {
                if depth < MAX_DEPTH {
                    self.walk(&path, depth + 1, visited);
                }
            } else if is_video(&path) {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                self.by_size.entry(meta.len()).or_default().push((path, mtime));
            }
        }
    }

    pub fn len(&self) -> usize {
        self.by_size.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A local file with this fingerprint: same size first, then a hash comparison.
    pub fn find(&mut self, fp: &Fingerprint) -> Option<PathBuf> {
        let candidates = self.by_size.get(&fp.size)?.clone();
        for (path, mtime) in candidates {
            let key = (path.clone(), fp.size, mtime);
            let local = match self.fp_cache.get(&key) {
                Some(f) => f.clone(),
                None => match fingerprint(&path) {
                    Ok(f) => {
                        self.fp_cache.insert(key, f.clone());
                        f
                    }
                    Err(e) => {
                        tracing::debug!("cannot fingerprint {}: {e}", path.display());
                        continue;
                    }
                },
            };
            if local == *fp {
                return Some(path);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sppy_common::media::fingerprint;

    use super::*;

    fn write(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn finds_file_regardless_of_layout() {
        let server = tempfile::tempdir().unwrap();
        let mine = tempfile::tempdir().unwrap();
        let original = write(server.path(), "Movie.mkv", b"the movie bytes");
        let copy = write(mine.path(), "deep/nested/whatever-name.bin.mkv", b"the movie bytes");
        let mut idx = LocalIndex::scan(&[mine.path().to_path_buf()]);
        assert_eq!(idx.find(&fingerprint(&original).unwrap()), Some(copy));
    }

    #[test]
    fn same_size_different_content_not_matched() {
        let server = tempfile::tempdir().unwrap();
        let mine = tempfile::tempdir().unwrap();
        let original = write(server.path(), "a.mkv", b"aaaa");
        write(mine.path(), "b.mkv", b"bbbb");
        let mut idx = LocalIndex::scan(&[mine.path().to_path_buf()]);
        assert_eq!(idx.find(&fingerprint(&original).unwrap()), None);
    }

    #[test]
    fn ignores_non_video_extensions_and_hidden() {
        let server = tempfile::tempdir().unwrap();
        let mine = tempfile::tempdir().unwrap();
        let original = write(server.path(), "a.mkv", b"same");
        write(mine.path(), "a.txt", b"same");
        write(mine.path(), ".hidden/a.mkv", b"same");
        let mut idx = LocalIndex::scan(&[mine.path().to_path_buf()]);
        assert_eq!(idx.find(&fingerprint(&original).unwrap()), None);
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn several_roots_and_missing_root() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), "x.mkv", b"1");
        let wanted = write(b.path(), "y.mp4", b"22");
        let mut idx = LocalIndex::scan(&[
            a.path().to_path_buf(),
            "/nonexistent/sppy".into(),
            b.path().to_path_buf(),
        ]);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx.find(&fingerprint(&wanted).unwrap()), Some(wanted));
    }
}

#[cfg(all(test, unix))]
mod symlink_tests {
    use super::*;

    #[test]
    fn symlink_loop_is_indexed_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/movie.mkv"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("a/loop")).unwrap();
        assert_eq!(LocalIndex::scan(&[dir.path().to_path_buf()]).len(), 1);
    }
}
