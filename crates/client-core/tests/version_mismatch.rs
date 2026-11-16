//! A server of another protocol version is refused once, with an explanation.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use sppy_client_core::api::{ConnState, UiCommand, UiEvent, spawn_client};
use sppy_client_core::config::ClientConfig;
use sppy_player::fake::{FakeConfig, FakeLauncher};
use sppy_proto::*;
use support::*;

/// Answers every hello with `reply` and counts connections.
async fn start_fake(reply: ServerMsg) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let connections = Arc::new(AtomicUsize::new(0));
    let (count, reply) = (connections.clone(), Arc::new(reply));
    let app = axum::Router::new().route(
        "/ws",
        get(move |up: WebSocketUpgrade| {
            let (count, reply) = (count.clone(), reply.clone());
            async move {
                up.on_upgrade(move |mut ws: WebSocket| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    if let Some(Ok(Message::Text(_))) = ws.recv().await {
                        let _ = ws.send(Message::text(encode(&*reply))).await;
                    }
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, connections)
}

#[tokio::test]
async fn a_server_of_another_version_is_refused_once_and_explained() {
    let reject = ServerMsg::Reject {
        code: ErrorCode::VersionMismatch { server_protocol: 2 },
        message: "server protocol v2, client v3".into(),
    };
    let (addr, connections) = start_fake(reject).await;
    let handle = spawn_client(
        ClientConfig { nickname: "me".into(), ..ClientConfig::default() },
        FakeLauncher::new(FakeConfig::default()),
    );
    let mut events = handle.subscribe_events();
    handle
        .commands
        .send(UiCommand::Connect {
            address: format!("sppy://{addr}/invite"),
            nickname: "me".into(),
        })
        .unwrap();
    eventually("rejected", 5, || {
        snap(&handle).connection
            == ConnState::Rejected(ErrorCode::VersionMismatch { server_protocol: 2 })
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(connections.load(Ordering::SeqCst), 1, "does not keep reconnecting");
    let mut shown = Vec::new();
    while let Ok(ev) = events.try_recv() {
        if let UiEvent::Error(t) = ev {
            shown.push(t);
        }
    }
    assert!(
        shown.iter().any(|t| t.contains("different version")),
        "the user is told what is wrong: {shown:?}"
    );
}
