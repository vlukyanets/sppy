//! Clients of other versions, or ones that claim our version and then misbehave, must not
//! crash the server or spoil the room for everyone else.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use sppy_proto::*;

fn hello_json(protocol: u32, token: &str, extra: &str) -> String {
    format!(
        r#"{{"type":"Hello","protocol":{protocol},"room":"main","auth":{{"kind":"Invite","token":"{token}"}},"nickname":"n{protocol}","client":"other/9"{extra}}}"#
    )
}

#[tokio::test]
async fn other_versions_and_impostors_do_not_affect_the_room() {
    let panics = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = panics.clone();
    let print = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        seen.lock().unwrap().push(info.to_string());
        print(info);
    }));

    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let (mut host, _) =
        TestClient::join(server.local_addr, Auth::Host { token: server.host_token.clone() }, "h")
            .await;
    let invite = server.invite_token();
    host.send(ClientMsg::PlaylistAdd {
        req_id: 1,
        sources: vec![Source::Url { url: "https://example.org/movie.mkv".into() }],
        after: None,
    })
    .await;
    host.recv_until(|m| matches!(m, ServerMsg::Room(r) if r.playlist.len() == 1)).await;

    // Any other version is refused with ours in the answer.
    for protocol in [0, 1, 2, PROTOCOL_VERSION + 1, u32::MAX] {
        let mut c = TestClient::connect_raw(server.local_addr).await;
        c.send_raw(&hello_json(protocol, &invite, "")).await;
        let rest = c.recv_until_closed().await;
        assert!(
            matches!(
                rest.first(),
                Some(ServerMsg::Reject {
                    code: ErrorCode::VersionMismatch { server_protocol: PROTOCOL_VERSION },
                    ..
                })
            ),
            "v{protocol}: {rest:?}"
        );
    }

    // A newer client that claims our version: unknown fields are ignored, it joins...
    let mut imp = TestClient::connect_raw(server.local_addr).await;
    imp.send_raw(&hello_json(PROTOCOL_VERSION, &invite, r#","capabilities":["v9"]"#)).await;
    assert!(matches!(imp.recv().await, ServerMsg::Welcome(_)));
    // ...and whatever it sends cannot break the room.
    let nasty = vec![
        ClientMsg::Seek { req_id: 1, position_ms: u64::MAX },
        ClientMsg::Play { req_id: 2, position_ms: Some(u64::MAX) },
        ClientMsg::Pause { req_id: 3, position_ms: Some(u64::MAX) },
        ClientMsg::SetSpeed { req_id: 4, speed: 1e300 },
        ClientMsg::SetSpeed { req_id: 5, speed: -1.0 },
        ClientMsg::SetSpeed { req_id: 6, speed: 0.0 },
        ClientMsg::PlaylistAdd {
            req_id: 7,
            sources: vec![
                Source::File { file_id: FileId("../../etc/passwd".into()) },
                Source::Url { url: "javascript:alert(1)".into() },
            ],
            after: Some(ItemId(u64::MAX)),
        },
        ClientMsg::PlaylistMove { req_id: 8, item_id: ItemId(u64::MAX), after: Some(ItemId(0)) },
        ClientMsg::PlaylistSelect { req_id: 9, item_id: ItemId(u64::MAX) },
        ClientMsg::PlaylistRemove { req_id: 10, item_id: ItemId(u64::MAX) },
        ClientMsg::SetStatus {
            item_id: ItemId(u64::MAX),
            status: MediaStatus::Ready { duration_ms: Some(u64::MAX), title: None },
        },
        ClientMsg::Progress {
            item_id: ItemId(1),
            delivery: Delivery::Download { bytes: u64::MAX, total: 0 },
            drift_ms: Some(i64::MIN),
        },
        ClientMsg::ItemEnded { item_id: ItemId(u64::MAX) },
        ClientMsg::Chat { text: "x".repeat(100_000) },
        ClientMsg::SetControlMode { req_id: 11, mode: ControlMode::HostOnly },
        ClientMsg::Kick { req_id: 12, participant_id: ParticipantId(1) },
        ClientMsg::SetCanControl { req_id: 13, participant_id: ParticipantId(1), allowed: false },
        ClientMsg::RotateInvite { req_id: 14 },
        ClientMsg::RescanLibrary { req_id: 15 },
        ClientMsg::StartWithoutWaiting { req_id: 16 },
        ClientMsg::AddMediaFolder { req_id: 17, path: "/".into(), name: None },
        ClientMsg::RemoveMediaFolder { req_id: 18, name: "x".into() },
        ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            room: RoomId::main(),
            auth: Auth::Host { token: "guess".into() },
            nickname: "again".into(),
            client: "x".into(),
        },
    ];
    for m in nasty {
        imp.send(m).await;
    }
    let rest = imp.recv_until_closed().await;
    assert!(rest.contains(&ServerMsg::Closing { reason: CloseReason::ProtocolError }));

    // A message from the future closes that connection only.
    let (mut fut, _) =
        TestClient::join(server.local_addr, Auth::Invite { token: invite.clone() }, "f").await;
    fut.send_raw(r#"{"type":"FromTheFuture","x":1}"#).await;
    let rest = fut.recv_until_closed().await;
    assert!(rest.contains(&ServerMsg::Closing { reason: CloseReason::ProtocolError }));

    // The host is still in charge and the room still works.
    host.send(ClientMsg::Ping { client_time_ms: 7 }).await;
    host.recv_until(|m| matches!(m, ServerMsg::Pong { client_time_ms: 7, .. })).await;
    host.send(ClientMsg::SetSpeed { req_id: 100, speed: 1.5 }).await;
    host.recv_until(|m| matches!(m, ServerMsg::Playback(p) if p.speed == 1.5)).await;
    let (_late, welcome) =
        TestClient::join(server.local_addr, Auth::Invite { token: invite }, "late").await;
    assert_eq!(welcome.room.control_mode, ControlMode::Everyone, "the guest changed nothing");
    assert!(welcome.room.participants.iter().any(|p| p.is_host && p.connected));
    assert!(welcome.room.participants.iter().all(|p| !p.can_control), "no rights changed");
    assert!(
        welcome.room.playlist.iter().all(|i| match &i.source {
            Source::File { file_id } => file_id.is_well_formed(),
            Source::Url { url } => url.starts_with("https://") || url.starts_with("http://"),
        }),
        "nothing bogus was added: {:?}",
        welcome.room.playlist
    );
    server.shutdown().await;

    let _ = std::panic::take_hook();
    let panics = panics.lock().unwrap();
    assert!(panics.is_empty(), "panics: {panics:#?}");
}
