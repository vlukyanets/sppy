mod common;
mod support;

use std::path::Path;

use common::*;
use sppy_client_core::api::{ConnState, UiCommand};
use sppy_player::MediaSource;
use sppy_player::fake::{FakeCmd, FakeConfig};
use sppy_proto::*;
use sppy_server::{MediaRoot, ServerConfig};
use support::*;

fn last_url(p: &Peer) -> Option<String> {
    p.launcher.remote()?.commands().into_iter().rev().find_map(|c| match c {
        FakeCmd::Load(MediaSource::Url(u)) => Some(u),
        _ => None,
    })
}

#[tokio::test]
async fn streamed_file_reloads_with_new_token_after_server_restart() {
    let (dir, media) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(media.path().join("Movie.mkv"), vec![1u8; 10_000]).unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: media.path().to_path_buf() }],
        ..test_config()
    };
    let server = start_server(dir.path(), cfg.clone()).await;
    let addr = server.local_addr;
    let h = host(&server, FakeConfig::default()).await;
    let movie = sppy_server::library::file_id("Movies", Path::new("Movie.mkv"));
    h.send(UiCommand::PlaylistAdd(vec![Source::File { file_id: movie }]));
    eventually("streaming", 5, || last_url(&h).is_some()).await;
    let first = last_url(&h).unwrap();

    server.shutdown().await;
    let server = start_server(dir.path(), ServerConfig { listen: addr, ..cfg }).await;
    eventually("reconnected", 15, || snap(&h.handle).connection == ConnState::Connected).await;
    eventually("reloaded with the new token", 5, || last_url(&h).is_some_and(|u| u != first)).await;
    let status = reqwest::get(last_url(&h).unwrap()).await.unwrap().status();
    assert_eq!(status, 200);
    server.shutdown().await;
}

#[tokio::test]
async fn closed_player_not_reopened_after_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let proxy = Proxy::start(server.local_addr).await;
    let a = host(&server, FakeConfig::default()).await;
    let launcher = sppy_player::fake::FakeLauncher::new(FakeConfig::default());
    let handle = sppy_client_core::api::spawn_client(
        sppy_client_core::config::ClientConfig { nickname: "bob".into(), ..Default::default() },
        launcher.clone(),
    );
    let b = Peer { handle, launcher };
    b.send(UiCommand::Connect {
        address: format!("sppy://{}/{}", proxy.addr, server.invite_token()),
        nickname: "bob".into(),
    });
    eventually("b connected", 5, || snap(&b.handle).connection == ConnState::Connected).await;
    add_and_play(&a, &["u1"]).await;
    eventually("b playing", 5, || b.synced()).await;
    b.remote().exit(Some(0));
    eventually("b closed", 3, || !snap(&b.handle).player_open).await;
    proxy.cut();
    eventually("b lost the connection", 10, || {
        matches!(snap(&b.handle).connection, ConnState::Reconnecting { .. })
    })
    .await;
    eventually("b back", 10, || snap(&b.handle).connection == ConnState::Connected).await;
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    assert_eq!(b.launcher.launches(), 1, "a player the user closed stays closed");
    server.shutdown().await;
}
