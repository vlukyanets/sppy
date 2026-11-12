mod common;

use common::*;
use sppy_proto::*;

#[tokio::test]
async fn state_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut h, _) = TestClient::join(
        server.local_addr,
        Auth::Host { token: server.host_token.clone() },
        "host",
    )
    .await;
    h.send(ClientMsg::SetControlMode { req_id: 1, mode: ControlMode::HostOnly }).await;
    h.send(ClientMsg::PlaylistAdd {
        req_id: 2,
        sources: vec![Source::Url { url: "u1".into() }, Source::Url { url: "u2".into() }],
        after: None,
    })
    .await;
    h.send(ClientMsg::Play { req_id: 3, position_ms: None }).await;
    h.send(ClientMsg::Seek { req_id: 4, position_ms: 30_000 }).await;
    h.recv_until(|m| matches!(m, ServerMsg::Playback(p) if p.position_ms == 30_000)).await;
    let host_token = server.host_token.clone();
    server.shutdown().await;

    let server = start_server(dir.path(), test_config()).await;
    assert_eq!(server.host_token, host_token);
    let (_h, w) =
        TestClient::join(server.local_addr, Auth::Host { token: host_token }, "host").await;
    let urls: Vec<_> = w.room.playlist.iter().map(|i| i.source.clone()).collect();
    assert_eq!(urls, vec![Source::Url { url: "u1".into() }, Source::Url { url: "u2".into() }]);
    assert_eq!(w.room.control_mode, ControlMode::HostOnly);
    assert_eq!(w.playback.item_id, Some(ItemId(1)));
    assert!(w.playback.paused);
    assert!((30_000..31_000).contains(&w.playback.position_ms), "{}", w.playback.position_ms);
    server.shutdown().await;
}

#[tokio::test]
async fn corrupt_state_renamed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("state.json"), "{").unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (_c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    assert!(w.room.playlist.is_empty());
    assert!(dir.path().join("state.json.bad").exists());
    assert!(!dir.path().join("state.json").exists());
    server.shutdown().await;
}
