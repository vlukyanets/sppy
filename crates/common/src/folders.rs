//! Shared media folders: checks for a new folder, and the `[[media]]` list in `server.toml`.

use std::path::Path;

use sppy_proto::MediaFolderProblem;

use crate::config::MediaRoot;

/// A name for a new folder: its own name, made unique with " 2", " 3", ….
pub fn default_name(existing: &[MediaRoot], path: &Path) -> String {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Media".to_owned());
    let taken = |n: &str| existing.iter().any(|r| r.name == n);
    if !taken(&base) {
        return base;
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).expect("some number is free")
}

/// Check a folder before sharing it. `name` is trimmed; empty means [`default_name`].
pub fn new_folder(
    existing: &[MediaRoot],
    path: &Path,
    name: Option<&str>,
) -> Result<MediaRoot, MediaFolderProblem> {
    if !path.is_absolute() {
        return Err(MediaFolderProblem::NotAbsolute);
    }
    let meta = std::fs::metadata(path).map_err(|_| MediaFolderProblem::NotFound)?;
    if !meta.is_dir() {
        return Err(MediaFolderProblem::NotAFolder);
    }
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let wanted = real(path);
    if existing.iter().any(|r| r.path == path || real(&r.path) == wanted) {
        return Err(MediaFolderProblem::AlreadyShared);
    }
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if existing.iter().any(|r| r.name == n) => {
            return Err(MediaFolderProblem::NameTaken);
        }
        Some(n) => n.to_owned(),
        None => default_name(existing, path),
    };
    Ok(MediaRoot { name, path: path.to_path_buf() })
}

/// Replace the `[[media]]` tables of a `server.toml` text; everything else (other settings,
/// comments, order) stays as it was.
pub fn set_media_in_toml(text: &str, roots: &[MediaRoot]) -> Result<String, toml_edit::TomlError> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    if roots.is_empty() {
        doc.remove("media");
    } else {
        let mut tables = toml_edit::ArrayOfTables::new();
        for r in roots {
            let mut t = toml_edit::Table::new();
            t.insert("name", toml_edit::value(r.name.as_str()));
            t.insert("path", toml_edit::value(r.path.to_string_lossy().as_ref()));
            tables.push(t);
        }
        doc.insert("media", toml_edit::Item::ArrayOfTables(tables));
    }
    Ok(doc.to_string())
}

/// Save the folder list into the config file at `path` (created if missing).
pub fn save_media_to_file(path: &Path, roots: &[MediaRoot]) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let new = set_media_in_toml(&text, roots)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, new)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    fn root(name: &str, path: &Path) -> MediaRoot {
        MediaRoot { name: name.into(), path: path.to_path_buf() }
    }

    #[test]
    fn default_name_is_the_folder_name_made_unique() {
        let existing =
            vec![root("Movies", Path::new("/a/Movies")), root("Movies 2", Path::new("/b"))];
        assert_eq!(default_name(&existing, Path::new("/c/Series")), "Series");
        assert_eq!(default_name(&existing, Path::new("/c/Movies")), "Movies 3");
        assert_eq!(default_name(&[], Path::new("/")), "Media");
    }

    #[test]
    fn new_folder_checks_the_path_and_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let movies = dir.path().join("Movies");
        std::fs::create_dir(&movies).unwrap();
        let file = dir.path().join("a.mkv");
        std::fs::write(&file, b"x").unwrap();

        let ok = new_folder(&[], &movies, None).unwrap();
        assert_eq!(ok, root("Movies", &movies));
        assert_eq!(new_folder(&[], &movies, Some("  Films ")).unwrap().name, "Films");

        assert_eq!(
            new_folder(&[], Path::new("rel/dir"), None),
            Err(MediaFolderProblem::NotAbsolute)
        );
        let missing = dir.path().join("nope");
        assert_eq!(new_folder(&[], &missing, None), Err(MediaFolderProblem::NotFound));
        assert_eq!(new_folder(&[], &file, None), Err(MediaFolderProblem::NotAFolder));
        let existing = vec![ok];
        assert_eq!(new_folder(&existing, &movies, None), Err(MediaFolderProblem::AlreadyShared));
        let same_via_dots = dir.path().join("Movies/../Movies");
        assert_eq!(
            new_folder(&existing, &same_via_dots, None),
            Err(MediaFolderProblem::AlreadyShared)
        );
        let other = dir.path().join("Other");
        std::fs::create_dir(&other).unwrap();
        assert_eq!(
            new_folder(&existing, &other, Some("Movies")),
            Err(MediaFolderProblem::NameTaken)
        );
    }

    #[test]
    fn media_list_is_replaced_and_the_rest_kept() {
        let text = "# my server\nlisten = \"0.0.0.0:9000\" # port for friends\n\n\
                    [[media]]\nname = \"Old\"\npath = \"/old\"\n";
        let roots =
            [root("Movies", Path::new("/srv/movies")), root("Series", Path::new("/srv/tv"))];
        let new = set_media_in_toml(text, &roots).unwrap();
        assert!(new.starts_with("# my server\nlisten = \"0.0.0.0:9000\" # port for friends\n"));
        assert!(!new.contains("Old"));
        let parsed = ServerConfig::from_toml(&new).unwrap();
        assert_eq!(parsed.listen.port(), 9000);
        assert_eq!(parsed.media, roots);

        let emptied = set_media_in_toml(&new, &[]).unwrap();
        assert!(!emptied.contains("media"));
        assert_eq!(ServerConfig::from_toml(&emptied).unwrap().listen.port(), 9000);
    }

    #[test]
    fn saving_creates_or_updates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/server.toml");
        save_media_to_file(&path, &[root("M", Path::new("/m"))]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(ServerConfig::from_toml(&text).unwrap().media, [root("M", Path::new("/m"))]);
        assert!(!dir.path().join("sub/server.toml.tmp").exists());
    }
}
