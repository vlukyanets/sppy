mod common;

use std::time::Duration;

use common::*;
use sppy_client_core::connection::{AuthToken, ConnEvent, spawn_connection};
use sppy_client_core::link::parse_address;
use sppy_proto::*;

async fn next_event(h: &mut sppy_client_core::connection::ConnectionHandle) -> ConnEvent {
    tokio::time::timeout(Duration::from_secs(10), h.events.recv())
        .await
        .expect("timed out")
        .expect("connection task alive")
}

async fn wait_connected(h: &mut sppy_client_core::connection::ConnectionHandle) -> Welcome {
    loop {
        if let ConnEvent::Connected { welcome } = next_event(h).await {
            return welcome;
        }
    }
}

#[tokio::test]
async fn connects_and_estimates_clock() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let addr = parse_address(&format!("sppy://{}", server.local_addr)).unwrap();
    let mut h = spawn_connection(addr, AuthToken::Invite(server.invite_token()), "alice".into());
    let w = wait_connected(&mut h).await;
    assert_eq!(w.room.participants[0].nickname, "alice");
    tokio::time::sleep(Duration::from_millis(800)).await;
    let clock = h.clock.lock().unwrap().clone();
    let offset = clock.offset_ms().expect("pings produced samples");
    // Server clock started just before the client: offset ≈ −(client epoch − server start), small.
    assert!(offset.abs() < 5_000, "{offset}");
    assert!(clock.best_rtt_ms().unwrap() < 200);
    server.shutdown().await;
}

#[tokio::test]
async fn forwards_requests_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let addr = parse_address(&format!("sppy://{}", server.local_addr)).unwrap();
    let mut h = spawn_connection(addr, AuthToken::Host(server.host_token.clone()), "host".into());
    wait_connected(&mut h).await;
    h.tx.send(ClientMsg::Chat { text: "hi".into() }).unwrap();
    loop {
        if let ConnEvent::Message(ServerMsg::Event(Event::Chat(c))) = next_event(&mut h).await {
            assert_eq!(c.text, "hi");
            break;
        }
    }
    server.shutdown().await;
}

#[tokio::test]
async fn reconnects_with_resume() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let proxy = Proxy::start(server.local_addr).await;
    let addr = parse_address(&format!("sppy://{}", proxy.addr)).unwrap();
    let mut h = spawn_connection(addr, AuthToken::Invite(server.invite_token()), "alice".into());
    let first = wait_connected(&mut h).await;
    proxy.cut();
    let second = wait_connected(&mut h).await;
    assert_eq!(second.participant_id, first.participant_id);
    server.shutdown().await;
}

#[tokio::test]
async fn rejoins_after_session_expired() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = test_config();
    cfg.resume_grace_secs = 0;
    let server = start_server(dir.path(), cfg).await;
    let proxy = Proxy::start(server.local_addr).await;
    let addr = parse_address(&format!("sppy://{}", proxy.addr)).unwrap();
    let mut h = spawn_connection(addr, AuthToken::Invite(server.invite_token()), "alice".into());
    let first = wait_connected(&mut h).await;
    proxy.cut();
    // Give the server time to notice the drop and expire the session (grace 0).
    tokio::time::sleep(Duration::from_millis(400)).await;
    let second = wait_connected(&mut h).await;
    assert_ne!(second.participant_id, first.participant_id);
    assert_eq!(second.room.participants.iter().filter(|p| p.nickname == "alice").count(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn reconnects_after_server_restart() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let port_addr = server.local_addr;
    let invite = server.invite_token();
    let addr = parse_address(&format!("sppy://{port_addr}")).unwrap();
    let mut h = spawn_connection(addr, AuthToken::Invite(invite), "alice".into());
    wait_connected(&mut h).await;
    server.shutdown().await;
    let mut cfg = test_config();
    cfg.listen = port_addr;
    let server = start_server(dir.path(), cfg).await;
    let w = wait_connected(&mut h).await;
    assert_eq!(w.room.participants.len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn bad_token_is_final() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let addr = parse_address(&format!("sppy://{}", server.local_addr)).unwrap();
    let mut h = spawn_connection(addr, AuthToken::Invite("wrong".into()), "alice".into());
    loop {
        match next_event(&mut h).await {
            ConnEvent::Rejected { code, .. } => {
                assert_eq!(code, ErrorCode::BadToken);
                break;
            }
            ConnEvent::Connected { .. } => panic!("must not connect"),
            _ => {}
        }
    }
    // The connection task stops: no reconnect attempts.
    let end = tokio::time::timeout(Duration::from_secs(3), h.events.recv()).await.unwrap();
    assert!(end.is_none());
    server.shutdown().await;
}

#[tokio::test]
async fn reports_reconnecting_when_server_down() {
    let addr = parse_address("sppy://127.0.0.1:9").unwrap();
    let mut h = spawn_connection(addr, AuthToken::Invite("x".into()), "alice".into());
    match next_event(&mut h).await {
        ConnEvent::Reconnecting { attempt, .. } => assert_eq!(attempt, 1),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn nickname_held_by_a_dropped_session_is_retried() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = test_config();
    cfg.resume_grace_secs = 2;
    let server = start_server(dir.path(), cfg).await;
    // A previous run of the app: its connection dies without a Bye (crash, kill, sleep).
    let proxy = Proxy::start(server.local_addr).await;
    let old = parse_address(&format!("sppy://{}", proxy.addr)).unwrap();
    let mut first = spawn_connection(old, AuthToken::Invite(server.invite_token()), "alice".into());
    wait_connected(&mut first).await;
    drop(proxy);
    drop(first);
    // The restarted app joins with the same nickname right away.
    let addr = parse_address(&format!("sppy://{}", server.local_addr)).unwrap();
    let mut h = spawn_connection(addr, AuthToken::Invite(server.invite_token()), "alice".into());
    loop {
        match next_event(&mut h).await {
            ConnEvent::Connected { welcome } => {
                assert_eq!(welcome.room.participants.len(), 1);
                break;
            }
            ConnEvent::Rejected { code, .. } => panic!("gave up with {code:?}"),
            _ => {}
        }
    }
    server.shutdown().await;
}
