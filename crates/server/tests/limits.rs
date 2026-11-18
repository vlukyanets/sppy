//! Connection-level limits: what a stranger or a misbehaving client can hold on the server.

mod common;

use std::time::Duration;

use common::*;
use sppy_proto::*;
use sppy_server::{MAX_MESSAGE_BURST, MAX_PENDING_CONNECTIONS};

/// Connections that never say Hello hold a slot for a while; there are only so many slots.
#[tokio::test]
async fn silent_connections_cannot_pile_up() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let mut silent = Vec::new();
    for _ in 0..MAX_PENDING_CONNECTIONS {
        silent.push(TestClient::connect_raw(server.local_addr).await);
    }
    let refused = tokio_tungstenite::connect_async(format!("ws://{}/ws", server.local_addr)).await;
    let Err(tokio_tungstenite::tungstenite::Error::Http(response)) = refused else {
        panic!("expected an HTTP refusal, got {refused:?}");
    };
    assert_eq!(response.status(), 503);

    // A closed one frees its slot.
    drop(silent.pop());
    let invite = Auth::Invite { token: server.invite_token() };
    let mut joined = None;
    for _ in 0..50 {
        if let Ok(Ok((ws, _))) = tokio::time::timeout(
            TIMEOUT,
            tokio_tungstenite::connect_async(format!("ws://{}/ws", server.local_addr)),
        )
        .await
        {
            joined = Some(ws);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(joined.is_some(), "the slot was not freed");
    drop(joined);

    // Joined participants do not take pending slots.
    drop(silent);
    let mut members = Vec::new();
    for i in 0..3 {
        let (c, _) = TestClient::join(server.local_addr, invite.clone(), &format!("p{i}")).await;
        members.push(c);
    }
    server.shutdown().await;
}

#[tokio::test]
async fn a_flooding_client_is_disconnected() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut host, _) =
        TestClient::join(server.local_addr, Auth::Host { token: server.host_token.clone() }, "h")
            .await;
    let (mut flood, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "f")
            .await;
    for _ in 0..MAX_MESSAGE_BURST * 2 {
        if !flood.try_send(ClientMsg::GetLibrary).await {
            break;
        }
    }
    // The server says why before closing, but a reset may drop that last message.
    flood.recv_until_closed().await;
    host.recv_until(|m| {
        matches!(m, ServerMsg::Room(r) if r.participants.iter().any(|p| p.nickname == "f" && !p.connected))
    })
    .await;
    server.shutdown().await;
}

/// A participant whose app stopped reading must not keep a server task stuck forever with
/// an ever-growing queue of messages for it.
#[tokio::test]
async fn a_client_that_stops_reading_is_disconnected() {
    let dir = tempfile::tempdir().unwrap();
    let config = sppy_server::ServerConfig { ping_timeout_ms: 1_000, ..test_config() };
    let server = start_server(dir.path(), config).await;
    let (mut host, _) =
        TestClient::join(server.local_addr, Auth::Host { token: server.host_token.clone() }, "h")
            .await;
    let (mut stuck, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "s")
            .await;
    // It keeps pinging, so only the full socket can tell that it is gone.
    let stuck = tokio::spawn(async move {
        loop {
            stuck.send(ClientMsg::Ping { client_time_ms: 0 }).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    // Every add sends the whole playlist to everyone: megabytes the stuck one never reads.
    let long = |i: u32| format!("https://example.org/{i}/{}", "a".repeat(4000));
    let gone = |m: &ServerMsg| matches!(m, ServerMsg::Room(r) if r.participants.iter().any(|p| p.nickname == "s" && !p.connected));
    let mut disconnected = false;
    for req in 1..=150 {
        host.send(ClientMsg::PlaylistAdd {
            req_id: req,
            sources: vec![Source::Url { url: long(req) }],
            after: None,
        })
        .await;
        host.send(ClientMsg::Ping { client_time_ms: 0 }).await;
        let room = host
            .recv_until(|m| {
                gone(m) || matches!(m, ServerMsg::Room(r) if r.playlist.len() == req as usize)
            })
            .await;
        if gone(&room) {
            disconnected = true;
            break;
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !disconnected {
        assert!(tokio::time::Instant::now() < deadline, "the stuck client is still connected");
        host.send(ClientMsg::Ping { client_time_ms: 0 }).await;
        disconnected = matches!(
            tokio::time::timeout(Duration::from_millis(300), host.recv()).await,
            Ok(m) if gone(&m)
        );
    }
    stuck.abort();
    server.shutdown().await;
}
