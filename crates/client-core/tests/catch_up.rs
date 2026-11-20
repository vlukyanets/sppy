//! How a participant's player catches up with the room: speed, jumps, or not at all.

mod common;
mod support;

use std::time::Duration;

use common::*;
use sppy_client_core::config::ClientConfig;
use sppy_player::fake::{FakeCmd, FakeConfig};
use sppy_sync::CatchUp;
use support::*;

fn catching_up(nick: &str, catch_up: CatchUp) -> ClientConfig {
    ClientConfig { catch_up, ..config(nick) }
}

fn speed_changes(p: &Peer) -> Vec<f64> {
    p.remote()
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            FakeCmd::SetSpeed(s) if (s - 1.0).abs() > 1e-9 => Some(s),
            _ => None,
        })
        .collect()
}

fn seeks(p: &Peer) -> usize {
    p.remote().commands().iter().filter(|c| matches!(c, FakeCmd::Seek(_))).count()
}

/// b falls about a second behind: its player stalls for a moment while the room plays on.
/// First the start settles: right after its own seek, the controller would take the stall
/// for a slow seek and learn a seek lead from it.
async fn fall_behind(b: &Peer) {
    let settle = sppy_sync::SyncParams::default().settle_ms;
    tokio::time::sleep(Duration::from_millis(settle + 500)).await;
    b.remote().set_buffering(true);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    b.remote().set_buffering(false);
}

#[tokio::test]
async fn off_leaves_the_player_alone_and_keeps_it_out_of_the_room() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b =
        guest_with(&server, "bob", FakeConfig::default(), catching_up("bob", CatchUp::Off)).await;
    add_and_play(&a, &["u1"]).await;
    eventually("b loaded the item", 5, || {
        b.launcher
            .remote()
            .is_some_and(|r| r.commands().iter().any(|c| matches!(c, FakeCmd::Load(_))))
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let driven: Vec<_> = b
        .remote()
        .commands()
        .into_iter()
        .filter(|c| {
            matches!(c, FakeCmd::Play | FakeCmd::Pause | FakeCmd::Seek(_) | FakeCmd::SetSpeed(_))
        })
        .collect();
    assert!(driven.is_empty(), "b's player was driven: {driven:?}");
    assert!(b.remote().paused(), "b's player opens paused and stays so");
    // What b does in its own player stays there.
    b.remote().user_pause(false);
    b.remote().user_seek(200_000);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(is_playing(&a) && a.remote().position_ms() < 100_000, "the room followed b");
    // b reports no drift, so nobody measures the room against it.
    let me = snap(&b.handle).me.unwrap();
    assert!(snap(&a.handle).progress.get(&me).is_none_or(|e| e.drift_ms.is_none()));
    server.shutdown().await;
}

#[tokio::test]
async fn jumps_catch_up_with_a_seek_instead_of_speed() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b =
        guest_with(&server, "bob", FakeConfig::default(), catching_up("bob", CatchUp::Jumps)).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    let before = seeks(&b);
    fall_behind(&b).await;
    eventually("b jumped", 5, || seeks(&b) > before).await;
    eventually("b in sync again", 5, || b.synced()).await;
    assert!(speed_changes(&b).is_empty(), "speeds: {:?}", speed_changes(&b));
    server.shutdown().await;
}

#[tokio::test]
async fn speed_mode_catches_up_with_speed() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    let before = seeks(&b);
    fall_behind(&b).await;
    eventually("b sped up", 5, || !speed_changes(&b).is_empty()).await;
    assert_eq!(seeks(&b), before, "a second behind is closed by speed, not a seek");
    server.shutdown().await;
}

#[tokio::test]
async fn a_player_that_cannot_change_speed_jumps() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b =
        guest(&server, "bob", FakeConfig { speed_control: false, ..FakeConfig::default() }).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    let before = seeks(&b);
    fall_behind(&b).await;
    eventually("b jumped", 5, || seeks(&b) > before).await;
    assert!(speed_changes(&b).is_empty(), "speeds: {:?}", speed_changes(&b));
    server.shutdown().await;
}
