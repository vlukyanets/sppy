//! Shared folders managed by the host over the protocol (and by the embedding app).

mod common;

use std::path::{Path, PathBuf};

use common::*;
use sppy_proto::*;
use sppy_server::{MediaRoot, ServerConfig, ServerHandle, StartOptions};

struct Setup {
    _dir: tempfile::TempDir,
    movies: tempfile::TempDir,
    series: tempfile::TempDir,
    config_file: PathBuf,
    server: ServerHandle,
}

async fn setup(allow: bool) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let (movies, series) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(movies.path().join("Movie.mkv"), b"movie").unwrap();
    std::fs::write(series.path().join("Episode.mkv"), b"episode").unwrap();
    let config_file = dir.path().join("server.toml");
    std::fs::write(&config_file, "# keep me\nstall_hold_secs = 7\n").unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: movies.path().to_path_buf() }],
        allow_remote_media_changes: allow,
        ..test_config()
    };
    let options = StartOptions { config_file: Some(config_file.clone()) };
    let server = sppy_server::start_with(cfg, dir.path().join("data"), options).await.unwrap();
    Setup { _dir: dir, movies, series, config_file, server }
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
                path: path_str(s.movies.path()),
                available: true,
            }],
            editable: true,
        }
    );
    s.server.shutdown().await;
}

#[tokio::test]
async fn host_adds_and_removes_folders_and_the_config_keeps_them() {
    let s = setup(true).await;
    let mut h = host(&s).await;
    h.recv_until(is_folders).await;
    h.send(ClientMsg::AddMediaFolder { req_id: 1, path: path_str(s.series.path()), name: None })
        .await;
    let series_name = s.series.path().file_name().unwrap().to_string_lossy().into_owned();
    let m = h.recv_until(is_folders).await;
    assert_eq!(folder_names(&m), ["Movies".to_owned(), series_name.clone()]);
    let both = vec!["Movies".to_owned(), series_name.clone()];
    h.recv_until(|m| library_roots(m).as_ref() == Some(&both)).await;

    let text = std::fs::read_to_string(&s.config_file).unwrap();
    assert!(text.starts_with("# keep me\nstall_hold_secs = 7\n"), "{text}");
    let saved = ServerConfig::from_toml(&text).unwrap();
    assert_eq!(saved.stall_hold_secs, 7);
    assert_eq!(saved.media.len(), 2);
    assert_eq!(saved.media[1].path, s.series.path());

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
                path: path_str(&s.series.path().join("missing")),
                name: None,
            },
            2,
            MediaFolderProblem::NotFound,
        ),
        (
            ClientMsg::AddMediaFolder { req_id: 3, path: path_str(s.movies.path()), name: None },
            3,
            MediaFolderProblem::AlreadyShared,
        ),
        (
            ClientMsg::AddMediaFolder {
                req_id: 4,
                path: path_str(s.series.path()),
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
    g.send(ClientMsg::AddMediaFolder { req_id: 1, path: path_str(s.series.path()), name: None })
        .await;
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
