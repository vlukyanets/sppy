//! Shared folders managed by the host over the protocol (and by the embedding app).

mod common;

use std::path::{Path, PathBuf};

use common::*;
use sppy_proto::*;
use sppy_server::{MediaRoot, ServerConfig, ServerHandle, StartOptions};

struct Setup {
    _dir: tempfile::TempDir,
    /// `media_root` unless a test sets another one; holds `movies` and `series`.
    base: tempfile::TempDir,
    movies: PathBuf,
    series: PathBuf,
    config_file: PathBuf,
    server: ServerHandle,
}

async fn setup(allow: bool) -> Setup {
    setup_with(allow, |base| Some(base.to_path_buf())).await
}

/// A headless server sharing `movies`; folders may be added only inside `media_root`.
async fn setup_with(allow: bool, media_root: impl FnOnce(&Path) -> Option<PathBuf>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let (movies, series) = (base.path().join("movies"), base.path().join("series"));
    std::fs::create_dir(&movies).unwrap();
    std::fs::create_dir(&series).unwrap();
    std::fs::write(movies.join("Movie.mkv"), b"movie").unwrap();
    std::fs::write(series.join("Episode.mkv"), b"episode").unwrap();
    let config_file = dir.path().join("server.toml");
    std::fs::write(&config_file, "# keep me\nstall_hold_secs = 7\n").unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: movies.clone() }],
        allow_remote_media_changes: allow,
        media_root: media_root(base.path()),
        ..test_config()
    };
    let options = StartOptions { config_file: Some(config_file.clone()), require_media_root: true };
    let server = sppy_server::start_with(cfg, dir.path().join("data"), options).await.unwrap();
    Setup { _dir: dir, base, movies, series, config_file, server }
}

async fn host(s: &Setup) -> TestClient {
    let auth = Auth::Host { token: s.server.host_token.clone() };
    TestClient::join(s.server.local_addr, auth, "h").await.0
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn is_folders(m: &ServerMsg) -> bool {
    matches!(m, ServerMsg::MediaFolders { .. })
}

fn folder_names(m: &ServerMsg) -> Vec<String> {
    match m {
        ServerMsg::MediaFolders { folders, .. } => folders.iter().map(|f| f.name.clone()).collect(),
        other => panic!("expected MediaFolders, got {other:?}"),
    }
}

fn library_roots(m: &ServerMsg) -> Option<Vec<String>> {
    match m {
        ServerMsg::Library { roots } => Some(
            roots
                .iter()
                .map(|r| match r {
                    LibraryNode::Dir { name, .. } | LibraryNode::File { name, .. } => name.clone(),
                })
                .collect(),
        ),
        _ => None,
    }
}

#[tokio::test]
async fn host_sees_the_folders_on_join() {
    let s = setup(true).await;
    let mut h = host(&s).await;
    let m = h.recv_until(is_folders).await;
    assert_eq!(
        m,
        ServerMsg::MediaFolders {
            folders: vec![MediaFolder {
                name: "Movies".into(),
                path: path_str(&s.movies),
                available: true,
            }],
            editable: true,
            adding: FolderAdding::Within { root: path_str(s.base.path()) },
        }
    );
    s.server.shutdown().await;
}

#[tokio::test]
async fn host_adds_and_removes_folders_and_the_config_keeps_them() {
    let s = setup(true).await;
    let mut h = host(&s).await;
    h.recv_until(is_folders).await;
    h.send(ClientMsg::AddMediaFolder { req_id: 1, path: path_str(&s.series), name: None }).await;
    let series_name = s.series.file_name().unwrap().to_string_lossy().into_owned();
    let m = h.recv_until(is_folders).await;
    assert_eq!(folder_names(&m), ["Movies".to_owned(), series_name.clone()]);
    let both = vec!["Movies".to_owned(), series_name.clone()];
    h.recv_until(|m| library_roots(m).as_ref() == Some(&both)).await;

    let text = std::fs::read_to_string(&s.config_file).unwrap();
    assert!(text.starts_with("# keep me\nstall_hold_secs = 7\n"), "{text}");
    let saved = ServerConfig::from_toml(&text).unwrap();
    assert_eq!(saved.stall_hold_secs, 7);
    assert_eq!(saved.media.len(), 2);
    assert_eq!(saved.media[1].path, s.series);

    h.send(ClientMsg::RemoveMediaFolder { req_id: 2, name: "Movies".into() }).await;
    let m = h.recv_until(is_folders).await;
    assert_eq!(folder_names(&m), std::slice::from_ref(&series_name));
    let only = vec![series_name];
    h.recv_until(|m| library_roots(m).as_ref() == Some(&only)).await;
    let saved = ServerConfig::from_toml(&std::fs::read_to_string(&s.config_file).unwrap()).unwrap();
    assert_eq!(saved.media.len(), 1);
    s.server.shutdown().await;
}

#[tokio::test]
async fn bad_folder_requests_are_refused_with_a_reason() {
    let s = setup(true).await;
    let mut h = host(&s).await;
    h.recv_until(is_folders).await;
    let denied = |problem| ErrorCode::MediaFolder { problem };
    let cases = [
        (
            ClientMsg::AddMediaFolder { req_id: 1, path: "relative/dir".into(), name: None },
            1,
            MediaFolderProblem::NotAbsolute,
        ),
        (
            ClientMsg::AddMediaFolder {
                req_id: 2,
                path: path_str(&s.series.join("missing")),
                name: None,
            },
            2,
            MediaFolderProblem::NotFound,
        ),
        (
            ClientMsg::AddMediaFolder { req_id: 3, path: path_str(&s.movies), name: None },
            3,
            MediaFolderProblem::AlreadyShared,
        ),
        (
            ClientMsg::AddMediaFolder {
                req_id: 4,
                path: path_str(&s.series),
                name: Some("Movies".into()),
            },
            4,
            MediaFolderProblem::NameTaken,
        ),
        (
            ClientMsg::RemoveMediaFolder { req_id: 5, name: "Nope".into() },
            5,
            MediaFolderProblem::UnknownFolder,
        ),
    ];
    for (msg, req_id, problem) in cases {
        h.send(msg).await;
        let m = h.recv_until(|m| matches!(m, ServerMsg::Denied { .. })).await;
        assert_eq!(m, ServerMsg::Denied { req_id, code: denied(problem) });
    }
    s.server.shutdown().await;
}

#[tokio::test]
async fn guests_cannot_change_folders() {
    let s = setup(true).await;
    let auth = Auth::Invite { token: s.server.invite_token() };
    let (mut g, _) = TestClient::join(s.server.local_addr, auth, "g").await;
    g.send(ClientMsg::AddMediaFolder { req_id: 1, path: path_str(&s.series), name: None }).await;
    let m = g.recv_until(|m| matches!(m, ServerMsg::Denied { .. })).await;
    assert_eq!(m, ServerMsg::Denied { req_id: 1, code: ErrorCode::NotHost });
    s.server.shutdown().await;
}

#[tokio::test]
async fn changes_can_be_disabled_in_the_config() {
    let s = setup(false).await;
    let mut h = host(&s).await;
    let m = h.recv_until(is_folders).await;
    assert!(matches!(m, ServerMsg::MediaFolders { editable: false, .. }));
    h.send(ClientMsg::RemoveMediaFolder { req_id: 1, name: "Movies".into() }).await;
    let m = h.recv_until(|m| matches!(m, ServerMsg::Denied { .. })).await;
    let code = ErrorCode::MediaFolder { problem: MediaFolderProblem::ChangesDisabled };
    assert_eq!(m, ServerMsg::Denied { req_id: 1, code });
    s.server.shutdown().await;
}

#[tokio::test]
async fn the_embedding_app_replaces_the_folders_directly() {
    let dir = tempfile::tempdir().unwrap();
    let (movies, series) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(series.path().join("Episode.mkv"), b"episode").unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: movies.path().to_path_buf() }],
        ..test_config()
    };
    let server = start_server(dir.path(), cfg).await;
    let auth = Auth::Host { token: server.host_token.clone() };
    let (mut h, _) = TestClient::join(server.local_addr, auth, "h").await;
    h.recv_until(is_folders).await;
    server.set_media_folders(vec![MediaRoot { name: "TV".into(), path: series.path().into() }]);
    let m = h.recv_until(is_folders).await;
    assert_eq!(folder_names(&m), ["TV".to_owned()]);
    h.recv_until(|m| library_roots(m) == Some(vec!["TV".to_owned()])).await;
    assert!(!dir.path().join("server.toml").exists(), "nothing written without a config file");
    server.shutdown().await;
}

#[tokio::test]
async fn a_link_with_the_host_token_makes_you_the_host() {
    let s = setup(true).await;
    let auth = Auth::Invite { token: s.server.host_token.clone() };
    let (mut admin, welcome) = TestClient::join(s.server.local_addr, auth, "admin").await;
    assert!(welcome.is_host);
    admin.recv_until(is_folders).await;
    let auth = Auth::Invite { token: s.server.invite_token() };
    let (_g, welcome) = TestClient::join(s.server.local_addr, auth, "g").await;
    assert!(!welcome.is_host);
    s.server.shutdown().await;
}

#[tokio::test]
async fn without_a_media_root_folders_can_be_removed_but_not_added() {
    let s = setup_with(true, |_| None).await;
    let mut h = host(&s).await;
    let m = h.recv_until(is_folders).await;
    assert!(matches!(m, ServerMsg::MediaFolders { adding: FolderAdding::NoMediaRoot, .. }));
    h.send(ClientMsg::AddMediaFolder { req_id: 1, path: path_str(&s.series), name: None }).await;
    let m = h.recv_until(|m| matches!(m, ServerMsg::Denied { .. })).await;
    let code = ErrorCode::MediaFolder { problem: MediaFolderProblem::NoMediaRoot };
    assert_eq!(m, ServerMsg::Denied { req_id: 1, code });
    h.send(ClientMsg::RemoveMediaFolder { req_id: 2, name: "Movies".into() }).await;
    let m = h.recv_until(is_folders).await;
    assert!(folder_names(&m).is_empty());
    s.server.shutdown().await;
}

#[tokio::test]
async fn folders_outside_the_media_root_are_refused() {
    let base = tempfile::tempdir().unwrap();
    let inside = base.path().join("Series");
    std::fs::create_dir(&inside).unwrap();
    let root = base.path().to_path_buf();
    let s = setup_with(true, |_| Some(root)).await;
    let mut h = host(&s).await;
    h.recv_until(is_folders).await;
    // The file system root is `/`, or `C:\` on Windows, where `/` is not absolute.
    let fs_root = base.path().ancestors().last().unwrap();
    let outside = [path_str(&s.series), path_str(fs_root), path_str(&inside.join("../.."))];
    for (i, path) in outside.into_iter().enumerate() {
        let req_id = i as u32 + 1;
        h.send(ClientMsg::AddMediaFolder { req_id, path, name: None }).await;
        let m = h.recv_until(|m| matches!(m, ServerMsg::Denied { .. })).await;
        let code = ErrorCode::MediaFolder { problem: MediaFolderProblem::OutsideMediaRoot };
        assert_eq!(m, ServerMsg::Denied { req_id, code });
    }
    h.send(ClientMsg::AddMediaFolder { req_id: 10, path: path_str(&inside), name: None }).await;
    let m = h.recv_until(is_folders).await;
    assert_eq!(folder_names(&m), ["Movies".to_owned(), "Series".to_owned()]);
    s.server.shutdown().await;
}

/// The app's own server shares the app user's folders: no `media_root` needed.
#[tokio::test]
async fn the_embedded_server_may_add_anywhere() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let auth = Auth::Host { token: server.host_token.clone() };
    let (mut h, _) = TestClient::join(server.local_addr, auth, "h").await;
    let m = h.recv_until(is_folders).await;
    assert!(matches!(m, ServerMsg::MediaFolders { adding: FolderAdding::Anywhere, .. }));
    server.shutdown().await;
}
