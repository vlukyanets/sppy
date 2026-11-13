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
            let mut visited = std::collections::HashSet::new();
            walk(root, &root.path, Path::new(""), 0, &mut visited, &mut found);
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

/// Subtitle folders next to videos: `Subs/`, `Subtitles/`, `Sub/` (any case).
const SUBTITLE_DIRS: &[&str] = &["subs", "subtitles", "sub"];

/// Lowercase; every run of non-alphanumeric characters becomes one `.`.
/// `Movie (2019) [EN]` → `movie.2019.en`, so differently punctuated names still match.
pub(crate) fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut sep = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            if sep && !out.is_empty() {
                out.push('.');
            }
            sep = false;
            out.extend(c.to_lowercase());
        } else {
            sep = true;
        }
    }
    out
}

/// Stable subtitle id: the first 32 bits of `blake3(path relative to the media root)`.
fn subtitle_index(root: &Path, path: &Path) -> u32 {
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    let h = blake3::hash(rel.as_bytes());
    u32::from_le_bytes(h.as_bytes()[..4].try_into().expect("4 bytes"))
}

fn stem_of(name: &str) -> String {
    Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A subtitle file found for the videos of one folder.
struct SubCandidate {
    path: PathBuf,
    /// Normalized file stem.
    stem: String,
    /// From `Subs/<video name>/`: belongs to that (normalized) video name whatever its own name.
    video: Option<String>,
}

fn subtitle_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_subtitle(p))
        .filter(|p| !p.file_name().is_some_and(|n| hidden(&n.to_string_lossy())))
        .collect()
}

/// Subtitles of a `Subs/` folder: loose files, and `Subs/<video name>/` folders.
fn collect_subs_dir(subs_dir: &Path, out: &mut Vec<SubCandidate>) {
    for path in subtitle_files(subs_dir) {
        let stem =
            normalize_name(&stem_of(&path.file_name().unwrap_or_default().to_string_lossy()));
        out.push(SubCandidate { path, stem, video: None });
    }
    let Ok(rd) = fs::read_dir(subs_dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let video = normalize_name(&e.file_name().to_string_lossy());
        for path in subtitle_files(&p) {
            let stem =
                normalize_name(&stem_of(&path.file_name().unwrap_or_default().to_string_lossy()));
            out.push(SubCandidate { path, stem, video: Some(video.clone()) });
        }
    }
}

fn walk(
    root: &MediaRoot,
    dir: &Path,
    rel: &Path,
    depth: usize,
    visited: &mut std::collections::HashSet<PathBuf>,
    out: &mut Vec<LibraryFile>,
) {
    // Symlinked directories may form loops: visit each real directory once.
    if !visited.insert(fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())) {
        return;
    }
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
            if SUBTITLE_DIRS.contains(&name.to_lowercase().as_str()) {
                collect_subs_dir(&path, &mut subs);
            }
            if depth < MAX_DEPTH {
                walk(root, &path, &rel.join(&name), depth + 1, visited, out);
            }
        } else if is_video(&path) {
            videos.push((name, path, meta));
        } else if is_subtitle(&path) {
            subs.push(SubCandidate { stem: normalize_name(&stem_of(&name)), path, video: None });
        }
    }
    subs.sort_by(|a, b| a.path.cmp(&b.path));
    let video_stems: Vec<String> =
        videos.iter().map(|(name, _, _)| normalize_name(&stem_of(name))).collect();
    // A subtitle belongs to the video whose normalized name it equals or starts with
    // (followed by a separator); the longest such name wins, so `Movie.2019.srt` goes to
    // `Movie.2019.mkv` rather than to `Movie.mkv` with label "2019".
    let mut per_video: Vec<Vec<(Option<String>, PathBuf)>> = vec![Vec::new(); videos.len()];
    for sub in subs {
        if let Some(target) = &sub.video {
            if let Some(vi) = video_stems.iter().position(|v| v == target) {
                per_video[vi].push((Some(sub.stem.clone()).filter(|s| !s.is_empty()), sub.path));
            }
            continue;
        }
        let best = video_stems
            .iter()
            .enumerate()
            .filter(|(_, v)| {
                !v.is_empty() && (sub.stem == **v || sub.stem.starts_with(&format!("{v}.")))
            })
            .max_by_key(|(_, v)| v.len());
        if let Some((vi, v)) = best {
            let rest = sub.stem.get(v.len() + 1..).unwrap_or_default().to_owned();
            per_video[vi].push(((!rest.is_empty()).then_some(rest), sub.path));
        }
    }
    for ((name, path, meta), matched) in videos.into_iter().zip(per_video) {
        let subtitles: Vec<(Subtitle, PathBuf)> = matched
            .into_iter()
            .map(|(label, sub_path)| {
                let ext = sub_path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                (Subtitle { index: subtitle_index(&root.path, &sub_path), label, ext }, sub_path)
            })
            .collect();
        let rel_path = rel.join(&name);
        out.push(LibraryFile {
            file_id: file_id(&root.name, &rel_path),
            root: root.name.clone(),
            abs_path: path,
            size: meta.len(),
            mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            title: stem_of(&name),
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
            f.subtitles.iter().map(|(s, _)| (s.label.clone(), s.ext.clone())).collect();
        assert_eq!(subs, vec![(Some("en".into()), "ass".into()), (None, "srt".into())]);
        assert_ne!(f.subtitles[0].0.index, f.subtitles[1].0.index);
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

#[cfg(all(test, unix))]
mod symlink_tests {
    use super::*;

    #[test]
    fn symlink_loop_is_scanned_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/movie.mkv"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("a/loop")).unwrap();
        let lib = Library::scan(&[MediaRoot { name: "M".into(), path: dir.path().to_path_buf() }]);
        assert_eq!(lib.files().count(), 1);
    }
}

#[cfg(test)]
mod subtitle_match_tests {
    use super::*;

    fn subs_of(lib: &Library, title: &str) -> Vec<(Option<String>, String)> {
        let f = lib.files().find(|f| f.title == title).unwrap();
        f.subtitles
            .iter()
            .map(|(s, p)| (s.label.clone(), p.file_name().unwrap().to_string_lossy().into_owned()))
            .collect()
    }

    #[test]
    fn subtitles_match_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Серия 1.mkv"), b"v").unwrap();
        std::fs::write(dir.path().join("серия 1.RU.SRT"), b"s").unwrap();
        let lib = Library::scan(&[MediaRoot { name: "M".into(), path: dir.path().to_path_buf() }]);
        assert_eq!(subs_of(&lib, "Серия 1"), vec![(Some("ru".into()), "серия 1.RU.SRT".into())]);
    }

    #[test]
    fn subtitle_goes_to_the_longest_matching_video() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["Movie.mkv", "Movie.2019.mkv", "Movie.2019.srt", "Movie.en.srt"] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let lib = Library::scan(&[MediaRoot { name: "M".into(), path: dir.path().to_path_buf() }]);
        assert_eq!(subs_of(&lib, "Movie.2019"), vec![(None, "Movie.2019.srt".into())]);
        assert_eq!(subs_of(&lib, "Movie"), vec![(Some("en".into()), "Movie.en.srt".into())]);
    }
}

#[cfg(test)]
mod subtitle_layout_tests {
    use super::*;

    fn put(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    fn scan(root: &Path) -> Library {
        Library::scan(&[MediaRoot { name: "M".into(), path: root.to_path_buf() }])
    }

    fn subs_of(lib: &Library, title: &str) -> Vec<(Option<String>, String)> {
        let f = lib.files().find(|f| f.title == title).unwrap();
        f.subtitles
            .iter()
            .map(|(s, p)| (s.label.clone(), p.file_name().unwrap().to_string_lossy().into_owned()))
            .collect()
    }

    #[test]
    fn fuzzy_names_match() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Movie.2019.1080p.mkv");
        put(dir.path(), "Movie (2019) 1080p.srt");
        put(dir.path(), "movie_2019_1080p [EN forced].ass");
        let lib = scan(dir.path());
        assert_eq!(
            subs_of(&lib, "Movie.2019.1080p"),
            vec![
                (None, "Movie (2019) 1080p.srt".into()),
                (Some("en.forced".into()), "movie_2019_1080p [EN forced].ass".into())
            ]
        );
    }

    #[test]
    fn subs_folder_next_to_the_video() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Show/Episode 01.mkv");
        put(dir.path(), "Show/Episode 02.mkv");
        put(dir.path(), "Show/Subs/Episode 01.en.srt");
        put(dir.path(), "Show/SUBTITLES/Episode 02.srt");
        let lib = scan(dir.path());
        assert_eq!(
            subs_of(&lib, "Episode 01"),
            vec![(Some("en".into()), "Episode 01.en.srt".into())]
        );
        assert_eq!(subs_of(&lib, "Episode 02"), vec![(None, "Episode 02.srt".into())]);
    }

    #[test]
    fn per_video_folder_inside_subs_takes_any_name() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Show/Episode 01.mkv");
        put(dir.path(), "Show/Subs/Episode 01/2_English.srt");
        put(dir.path(), "Show/Subs/Episode 01/3_Russian.srt");
        put(dir.path(), "Show/Subs/Episode 02/2_English.srt");
        let lib = scan(dir.path());
        assert_eq!(
            subs_of(&lib, "Episode 01"),
            vec![
                (Some("2.english".into()), "2_English.srt".into()),
                (Some("3.russian".into()), "3_Russian.srt".into())
            ]
        );
    }

    #[test]
    fn subtitle_index_is_stable_when_files_are_added() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Movie.mkv");
        put(dir.path(), "Movie.ru.srt");
        let before = scan(dir.path()).files().next().unwrap().subtitles[0].0.index;
        put(dir.path(), "Movie.en.srt"); // sorts before "ru"
        let lib = scan(dir.path());
        let f = lib.files().next().unwrap();
        let ru = f.subtitles.iter().find(|(s, _)| s.label.as_deref() == Some("ru")).unwrap();
        assert_eq!(ru.0.index, before);
    }
}
