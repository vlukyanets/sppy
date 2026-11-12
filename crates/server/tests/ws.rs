mod common;

use std::time::Duration;

use common::*;
use sppy_proto::*;

#[tokio::test]
async fn host_handshake() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut c, w) = TestClient::join(
        server.local_addr,
        Auth::Host { token: server.host_token.clone() },
        "host",
    )
    .await;
    assert!(w.is_host);
    assert_eq!(w.room.participants.len(), 1);
    c.recv_until(|m| matches!(m, ServerMsg::Room(_))).await;
    let ServerMsg::InviteToken { token } =
        c.recv_until(|m| matches!(m, ServerMsg::InviteToken { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(token, server.invite_token());
    server.shutdown().await;
}

#[tokio::test]
async fn invite_handshake_and_bad_token() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (_c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "guest")
            .await;
    assert!(!w.is_host);
    let (mut bad, first) =
        TestClient::hello(server.local_addr, Auth::Invite { token: "nope".into() }, "evil").await;
    assert!(matches!(first, ServerMsg::Reject { code: ErrorCode::BadToken, .. }));
    assert!(bad.try_recv().await.is_none(), "socket closes after Reject");
    // A host token used as an invite is fine; an invite used as a host token is not.
    let (_h, first) =
        TestClient::hello(server.local_addr, Auth::Host { token: server.invite_token() }, "x")
            .await;
    assert!(matches!(first, ServerMsg::Reject { code: ErrorCode::BadToken, .. }));
    server.shutdown().await;
}

#[tokio::test]
async fn version_mismatch_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let mut c = TestClient::connect_raw(server.local_addr).await;
    c.send(ClientMsg::Hello {
        protocol: 99,
        room: RoomId::main(),
        auth: Auth::Invite { token: server.invite_token() },
        nickname: "old".into(),
        client: "test/0".into(),
    })
    .await;
    assert!(matches!(
        c.recv().await,
        ServerMsg::Reject { code: ErrorCode::VersionMismatch { server_protocol: 1 }, .. }
    ));
    server.shutdown().await;
}

#[tokio::test]
async fn ping_pong() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    c.send(ClientMsg::Ping { client_time_ms: 777 }).await;
    let ServerMsg::Pong { client_time_ms, server_time_ms } =
        c.recv_until(|m| matches!(m, ServerMsg::Pong { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(client_time_ms, 777);
    assert!(server_time_ms >= w.server_time_ms);
    server.shutdown().await;
}

#[tokio::test]
async fn pause_broadcast_to_all() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut h, _) = TestClient::join(
        server.local_addr,
        Auth::Host { token: server.host_token.clone() },
        "host",
    )
    .await;
    let (mut g, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "guest")
            .await;
    let req = h.req();
    h.send(ClientMsg::PlaylistAdd {
        req_id: req,
        sources: vec![Source::Url { url: "u".into() }],
        after: None,
    })
    .await;
    let req = h.req();
    h.send(ClientMsg::Play { req_id: req, position_ms: None }).await;
    let m = g.recv_until(|m| matches!(m, ServerMsg::Playback(p) if !p.paused)).await;
    let ServerMsg::Playback(p) = m else { unreachable!() };
    assert_eq!(p.item_id, Some(ItemId(1)));
    server.shutdown().await;
}

#[tokio::test]
async fn guest_denied_in_host_only_mode() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut h, _) = TestClient::join(
        server.local_addr,
        Auth::Host { token: server.host_token.clone() },
        "host",
    )
    .await;
    let (mut g, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "guest")
            .await;
    h.send(ClientMsg::SetControlMode { req_id: 1, mode: ControlMode::HostOnly }).await;
    g.recv_until(|m| matches!(m, ServerMsg::Room(r) if r.control_mode == ControlMode::HostOnly))
        .await;
    g.send(ClientMsg::PlaylistAdd {
        req_id: 5,
        sources: vec![Source::Url { url: "u".into() }],
        after: None,
    })
    .await;
    let m = g.recv_until(|m| matches!(m, ServerMsg::Denied { .. })).await;
    assert_eq!(m, ServerMsg::Denied { req_id: 5, code: ErrorCode::NotPermitted });
    assert!(matches!(g.recv().await, ServerMsg::Playback(_)));
    assert!(matches!(g.recv().await, ServerMsg::Room(_)));
    server.shutdown().await;
}

#[tokio::test]
async fn resume_after_drop() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (c, w) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    drop(c);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (_c2, w2) = TestClient::join(
        server.local_addr,
        Auth::Resume { session_token: w.session_token.clone() },
        "g",
    )
    .await;
    assert_eq!(w2.participant_id, w.participant_id);
    let (_c3, first) =
        TestClient::hello(server.local_addr, Auth::Resume { session_token: "unknown".into() }, "g")
            .await;
    assert!(matches!(first, ServerMsg::Reject { code: ErrorCode::SessionExpired, .. }));
    server.shutdown().await;
}

#[tokio::test]
async fn garbage_frame_closes() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut c, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    c.send_raw("{not json").await;
    let rest = c.recv_until_closed().await;
    assert!(rest.contains(&ServerMsg::Closing { reason: CloseReason::ProtocolError }));
    server.shutdown().await;
}

#[tokio::test]
async fn first_frame_must_be_hello() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let mut c = TestClient::connect_raw(server.local_addr).await;
    c.send(ClientMsg::Ping { client_time_ms: 1 }).await;
    let rest = c.recv_until_closed().await;
    assert_eq!(rest, vec![ServerMsg::Closing { reason: CloseReason::ProtocolError }]);
    server.shutdown().await;
}

#[tokio::test]
async fn ping_timeout_marks_disconnected() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = test_config();
    cfg.ping_timeout_ms = 300;
    let server = start_server(dir.path(), cfg).await;
    let (_silent, sw) = TestClient::join(
        server.local_addr,
        Auth::Invite { token: server.invite_token() },
        "silent",
    )
    .await;
    let (mut obs, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "obs")
            .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "silent client never marked disconnected");
        obs.send(ClientMsg::Ping { client_time_ms: 0 }).await;
        if let ServerMsg::Room(r) = obs.recv().await
            && r.participants.iter().any(|p| p.id == sw.participant_id && !p.connected)
        {
            assert!(r.participants.iter().any(|p| p.nickname == "obs" && p.connected));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.shutdown().await;
}

#[tokio::test]
async fn kick_closes_connection() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut h, _) = TestClient::join(
        server.local_addr,
        Auth::Host { token: server.host_token.clone() },
        "host",
    )
    .await;
    let (mut g, gw) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "guest")
            .await;
    h.send(ClientMsg::Kick { req_id: 1, participant_id: gw.participant_id }).await;
    let rest = g.recv_until_closed().await;
    assert!(rest.contains(&ServerMsg::Closing { reason: CloseReason::Kicked }));
    server.shutdown().await;
}

#[tokio::test]
async fn rotate_invite_by_host() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let old = server.invite_token();
    let (mut h, _) = TestClient::join(
        server.local_addr,
        Auth::Host { token: server.host_token.clone() },
        "host",
    )
    .await;
    let (mut g, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: old.clone() }, "guest").await;
    g.send(ClientMsg::RotateInvite { req_id: 3 }).await;
    let m = g.recv_until(|m| matches!(m, ServerMsg::Denied { .. })).await;
    assert_eq!(m, ServerMsg::Denied { req_id: 3, code: ErrorCode::NotHost });
    h.recv_until(|m| matches!(m, ServerMsg::InviteToken { .. })).await; // initial
    h.send(ClientMsg::RotateInvite { req_id: 4 }).await;
    let ServerMsg::InviteToken { token } =
        h.recv_until(|m| matches!(m, ServerMsg::InviteToken { .. })).await
    else {
        unreachable!()
    };
    assert_ne!(token, old);
    assert_eq!(server.invite_token(), token);
    let (_x, first) =
        TestClient::hello(server.local_addr, Auth::Invite { token: old }, "late").await;
    assert!(matches!(first, ServerMsg::Reject { code: ErrorCode::BadToken, .. }));
    server.shutdown().await;
}

#[tokio::test]
async fn shutdown_notifies_clients() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut c, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: server.invite_token() }, "g")
            .await;
    server.shutdown().await;
    let rest = c.recv_until_closed().await;
    assert!(rest.contains(&ServerMsg::Closing { reason: CloseReason::ServerShutdown }));
}

#[tokio::test]
async fn start_fails_cleanly_on_busy_port() {
    let dir = tempfile::tempdir().unwrap();
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = test_config();
    cfg.listen = busy.local_addr().unwrap();
    let err = sppy_server::start(cfg, dir.path().to_path_buf()).await.err().expect("must fail");
    assert!(matches!(err, sppy_server::ServerError::Bind { .. }));
}
