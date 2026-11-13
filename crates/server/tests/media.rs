mod common;

use std::path::Path;

use common::*;
use sppy_proto::*;
use sppy_server::{MediaRoot, ServerConfig, ServerHandle};

fn content() -> Vec<u8> {
    (0..1000u32).map(|i| (i % 256) as u8).collect()
}

/// Server with one media folder: `Movie.mkv` (+ `Movie.en.srt`).
async fn media_server(dir: &Path) -> (ServerHandle, tempfile::TempDir) {
    let media = tempfile::tempdir().unwrap();
    std::fs::write(media.path().join("Movie.mkv"), content()).unwrap();
    std::fs::write(media.path().join("Movie.en.srt"), b"1\n00:00:01,000 --> 00:00:02,000\nhi\n")
        .unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: media.path().to_path_buf() }],
        ..test_config()
    };
    (start_server(dir, cfg).await, media)
}

fn movie_id() -> FileId {
    sppy_server::library::file_id("Movies", Path::new("Movie.mkv"))
}

#[tokio::test]
async fn media_requires_valid_token() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _media) = media_server(dir.path()).await;
    let (_c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    let base = format!("http://{}/media/{}", server.local_addr, movie_id().0);
    let http = reqwest::Client::new();
    assert_eq!(http.get(&base).send().await.unwrap().status(), 401);
    assert_eq!(http.get(format!("{base}?t=wrong")).send().await.unwrap().status(), 401);
    let ok = http.get(format!("{base}?t={}", w.session_token)).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.bytes().await.unwrap().to_vec(), content());
    server.shutdown().await;
}

#[tokio::test]
async fn media_range_206() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _media) = media_server(dir.path()).await;
    let (_c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    let url = format!("http://{}/media/{}?t={}", server.local_addr, movie_id().0, w.session_token);
    let res = reqwest::Client::new().get(url).header("Range", "bytes=10-19").send().await.unwrap();
    assert_eq!(res.status(), 206);
    assert_eq!(res.headers()["content-range"], "bytes 10-19/1000");
    assert_eq!(res.bytes().await.unwrap().to_vec(), content()[10..20].to_vec());
    server.shutdown().await;
}

#[tokio::test]
async fn subtitle_served() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _media) = media_server(dir.path()).await;
    let (mut c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    c.send(ClientMsg::GetLibrary).await;
    let ServerMsg::Library { roots } =
        c.recv_until(|m| matches!(m, ServerMsg::Library { .. })).await
    else {
        unreachable!()
    };
    let LibraryNode::Dir { children, .. } = &roots[0] else { panic!() };
    let LibraryNode::File { subtitles, .. } = &children[0] else { panic!() };
    let index = subtitles[0].index;
    let http = reqwest::Client::new();
    let sub = format!(
        "http://{}/media/{}/sub/{index}?t={}",
        server.local_addr,
        movie_id().0,
        w.session_token
    );
    let res = http.get(sub).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.text().await.unwrap().contains("hi"));
    let missing = format!(
        "http://{}/media/{}/sub/{}?t={}",
        server.local_addr,
        movie_id().0,
        index ^ 1,
        w.session_token
    );
    assert_eq!(http.get(missing).send().await.unwrap().status(), 404);
    server.shutdown().await;
}

#[tokio::test]
async fn unknown_file_404() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _media) = media_server(dir.path()).await;
    let (_c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    let url = format!("http://{}/media/0000000000000000?t={}", server.local_addr, w.session_token);
    assert_eq!(reqwest::get(url).await.unwrap().status(), 404);
    server.shutdown().await;
}

#[tokio::test]
async fn token_invalid_after_leave() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _media) = media_server(dir.path()).await;
    let (mut c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    c.send(ClientMsg::Bye).await;
    let url = format!("http://{}/media/{}?t={}", server.local_addr, movie_id().0, w.session_token);
    let mut status = 0;
    for _ in 0..20 {
        status = reqwest::get(&url).await.unwrap().status().as_u16();
        if status == 401 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(status, 401);
    server.shutdown().await;
}

#[tokio::test]
async fn get_library_returns_tree_and_items_get_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _media) = media_server(dir.path()).await;
    let (mut h, _) =
        TestClient::join(server.local_addr, Auth::Host { token: server.host_token.clone() }, "h")
            .await;
    h.send(ClientMsg::GetLibrary).await;
    let ServerMsg::Library { roots } =
        h.recv_until(|m| matches!(m, ServerMsg::Library { .. })).await
    else {
        unreachable!()
    };
    let LibraryNode::Dir { name, children } = &roots[0] else { panic!() };
    assert_eq!(name, "Movies");
    assert!(
        matches!(&children[0], LibraryNode::File { name, subtitles, .. } if name == "Movie.mkv" && subtitles.len() == 1)
    );
    h.send(ClientMsg::PlaylistAdd {
        req_id: 1,
        sources: vec![Source::File { file_id: movie_id() }],
        after: None,
    })
    .await;
    // The fingerprint is hashed in the background: it comes with the add or shortly after.
    let ServerMsg::Room(r) = h
        .recv_until(|m| matches!(m, ServerMsg::Room(r) if r.playlist.first().is_some_and(|i| i.fingerprint.is_some())))
        .await
    else {
        unreachable!()
    };
    assert_eq!(r.playlist[0].fingerprint.as_ref().unwrap().size, 1000);
    assert_eq!(r.playlist[0].title, "Movie");
    assert_eq!(r.playlist[0].subtitles.len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn rescan_by_host_finds_new_files_guest_denied() {
    let dir = tempfile::tempdir().unwrap();
    let (server, media) = media_server(dir.path()).await;
    let (mut h, _) =
        TestClient::join(server.local_addr, Auth::Host { token: server.host_token.clone() }, "h")
            .await;
    let (mut g, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    g.send(ClientMsg::RescanLibrary { req_id: 9 }).await;
    let m = g.recv_until(|m| matches!(m, ServerMsg::Denied { .. })).await;
    assert_eq!(m, ServerMsg::Denied { req_id: 9, code: ErrorCode::NotHost });
    std::fs::write(media.path().join("New.mp4"), b"new").unwrap();
    h.send(ClientMsg::RescanLibrary { req_id: 1 }).await;
    let found = |m: &ServerMsg| match m {
        ServerMsg::Library { roots } => {
            matches!(&roots[0], LibraryNode::Dir { children, .. } if children.len() == 2)
        }
        _ => false,
    };
    g.recv_until(found).await;
    server.shutdown().await;
}

#[tokio::test]
async fn shutdown_does_not_hang_on_an_open_stream() {
    let dir = tempfile::tempdir().unwrap();
    let media = tempfile::tempdir().unwrap();
    std::fs::write(media.path().join("Big.mkv"), vec![0u8; 64 * 1024 * 1024]).unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: media.path().to_path_buf() }],
        ..test_config()
    };
    let server = start_server(dir.path(), cfg).await;
    let (_c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    let id = sppy_server::library::file_id("Movies", Path::new("Big.mkv"));
    let url = format!("http://{}/media/{}?t={}", server.local_addr, id.0, w.session_token);
    // Like mpv: an open-ended range request whose body is not being read.
    let _stalled =
        reqwest::Client::new().get(url).header("Range", "bytes=0-").send().await.unwrap();
    let done = tokio::time::timeout(std::time::Duration::from_secs(6), server.shutdown()).await;
    assert!(done.is_ok(), "shutdown hung");
}

#[tokio::test]
async fn vobsub_companion_is_served() {
    let dir = tempfile::tempdir().unwrap();
    let media = tempfile::tempdir().unwrap();
    std::fs::write(media.path().join("Movie.mkv"), b"video").unwrap();
    std::fs::write(media.path().join("Movie.idx"), b"index file").unwrap();
    std::fs::write(media.path().join("Movie.sub"), b"subpicture data").unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: media.path().to_path_buf() }],
        ..test_config()
    };
    let server = start_server(dir.path(), cfg).await;
    let (mut c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    c.send(ClientMsg::GetLibrary).await;
    let ServerMsg::Library { roots } =
        c.recv_until(|m| matches!(m, ServerMsg::Library { .. })).await
    else {
        unreachable!()
    };
    let LibraryNode::Dir { children, .. } = &roots[0] else { panic!() };
    let LibraryNode::File { subtitles, .. } = &children[0] else { panic!() };
    assert_eq!(subtitles.len(), 1);
    let base =
        format!("http://{}/media/{}/sub/{}", server.local_addr, movie_id().0, subtitles[0].index);
    let idx = reqwest::get(format!("{base}?t={}", w.session_token)).await.unwrap();
    assert_eq!(idx.bytes().await.unwrap().as_ref(), b"index file");
    let data = reqwest::get(format!("{base}/companion?t={}", w.session_token)).await.unwrap();
    assert_eq!(data.status(), 200);
    assert_eq!(data.bytes().await.unwrap().as_ref(), b"subpicture data");
    assert_eq!(reqwest::get(format!("{base}/companion")).await.unwrap().status(), 401);
    server.shutdown().await;
}
