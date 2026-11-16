//! The server ends an item by itself, from the file's own duration: nobody has to watch.

mod common;

use std::time::Duration;

use common::*;
use sppy_proto::*;
use sppy_server::{MediaRoot, ServerConfig};

/// A Matroska file whose header says `ms` milliseconds (no media data).
fn mkv(ms: f64, tag: u8) -> Vec<u8> {
    let el = |id: &[u8], payload: &[u8]| {
        let mut v = id.to_vec();
        v.push(0x80 | payload.len() as u8);
        v.extend_from_slice(payload);
        v
    };
    let info = el(&[0x15, 0x49, 0xA9, 0x66], &el(&[0x44, 0x89], &ms.to_be_bytes()));
    let void = el(&[0xEC], &[tag]);
    [el(&[0x1A, 0x45, 0xDF, 0xA3], b""), el(&[0x18, 0x53, 0x80, 0x67], &[info, void].concat())]
        .concat()
}

#[tokio::test]
async fn the_room_moves_on_when_the_file_ends_with_nobody_watching() {
    let (dir, media) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(media.path().join("1.mkv"), mkv(1_500.0, 1)).unwrap();
    std::fs::write(media.path().join("2.mkv"), mkv(60_000.0, 2)).unwrap();
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "M".into(), path: media.path().to_path_buf() }],
        ..test_config()
    };
    let server = start_server(dir.path(), cfg).await;
    let (mut h, _) =
        TestClient::join(server.local_addr, Auth::Host { token: server.host_token.clone() }, "h")
            .await;
    let id = |name: &str| sppy_server::library::file_id("M", std::path::Path::new(name));
    let sources =
        vec![Source::File { file_id: id("1.mkv") }, Source::File { file_id: id("2.mkv") }];
    // Durations arrive with the background fingerprint pass: add once they are known.
    let items = loop {
        h.send(ClientMsg::PlaylistAdd { req_id: 1, sources: sources.clone(), after: None }).await;
        let ServerMsg::Room(room) = h.recv_until(|m| matches!(m, ServerMsg::Room(_))).await else {
            unreachable!()
        };
        let durations: Vec<_> = room.playlist.iter().map(|i| i.duration_ms).collect();
        if durations == [Some(1_500), Some(60_000)] {
            break room.playlist.iter().map(|i| i.item_id).collect::<Vec<_>>();
        }
        for item in room.playlist {
            h.send(ClientMsg::PlaylistRemove { req_id: 2, item_id: item.item_id }).await;
            h.recv_until(|m| matches!(m, ServerMsg::Room(_))).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    h.send(ClientMsg::PlaylistSelect { req_id: 3, item_id: items[0] }).await;
    // A lying duration from a player changes nothing.
    let ready = MediaStatus::Ready { duration_ms: Some(1), title: None };
    h.send(ClientMsg::SetStatus { item_id: items[0], status: ready }).await;
    h.recv_until(|m| {
        matches!(m, ServerMsg::Playback(p)
            if p.item_id == Some(items[0]) && p.hold.is_none() && !p.paused)
    })
    .await;
    // The host closes the player and does nothing else.
    h.send(ClientMsg::SetStatus { item_id: items[0], status: MediaStatus::PlayerClosed }).await;
    let m =
        h.recv_until(|m| matches!(m, ServerMsg::Playback(p) if p.item_id == Some(items[1]))).await;
    let ServerMsg::Playback(p) = m else { unreachable!() };
    assert_eq!(p.item_id, Some(items[1]));
    server.shutdown().await;
}
