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

fn data() -> Vec<u8> {
    (0..50_000u32).map(|i| (i % 253) as u8).collect()
}

async fn media_server(dir: &Path, media: &Path) -> ServerHandle {
    std::fs::write(media.join("Movie.mkv"), data()).unwrap();
    std::fs::write(media.join("Movie.srt"), b"sub").unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: media.to_path_buf() }],
        ..test_config()
    };
    start_server(dir, cfg).await
}

fn preload_config(cache: &Path) -> ClientConfig {
    ClientConfig {
        nickname: "host".into(),
        mode: DeliveryMode::Preload,
        cache_dir: Some(cache.to_path_buf()),
        download_backoff_ms: vec![10; 5],
        ..ClientConfig::default()
    }
}

fn movie() -> FileId {
    sppy_server::library::file_id("Movies", Path::new("Movie.mkv"))
}

fn loaded_path(p: &Peer) -> Option<PathBuf> {
    p.launcher.remote()?.commands().into_iter().find_map(|c| match c {
        FakeCmd::Load(MediaSource::Path(p)) => Some(p),
        _ => None,
    })
}

#[tokio::test]
async fn preload_mode_loads_local_file_after_download() {
    let (dir, media, cache) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = media_server(dir.path(), media.path()).await;
    let h = host_with(&server, FakeConfig::default(), preload_config(cache.path())).await;
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie() }]));
    eventually("local file loaded", 5, || loaded_path(&h).is_some()).await;
    let path = loaded_path(&h).unwrap();
    assert_eq!(path, cache.path().join("media").join(&movie().0));
    assert_eq!(std::fs::read(&path).unwrap(), data());
    assert!(
        h.remote().commands().iter().any(|c| matches!(c, FakeCmd::SubAdd(MediaSource::Path(_))))
    );
    eventually("delivery is local", 5, || {
        snap(&h.handle).progress.values().any(|p| p.delivery == Delivery::Local)
    })
    .await;
    eventually("hold released", 5, || snap(&h.handle).playback.is_some_and(|p| p.hold.is_none()))
        .await;
    server.shutdown().await;
}

#[tokio::test]
async fn preload_uses_completed_cache_entry() {
    let (dir, media, cache) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = media_server(dir.path(), media.path()).await;
    std::fs::create_dir_all(cache.path().join("media")).unwrap();
    std::fs::write(cache.path().join("media").join(&movie().0), b"cached").unwrap();
    let h = host_with(&server, FakeConfig::default(), preload_config(cache.path())).await;
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie() }]));
    eventually("cached file loaded", 5, || loaded_path(&h).is_some()).await;
    assert_eq!(std::fs::read(loaded_path(&h).unwrap()).unwrap(), b"cached");
    server.shutdown().await;
}

#[tokio::test]
async fn preload_failure_falls_back_to_stream() {
    let (dir, media, cache) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = media_server(dir.path(), media.path()).await;
    // A directory where the .part file should go makes every attempt fail.
    std::fs::create_dir_all(cache.path().join("media").join(format!("{}.part", movie().0)))
        .unwrap();
    let h = host_with(&server, FakeConfig::default(), preload_config(cache.path())).await;
    let mut events = h.handle.subscribe_events();
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie() }]));
    let streamed = || {
        h.launcher.remote().is_some_and(|r| {
            r.commands()
                .iter()
                .any(|c| matches!(c, FakeCmd::Load(MediaSource::Url(u)) if u.contains("/media/")))
        })
    };
    eventually("fell back to stream", 5, streamed).await;
    eventually("delivery is stream", 5, || {
        snap(&h.handle).progress.values().any(|p| p.delivery == Delivery::Stream)
    })
    .await;
    let mut saw_notice = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while !saw_notice && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(UiEvent::Notice(t))) =
            tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await
        {
            saw_notice = t.contains("download failed");
        }
    }
    assert!(saw_notice, "everyone sees the fallback notice");
    server.shutdown().await;
}
