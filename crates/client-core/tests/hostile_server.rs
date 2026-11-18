//! A server that speaks our protocol version but sends nonsense or hostile data must not
//! crash the client, stop it working, or make it write outside its cache.

mod support;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use sppy_client_core::api::{ConnState, UiCommand, spawn_client};
use sppy_client_core::config::{ClientConfig, DeliveryMode};
use sppy_player::fake::{FakeConfig, FakeLauncher};
use sppy_proto::*;
use support::*;

/// Messages sent one by one after the welcome.
type Script = Arc<Vec<Message>>;

#[derive(Clone)]
struct Fake {
    script: Script,
    welcome: Arc<ServerMsg>,
    connections: Arc<AtomicUsize>,
    pings: Arc<AtomicUsize>,
}

async fn session(mut ws: WebSocket, fake: Fake) {
    fake.connections.fetch_add(1, Ordering::SeqCst);
    let Some(Ok(Message::Text(_hello))) = ws.recv().await else { return };
    if ws.send(Message::text(encode(&*fake.welcome))).await.is_err() {
        return;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    for m in fake.script.iter() {
        if ws.send(m.clone()).await.is_err() {
            return;
        }
    }
    while let Some(Ok(m)) = ws.recv().await {
        if let Message::Text(t) = m
            && let Ok(ClientMsg::Ping { client_time_ms }) = decode_client(t.as_str())
        {
            fake.pings.fetch_add(1, Ordering::SeqCst);
            let pong = ServerMsg::Pong { client_time_ms, server_time_ms: 1_000_000 };
            if ws.send(Message::text(encode(&pong))).await.is_err() {
                return;
            }
        }
    }
}

/// Serves `/ws` with the script, and the same bytes for every other path (a download of
/// `/media/../../escaped` lands here).
async fn start_fake(welcome: ServerMsg, script: Vec<Message>) -> (std::net::SocketAddr, Fake) {
    let fake = Fake {
        script: Arc::new(script),
        welcome: Arc::new(welcome),
        connections: Default::default(),
        pings: Default::default(),
    };
    let f = fake.clone();
    let app = axum::Router::new()
        .route(
            "/ws",
            get(move |up: WebSocketUpgrade| {
                let f = f.clone();
                async move { up.on_upgrade(move |ws| session(ws, f)) }
            }),
        )
        .fallback(|| async { "attacker-controlled bytes" });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, fake)
}

fn participant(id: u64) -> Participant {
    Participant {
        id: ParticipantId(id),
        nickname: format!("p{id}"),
        is_host: false,
        can_control: true,
        connected: true,
        status: MediaStatus::Loading,
    }
}

fn item(id: u64, file_id: &str, sub_ext: &str) -> Item {
    Item {
        item_id: ItemId(id),
        source: Source::File { file_id: FileId(file_id.into()) },
        title: "t".into(),
        duration_ms: Some(u64::MAX),
        fingerprint: Some(Fingerprint { size: 25, hash: "00".repeat(32) }),
        subtitles: vec![Subtitle { index: 1, label: None, ext: sub_ext.into() }],
        added_by: ParticipantId(99),
    }
}

fn room(playlist: Vec<Item>) -> RoomState {
    RoomState {
        room_id: RoomId::main(),
        participants: vec![participant(1), participant(1), participant(u64::MAX)],
        control_mode: ControlMode::Everyone,
        playlist,
        settings: RoomSettings { stall_hold_secs: u32::MAX, resume_buffer_secs: u32::MAX },
    }
}

fn playback(item: Option<u64>, position_ms: u64, at_server_ms: u64, speed: f64) -> PlaybackState {
    PlaybackState {
        item_id: item.map(ItemId),
        position_ms,
        at_server_ms,
        paused: false,
        speed,
        hold: None,
        seq: u64::MAX,
        cause: None,
    }
}

fn text(msg: &ServerMsg) -> Message {
    Message::text(encode(msg))
}

fn deep_library(depth: usize) -> Vec<LibraryNode> {
    let mut node = LibraryNode::File {
        file_id: FileId("../x".into()),
        name: String::new(),
        size: u64::MAX,
        fingerprint: None,
        subtitles: vec![],
    };
    for _ in 0..depth {
        node = LibraryNode::Dir { name: "/".into(), children: vec![node] };
    }
    vec![node]
}

fn hostile_script() -> Vec<Message> {
    let mut big_playlist: Vec<Item> =
        (0..2000).map(|i| item(i % 3, "0123456789abcdef", "srt")).collect();
    big_playlist.push(item(7, "/etc/passwd", "../../evil"));
    vec![
        Message::text(r#"{"type":"FromTheFuture","x":1}"#),
        Message::text("not json at all"),
        Message::text(r#"{"type":"Playback"}"#),
        Message::Binary(vec![0u8; 1024].into()),
        text(&ServerMsg::Playback(playback(Some(2), u64::MAX, u64::MAX, 1e300))),
        text(&ServerMsg::Playback(playback(Some(2), 0, 0, -3.0))),
        text(&ServerMsg::Playback(playback(Some(2), 5, u64::MAX, 0.0))),
        text(&ServerMsg::Playback(playback(Some(424242), 1, 1, 1.0))),
        text(&ServerMsg::ProgressBatch {
            entries: vec![
                ProgressEntry {
                    participant_id: ParticipantId(1),
                    item_id: ItemId(2),
                    delivery: Delivery::Download { bytes: u64::MAX, total: 0 },
                    drift_ms: Some(i64::MIN),
                },
                ProgressEntry {
                    participant_id: ParticipantId(u64::MAX),
                    item_id: ItemId(424242),
                    delivery: Delivery::Stream,
                    drift_ms: Some(i64::MAX),
                },
            ],
        }),
        text(&ServerMsg::Event(Event::Notice {
            actor: Some(ParticipantId(31337)),
            action: NoticeAction::Seeked { to_ms: u64::MAX },
        })),
        text(&ServerMsg::Event(Event::Chat(ChatMessage {
            from: ParticipantId(u64::MAX),
            nickname: "${filename} %s {}".into(),
            text: "x".repeat(200_000),
            at_server_ms: u64::MAX,
        }))),
        text(&ServerMsg::MediaFolders {
            folders: vec![MediaFolder { name: "../".into(), path: "/".into(), available: true }],
            editable: true,
            adding: FolderAdding::Within { root: "x".repeat(100_000) },
        }),
        text(&ServerMsg::InviteToken { token: String::new() }),
        text(&ServerMsg::Denied {
            req_id: u32::MAX,
            code: ErrorCode::MediaFolder { problem: MediaFolderProblem::CannotSave },
        }),
        text(&ServerMsg::Library { roots: deep_library(100) }),
        text(&ServerMsg::Room(room(big_playlist))),
        text(&ServerMsg::Pong { client_time_ms: u64::MAX, server_time_ms: 0 }),
        text(&ServerMsg::Pong { client_time_ms: 0, server_time_ms: u64::MAX }),
        text(&ServerMsg::Room(room(vec![]))),
        text(&ServerMsg::Playback(playback(None, 0, 0, 1.0))),
    ]
}

fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[tokio::test]
async fn hostile_server_cannot_break_the_client() {
    let panics = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = panics.clone();
    let print = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        seen.lock().unwrap().push(info.to_string());
        print(info);
    }));

    let root = tempfile::tempdir().unwrap();
    // The download cache is <root>/a/b/media: "../../../escaped" would be <root>/escaped.
    let cache_dir = root.path().join("a/b");
    let welcome = ServerMsg::Welcome(Welcome {
        participant_id: ParticipantId(1),
        session_token: "../token?&#".into(),
        is_host: true,
        server_time_ms: u64::MAX,
        room: room(vec![
            item(1, "../../../escaped", "srt"),
            item(2, "0123456789abcdef", "../../../../evil"),
        ]),
        playback: playback(Some(1), 0, u64::MAX, 1.0),
        chat_history: vec![],
    });
    let (addr, fake) = start_fake(welcome, hostile_script()).await;

    let config = ClientConfig {
        nickname: "me".into(),
        mode: DeliveryMode::Preload,
        cache_dir: Some(cache_dir.clone()),
        download_backoff_ms: vec![10; 3],
        ..ClientConfig::default()
    };
    let launcher = FakeLauncher::new(FakeConfig::default());
    let handle = spawn_client(config, launcher.clone());
    handle
        .commands
        .send(UiCommand::Connect {
            address: format!("sppy://{addr}/invite"),
            nickname: "me".into(),
        })
        .unwrap();
    eventually("connected", 5, || snap(&handle).connection == ConnState::Connected).await;
    // The script has been sent; let the client chew on it and keep pinging.
    let pings_before = fake.pings.load(Ordering::SeqCst);
    eventually("still pinging after the script", 10, || {
        fake.pings.load(Ordering::SeqCst) > pings_before + 2
    })
    .await;

    // Still alive and answering commands.
    handle.commands.send(UiCommand::SetMode(DeliveryMode::Stream)).unwrap();
    eventually("commands still work", 5, || snap(&handle).local.mode == DeliveryMode::Stream).await;
    assert_eq!(snap(&handle).connection, ConnState::Connected);
    assert_eq!(fake.connections.load(Ordering::SeqCst), 1, "no reconnects");

    // Nothing was written outside the cache.
    let outside: Vec<_> =
        files_under(root.path()).into_iter().filter(|p| !p.starts_with(&cache_dir)).collect();
    assert!(outside.is_empty(), "files outside the cache: {outside:?}");
    assert!(!root.path().join("escaped").exists());

    let _ = std::panic::take_hook();
    let panics = panics.lock().unwrap();
    assert!(panics.is_empty(), "panics: {panics:#?}");
}
