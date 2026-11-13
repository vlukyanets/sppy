mod common;
mod support;

use common::*;
use sppy_client_core::api::UiCommand;
use sppy_player::MediaSource;
use sppy_player::fake::{FakeCmd, FakeConfig};
use sppy_proto::*;
use sppy_server::{MediaRoot, ServerConfig};
use support::*;

#[tokio::test]
async fn file_item_streams_from_server() {
    let dir = tempfile::tempdir().unwrap();
    let media = tempfile::tempdir().unwrap();
    std::fs::write(media.path().join("Movie.mkv"), vec![7u8; 4096]).unwrap();
    std::fs::write(media.path().join("Movie.srt"), b"sub").unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: media.path().to_path_buf() }],
        ..test_config()
    };
    let server = start_server(dir.path(), cfg).await;
    let h = host(&server, FakeConfig::default()).await;
    h.send(UiCommand::GetLibrary);
    eventually("library", 5, || snap(&h.handle).library.is_some()).await;
    let lib = snap(&h.handle).library.unwrap();
    let LibraryNode::Dir { children, .. } = &lib[0] else { panic!() };
    let LibraryNode::File { file_id, .. } = &children[0] else { panic!() };
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: file_id.clone() }]));

    let prefix = format!("http://127.0.0.1:{}/media/{}?t=", server.local_addr.port(), file_id.0);
    let loaded = || {
        h.launcher.remote().and_then(|r| {
            r.commands().into_iter().find_map(|c| match c {
                FakeCmd::Load(MediaSource::Url(u)) if u.starts_with(&prefix) => Some(u),
                _ => None,
            })
        })
    };
    eventually("file loaded from server", 5, || loaded().is_some()).await;
    let url = loaded().unwrap();
    let res = reqwest::get(&url).await.unwrap();
    assert_eq!(res.status(), 200, "{url}");
    assert_eq!(res.bytes().await.unwrap().len(), 4096);
    let subs: Vec<_> = h
        .remote()
        .commands()
        .into_iter()
        .filter(|c| matches!(c, FakeCmd::SubAdd(MediaSource::Url(u)) if u.contains("/sub/") && u.contains("?t=")))
        .collect();
    assert_eq!(subs.len(), 1);
    eventually("progress reported as stream", 5, || {
        snap(&h.handle).progress.values().any(|p| p.delivery == Delivery::Stream)
    })
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn preferred_subtitle_language_is_added_first() {
    let dir = tempfile::tempdir().unwrap();
    let media = tempfile::tempdir().unwrap();
    std::fs::write(media.path().join("Movie.mkv"), vec![7u8; 1024]).unwrap();
    std::fs::write(media.path().join("Movie.en.srt"), b"en").unwrap();
    std::fs::write(media.path().join("Movie.ru.srt"), b"ru").unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: media.path().to_path_buf() }],
        ..test_config()
    };
    let server = start_server(dir.path(), cfg).await;
    let client_cfg = sppy_client_core::config::ClientConfig {
        nickname: "host".into(),
        subtitle_languages: vec!["ru".into()],
        ..Default::default()
    };
    let h = host_with(&server, FakeConfig::default(), client_cfg).await;
    let movie = sppy_server::library::file_id("Movies", std::path::Path::new("Movie.mkv"));
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie }]));
    eventually("subtitles added", 5, || {
        h.launcher.remote().is_some_and(|r| {
            r.commands().iter().filter(|c| matches!(c, FakeCmd::SubAdd(_))).count() == 2
        })
    })
    .await;
    let room = snap(&h.handle).room.unwrap();
    let ru =
        room.playlist[0].subtitles.iter().find(|s| s.label.as_deref() == Some("ru")).unwrap().index;
    let first = h.remote().commands().into_iter().find_map(|c| match c {
        FakeCmd::SubAdd(MediaSource::Url(u)) => Some(u),
        _ => None,
    });
    assert!(first.unwrap().contains(&format!("/sub/{ru}?")));
    server.shutdown().await;
}
