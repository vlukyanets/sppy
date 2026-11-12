//! Recursive scan of media folders.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sppy_common::config::MediaRoot;
use sppy_common::media::{is_subtitle, is_video};
use sppy_proto::{FileId, Fingerprint, LibraryNode, Subtitle};

use super::FingerprintCache;
use crate::room::FileInfo;

const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone)]
pub struct LibraryFile {
    pub file_id: FileId,
    pub root: String,
    pub rel_path: PathBuf,
    pub abs_path: PathBuf,
    pub size: u64,
    pub mtime: SystemTime,
    /// File name without extension.
    pub title: String,
    pub subtitles: Vec<(Subtitle, PathBuf)>,
    pub fingerprint: Option<Fingerprint>,
}

/// All media files of the configured roots.
#[derive(Debug, Clone, Default)]
pub struct Library {
    roots: Vec<String>,
    files: Vec<LibraryFile>,
    index: HashMap<FileId, usize>,
}

/// First 16 hex chars of `blake3(root_name + "/" + relative_path)`, with `/` separators.
pub fn file_id(root_name: &str, rel_path: &Path) -> FileId {
    let rel = rel_path.to_string_lossy().replace('\\', "/");
    let hash = blake3::hash(format!("{root_name}/{rel}").as_bytes());
    FileId(hash.to_hex()[..16].to_owned())
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

impl Library {
    pub fn scan(roots: &[MediaRoot]) -> Library {
        let mut lib =
            Library { roots: roots.iter().map(|r| r.name.clone()).collect(), ..Library::default() };
        for root in roots {
            if !root.path.is_dir() {
                tracing::warn!("media folder {} is not a directory, skipping", root.path.display());
                continue;
            }
            let mut found = Vec::new();
            walk(root, &root.path, Path::new(""), 0, &mut found);
            found.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
            lib.files.extend(found);
        }
        lib.index = lib.files.iter().enumerate().map(|(i, f)| (f.file_id.clone(), i)).collect();
        lib
    }

    pub fn files(&self) -> impl Iterator<Item = &LibraryFile> {
        self.files.iter()
    }

    pub fn get(&self, id: &FileId) -> Option<&LibraryFile> {
        self.index.get(id).map(|i| &self.files[*i])
    }

    pub fn file_infos(&self) -> Vec<FileInfo> {
        self.files
            .iter()
            .map(|f| FileInfo {
                file_id: f.file_id.clone(),
                title: f.title.clone(),
                fingerprint: f.fingerprint.clone(),
                subtitles: f.subtitles.iter().map(|(s, _)| s.clone()).collect(),
            })
            .collect()
    }

    /// Fill in missing fingerprints (from the cache or by hashing). Returns how many were hashed.
    pub fn fill_fingerprints(&mut self, cache: &mut FingerprintCache) -> usize {
        let mut computed = 0;
        for f in self.files.iter_mut().filter(|f| f.fingerprint.is_none()) {
            if let Some(fp) = cache.get(&f.abs_path, f.size, f.mtime) {
                f.fingerprint = Some(fp.clone());
                continue;
            }
            match sppy_common::media::fingerprint(&f.abs_path) {
                Ok(fp) => {
                    cache.insert(&f.abs_path, f.size, f.mtime, fp.clone());
                    f.fingerprint = Some(fp);
                    computed += 1;
                }
                Err(e) => tracing::warn!("cannot fingerprint {}: {e}", f.abs_path.display()),
            }
        }
        computed
    }

    /// Folder tree per root: directories first, then files, by name (case-insensitive).
    pub fn tree(&self) -> Vec<LibraryNode> {
        #[derive(Default)]
        struct Dir<'a> {
            dirs: BTreeMap<String, (String, Dir<'a>)>,
            files: Vec<&'a LibraryFile>,
        }
        fn insert<'a>(dir: &mut Dir<'a>, comps: &[String], f: &'a LibraryFile) {
            match comps {
                [] | [_] => dir.files.push(f),
                [first, rest @ ..] => {
                    let entry = dir
                        .dirs
                        .entry(first.to_lowercase())
                        .or_insert_with(|| (first.clone(), Dir::default()));
                    insert(&mut entry.1, rest, f);
                }
            }
        }
        fn build(name: String, dir: Dir<'_>) -> LibraryNode {
            let mut children: Vec<LibraryNode> =
                dir.dirs.into_values().map(|(n, d)| build(n, d)).collect();
            let mut files = dir.files;
            files.sort_by_key(|f| {
                f.rel_path.file_name().map(|n| n.to_string_lossy().to_lowercase())
            });
            children.extend(files.into_iter().map(|f| {
                LibraryNode::File {
                    file_id: f.file_id.clone(),
                    name: f
                        .rel_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    size: f.size,
                    fingerprint: f.fingerprint.clone(),
                    subtitles: f.subtitles.iter().map(|(s, _)| s.clone()).collect(),
                }
            }));
            LibraryNode::Dir { name, children }
        }
        self.roots
            .iter()
            .map(|root| {
                let mut top = Dir::default();
                for f in self.files.iter().filter(|f| &f.root == root) {
                    let comps: Vec<String> = f
                        .rel_path
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect();
                    insert(&mut top, &comps, f);
                }
                build(root.clone(), top)
            })
            .collect()
    }
}

fn walk(root: &MediaRoot, dir: &Path, rel: &Path, depth: usize, out: &mut Vec<LibraryFile>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", dir.display());
            return;
        }
    };
    let mut videos = Vec::new();
    let mut subs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if hidden(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::metadata(&path) else { continue };
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &path, &rel.join(&name), depth + 1, out);
            }
        } else if is_video(&path) {
            videos.push((name, path, meta));
        } else if is_subtitle(&path) {
            subs.push(name);
        }
    }
    subs.sort();
    for (name, path, meta) in videos {
        let stem = Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut subtitles = Vec::new();
        for sub in &subs {
            let sub_path = Path::new(sub);
            let sub_stem =
                sub_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let label = if sub_stem == stem {
                None
            } else if let Some(rest) = sub_stem.strip_prefix(&format!("{stem}.")) {
                Some(rest.to_owned())
            } else {
                continue;
            };
            let ext = sub_path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let index = subtitles.len() as u32;
            subtitles.push((Subtitle { index, label, ext }, dir.join(sub)));
        }
        let rel_path = rel.join(&name);
        out.push(LibraryFile {
            file_id: file_id(&root.name, &rel_path),
            root: root.name.clone(),
            abs_path: path,
            size: meta.len(),
            mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            title: stem,
            subtitles,
            fingerprint: None,
            rel_path,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, rel.as_bytes()).unwrap();
    }

    fn files(lib: &Library) -> Vec<String> {
        let mut v: Vec<String> =
            lib.files().map(|f| f.rel_path.to_string_lossy().replace('\\', "/")).collect();
        v.sort();
        v
    }

    fn roots(dir: &Path) -> Vec<MediaRoot> {
        vec![MediaRoot { name: "Movies".into(), path: dir.to_path_buf() }]
    }

    #[test]
    fn scans_videos_recursively_ignores_others() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.mkv");
        touch(dir.path(), "deep/er/b.MP4");
        touch(dir.path(), "notes.txt");
        touch(dir.path(), ".hidden.mkv");
        touch(dir.path(), ".cache/c.mkv");
        let lib = Library::scan(&roots(dir.path()));
        assert_eq!(files(&lib), vec!["a.mkv", "deep/er/b.MP4"]);
    }

    #[test]
    fn file_id_stable_and_16_hex() {
        let id = file_id("Movies", Path::new("a/b.mkv"));
        assert_eq!(id, file_id("Movies", Path::new("a/b.mkv")));
        assert_eq!(id.0.len(), 16);
        assert!(id.0.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(id, file_id("Series", Path::new("a/b.mkv")));
    }

    #[test]
    fn file_id_separator_independent() {
        assert_eq!(file_id("M", Path::new("a\\b.mkv")), file_id("M", Path::new("a/b.mkv")));
    }

    #[test]
    fn subtitles_matched_with_labels() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["Movie.mkv", "Movie.srt", "Movie.en.ass", "Other.srt", "Movie2.srt"] {
            touch(dir.path(), f);
        }
        let lib = Library::scan(&roots(dir.path()));
        let f = lib.files().next().unwrap();
        let subs: Vec<_> =
            f.subtitles.iter().map(|(s, _)| (s.index, s.label.clone(), s.ext.clone())).collect();
        assert_eq!(subs, vec![(0, Some("en".into()), "ass".into()), (1, None, "srt".into())]);
        assert_eq!(f.title, "Movie");
    }

    #[test]
    fn scan_handles_unicode_and_spaces() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "Фильм (2019)/Серия 1.mkv");
        touch(dir.path(), "Фильм (2019)/Серия 1.ru.srt");
        let lib = Library::scan(&roots(dir.path()));
        let f = lib.files().next().unwrap();
        assert_eq!(f.title, "Серия 1");
        assert_eq!(f.subtitles.len(), 1);
        assert_eq!(f.subtitles[0].0.label.as_deref(), Some("ru"));
        assert_eq!(f.file_id, file_id("Movies", Path::new("Фильм (2019)/Серия 1.mkv")));
        assert!(lib.get(&f.file_id).is_some());
    }

    #[test]
    fn tree_lists_dirs_with_videos_only() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "b.mkv");
        touch(dir.path(), "Season 1/e1.mkv");
        touch(dir.path(), "empty/readme.txt");
        let lib = Library::scan(&roots(dir.path()));
        let tree = lib.tree();
        assert_eq!(tree.len(), 1);
        let LibraryNode::Dir { name, children } = &tree[0] else { panic!() };
        assert_eq!(name, "Movies");
        let names: Vec<_> = children
            .iter()
            .map(|c| match c {
                LibraryNode::Dir { name, .. } | LibraryNode::File { name, .. } => name.clone(),
            })
            .collect();
        assert_eq!(names, vec!["Season 1", "b.mkv"]);
    }

    #[test]
    fn missing_root_is_skipped() {
        let lib =
            Library::scan(&[MediaRoot { name: "X".into(), path: "/nonexistent/sppy".into() }]);
        assert_eq!(lib.files().count(), 0);
    }

    #[test]
    fn file_infos_for_room() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "Movie.mkv");
        touch(dir.path(), "Movie.srt");
        let lib = Library::scan(&roots(dir.path()));
        let infos = lib.file_infos();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].title, "Movie");
        assert_eq!(infos[0].subtitles.len(), 1);
    }
}
