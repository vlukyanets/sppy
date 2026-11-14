use sppy_proto::*;

fn item() -> Item {
    Item {
        item_id: ItemId(3),
        source: Source::File { file_id: FileId("0123456789abcdef".into()) },
        title: "Movie".into(),
        duration_ms: Some(5_400_000),
        fingerprint: Some(Fingerprint { size: 1_000_000, hash: "ab".repeat(32) }),
        subtitles: vec![Subtitle { index: 0, label: Some("en".into()), ext: "srt".into() }],
        added_by: ParticipantId(1),
    }
}

fn room() -> RoomState {
    RoomState {
        room_id: RoomId::main(),
        participants: vec![Participant {
            id: ParticipantId(1),
            nickname: "alice".into(),
            is_host: true,
            can_control: true,
            connected: true,
            status: MediaStatus::Ready { duration_ms: Some(5_400_000), title: None },
        }],
        control_mode: ControlMode::Selected,
        playlist: vec![item()],
        settings: RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 },
    }
}

fn playback() -> PlaybackState {
    PlaybackState {
        item_id: Some(ItemId(3)),
        position_ms: 120_000,
        at_server_ms: 42_000,
        paused: false,
        speed: 1.0,
        hold: Some(Hold { reason: HoldReason::Stalled, waiting_for: vec![ParticipantId(2)] }),
        seq: 17,
        cause: Some(Cause { participant_id: ParticipantId(1), req_id: 9 }),
    }
}

fn chat() -> ChatMessage {
    ChatMessage {
        from: ParticipantId(2),
        nickname: "bob".into(),
        text: "hi".into(),
        at_server_ms: 1000,
    }
}

fn client_samples() -> Vec<(&'static str, ClientMsg)> {
    vec![
        (
            "Hello",
            ClientMsg::Hello {
                protocol: PROTOCOL_VERSION,
                room: RoomId::main(),
                auth: Auth::Invite { token: "tok".into() },
                nickname: "alice".into(),
                client: "sppy-app/0.1.0".into(),
            },
        ),
        (
            "HelloResume",
            ClientMsg::Hello {
                protocol: PROTOCOL_VERSION,
                room: RoomId::main(),
                auth: Auth::Resume { session_token: "sess".into() },
                nickname: "alice".into(),
                client: "sppy-app/0.1.0".into(),
            },
        ),
        ("Ping", ClientMsg::Ping { client_time_ms: 123 }),
        ("Bye", ClientMsg::Bye),
        ("Play", ClientMsg::Play { req_id: 1, position_ms: Some(1000) }),
        ("Pause", ClientMsg::Pause { req_id: 2, position_ms: None }),
        ("Seek", ClientMsg::Seek { req_id: 3, position_ms: 120_000 }),
        ("SetSpeed", ClientMsg::SetSpeed { req_id: 4, speed: 1.25 }),
        (
            "PlaylistAdd",
            ClientMsg::PlaylistAdd {
                req_id: 5,
                sources: vec![
                    Source::File { file_id: FileId("0123456789abcdef".into()) },
                    Source::Url { url: "https://example.com/v".into() },
                ],
                after: Some(ItemId(3)),
            },
        ),
        ("PlaylistRemove", ClientMsg::PlaylistRemove { req_id: 6, item_id: ItemId(3) }),
        ("PlaylistMove", ClientMsg::PlaylistMove { req_id: 7, item_id: ItemId(3), after: None }),
        ("PlaylistSelect", ClientMsg::PlaylistSelect { req_id: 8, item_id: ItemId(3) }),
        (
            "SetStatus",
            ClientMsg::SetStatus {
                item_id: ItemId(3),
                status: MediaStatus::Failed { message: "boom".into() },
            },
        ),
        (
            "Progress",
            ClientMsg::Progress {
                item_id: ItemId(3),
                delivery: Delivery::Download { bytes: 10, total: 100 },
                drift_ms: None,
            },
        ),
        ("ItemEnded", ClientMsg::ItemEnded { item_id: ItemId(3) }),
        ("Chat", ClientMsg::Chat { text: "hi".into() }),
        ("GetLibrary", ClientMsg::GetLibrary),
        ("SetControlMode", ClientMsg::SetControlMode { req_id: 9, mode: ControlMode::HostOnly }),
        (
            "SetCanControl",
            ClientMsg::SetCanControl {
                req_id: 10,
                participant_id: ParticipantId(2),
                allowed: true,
            },
        ),
        ("Kick", ClientMsg::Kick { req_id: 11, participant_id: ParticipantId(2) }),
        ("RotateInvite", ClientMsg::RotateInvite { req_id: 12 }),
        ("RescanLibrary", ClientMsg::RescanLibrary { req_id: 13 }),
        ("StartWithoutWaiting", ClientMsg::StartWithoutWaiting { req_id: 14 }),
    ]
}

fn server_samples() -> Vec<(&'static str, ServerMsg)> {
    vec![
        (
            "Welcome",
            ServerMsg::Welcome(Welcome {
                participant_id: ParticipantId(1),
                session_token: "sess".into(),
                is_host: true,
                server_time_ms: 42_000,
                room: room(),
                playback: playback(),
                chat_history: vec![chat()],
            }),
        ),
        (
            "Reject",
            ServerMsg::Reject {
                code: ErrorCode::VersionMismatch { server_protocol: 1 },
                message: "update".into(),
            },
        ),
        ("Pong", ServerMsg::Pong { client_time_ms: 123, server_time_ms: 42_000 }),
        ("Playback", ServerMsg::Playback(playback())),
        ("Room", ServerMsg::Room(room())),
        (
            "ProgressBatch",
            ServerMsg::ProgressBatch {
                entries: vec![ProgressEntry {
                    participant_id: ParticipantId(2),
                    item_id: ItemId(3),
                    delivery: Delivery::Stream,
                    drift_ms: Some(-12),
                }],
            },
        ),
        ("EventChat", ServerMsg::Event(Event::Chat(chat()))),
        (
            "EventNotice",
            ServerMsg::Event(Event::Notice {
                actor: Some(ParticipantId(1)),
                action: NoticeAction::Seeked { to_ms: 60_000 },
            }),
        ),
        ("Denied", ServerMsg::Denied { req_id: 4, code: ErrorCode::NotPermitted }),
        (
            "Library",
            ServerMsg::Library {
                roots: vec![LibraryNode::Dir {
                    name: "Movies".into(),
                    children: vec![LibraryNode::File {
                        file_id: FileId("0123456789abcdef".into()),
                        name: "Movie.mkv".into(),
                        size: 1_000_000,
                        fingerprint: None,
                        subtitles: vec![],
                    }],
                }],
            },
        ),
        ("InviteToken", ServerMsg::InviteToken { token: "tok".into() }),
        ("Closing", ServerMsg::Closing { reason: CloseReason::Kicked }),
    ]
}

#[test]
fn golden_client_messages() {
    for (name, msg) in client_samples() {
        insta::assert_json_snapshot!(format!("client_{name}"), msg);
    }
}

#[test]
fn golden_server_messages() {
    for (name, msg) in server_samples() {
        insta::assert_json_snapshot!(format!("server_{name}"), msg);
    }
}

#[test]
fn roundtrip_all() {
    for (name, msg) in client_samples() {
        let text = encode(&msg);
        assert_eq!(decode_client(&text).unwrap(), msg, "{name}");
    }
    for (name, msg) in server_samples() {
        let text = encode(&msg);
        let back: ServerMsg = serde_json::from_str(&text).unwrap();
        assert_eq!(back, msg, "{name}");
    }
}

#[test]
fn seek_wire_format() {
    assert_eq!(
        encode(&ClientMsg::Seek { req_id: 7, position_ms: 120000 }),
        r#"{"type":"Seek","req_id":7,"position_ms":120000}"#
    );
}

#[test]
fn req_id_accessor() {
    assert_eq!(ClientMsg::Seek { req_id: 7, position_ms: 0 }.req_id(), Some(7));
    assert_eq!(ClientMsg::Chat { text: "x".into() }.req_id(), None);
}
