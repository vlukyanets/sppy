mod common;
mod support;

use std::path::{Path, PathBuf};

use common::*;
use sppy_client_core::api::{UiCommand, UiEvent};
use sppy_client_core::config::{ClientConfig, DeliveryMode};
use sppy_player::MediaSource;
use sppy_player::fake::{FakeCmd, FakeConfig};
use sppy_proto::*;
use sppy_server::{MediaRoot, ServerConfig, ServerHandle};
use support::*;

fn movie_bytes() -> Vec<u8> {
    (0..40_000u32).map(|i| (i % 241) as u8).collect()
}

async fn media_server(dir: &Path, media: &Path) -> ServerHandle {
    std::fs::write(media.join("Movie.mkv"), movie_bytes()).unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: media.to_path_buf() }],
        ..test_config()
    };
    start_server(dir, cfg).await
}

fn movie() -> FileId {
    sppy_server::library::file_id("Movies", Path::new("Movie.mkv"))
}

fn cfg(cache: &Path, mine: &Path) -> ClientConfig {
    ClientConfig {
        nickname: "host".into(),
        mode: DeliveryMode::Preload,
        cache_dir: Some(cache.to_path_buf()),
        local_media: vec![mine.to_path_buf()],
        download_backoff_ms: vec![10; 5],
        ..ClientConfig::default()
    }
}

fn loaded_paths(p: &Peer) -> Vec<PathBuf> {
    p.launcher
        .remote()
        .map(|r| {
            r.commands()
                .into_iter()
                .filter_map(|c| match c {
                    FakeCmd::Load(MediaSource::Path(p)) => Some(p),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn local_copy_used_instead_of_download() {
    let (dir, media, cache, mine) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let copy = mine.path().join("some/where/renamed.mkv");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, movie_bytes()).unwrap();
    let server = media_server(dir.path(), media.path()).await;
    let h = host_with(&server, FakeConfig::default(), cfg(cache.path(), mine.path())).await;
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie() }]));
    eventually("local copy loaded", 5, || loaded_paths(&h).contains(&copy)).await;
    eventually("delivery local", 5, || {
        snap(&h.handle).progress.values().any(|p| p.delivery == Delivery::Local)
    })
    .await;
    assert!(!cache.path().join("media").join(&movie().0).exists(), "nothing was downloaded");
    server.shutdown().await;
}

#[tokio::test]
async fn chosen_file_that_does_not_match_warns_but_plays() {
    let (dir, media, cache, mine) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let other = mine.path().join("other-rip.mkv");
    std::fs::write(&other, b"a different encode").unwrap();
    let server = media_server(dir.path(), media.path()).await;
    let config =
        ClientConfig { mode: DeliveryMode::Stream, ..cfg(cache.path(), Path::new("/nonexistent")) };
    let h = host_with(&server, FakeConfig::default(), config).await;
    let mut events = h.handle.subscribe_events();
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie() }]));
    eventually("fingerprint known", 5, || {
        snap(&h.handle)
            .room
            .is_some_and(|r| r.playlist.first().is_some_and(|i| i.fingerprint.is_some()))
    })
    .await;
    h.send(UiCommand::ChooseLocalFile { item_id: ItemId(1), path: other.clone() });
    eventually("chosen file loaded", 5, || loaded_paths(&h).contains(&other)).await;
    let mut warned = false;
    while let Ok(ev) = events.try_recv() {
        warned |= matches!(&ev, UiEvent::Error(t) if t.contains("does not match"));
    }
    assert!(warned);
    server.shutdown().await;
}

#[tokio::test]
async fn host_library_folders_count_as_local_copies() {
    let (dir, media, cache) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = media_server(dir.path(), media.path()).await;
    let mut config = cfg(cache.path(), Path::new("/nonexistent"));
    config.local_media.clear();
    config.host.media = vec![MediaRoot { name: "Movies".into(), path: media.path().to_path_buf() }];
    let h = host_with(&server, FakeConfig::default(), config).await;
    eventually("folders indexed", 5, || snap(&h.handle).local.indexed_files == Some(1)).await;
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie() }]));
    let original = media.path().join("Movie.mkv");
    eventually("library file loaded", 5, || loaded_paths(&h).contains(&original)).await;
    assert!(!cache.path().join("media").join(&movie().0).exists(), "nothing was downloaded");
    server.shutdown().await;
}

#[tokio::test]
async fn media_folders_changed_at_runtime_are_used() {
    let (dir, media, cache, mine) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let copy = mine.path().join("renamed.mkv");
    std::fs::write(&copy, movie_bytes()).unwrap();
    let server = media_server(dir.path(), media.path()).await;
    let mut config = cfg(cache.path(), mine.path());
    config.local_media.clear();
    let h = host_with(&server, FakeConfig::default(), config).await;
    eventually("nothing indexed", 5, || snap(&h.handle).local.indexed_files == Some(0)).await;
    h.send(UiCommand::SetMediaFolders(vec![mine.path().to_path_buf()]));
    eventually("new folder indexed", 5, || snap(&h.handle).local.indexed_files == Some(1)).await;
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie() }]));
    eventually("local copy loaded", 5, || loaded_paths(&h).contains(&copy)).await;
    assert!(!cache.path().join("media").join(&movie().0).exists(), "nothing was downloaded");
    server.shutdown().await;
}

#[tokio::test]
async fn next_item_with_a_local_copy_is_not_prefetched() {
    let (dir, media, cache) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let second: Vec<u8> = (0..30_000u32).map(|i| (i % 199) as u8).collect();
    std::fs::write(media.path().join("Second.mkv"), second).unwrap();
    let server = media_server(dir.path(), media.path()).await;
    let mut config = cfg(cache.path(), Path::new("/nonexistent"));
    config.local_media.clear();
    config.prefetch_next = true;
    config.host.media = vec![MediaRoot { name: "Movies".into(), path: media.path().to_path_buf() }];
    let h = host_with(&server, FakeConfig::default(), config).await;
    eventually("folders indexed", 5, || snap(&h.handle).local.indexed_files == Some(2)).await;
    let second_id = sppy_server::library::file_id("Movies", Path::new("Second.mkv"));
    h.send(UiCommand::PlaylistAdd(vec![
        Source::File { file_id: movie() },
        Source::File { file_id: second_id.clone() },
    ]));
    let original = media.path().join("Movie.mkv");
    eventually("first item loaded", 5, || loaded_paths(&h).contains(&original)).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let media_cache = cache.path().join("media");
    assert!(!media_cache.join(&second_id.0).exists(), "the next item was downloaded");
    server.shutdown().await;
}

#[tokio::test]
async fn rescan_finds_files_added_later() {
    let (dir, media, cache, mine) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let server = media_server(dir.path(), media.path()).await;
    let h = host_with(&server, FakeConfig::default(), cfg(cache.path(), mine.path())).await;
    eventually("empty folder indexed", 5, || snap(&h.handle).local.indexed_files == Some(0)).await;
    std::fs::write(mine.path().join("late.mkv"), movie_bytes()).unwrap();
    h.send(UiCommand::SetMediaFolders(vec![mine.path().to_path_buf()]));
    eventually("rescanned", 5, || snap(&h.handle).local.indexed_files == Some(1)).await;
    server.shutdown().await;
}
