mod common;
mod support;

use std::time::Duration;

use common::*;
use sppy_client_core::api::UiCommand;
use sppy_player::fake::{FakeCmd, FakeConfig};
use sppy_player::{MediaSource, PlayerEvent};
use sppy_proto::*;
use support::*;

#[tokio::test]
async fn single_client_follows_server() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let h = host(&server, FakeConfig::default()).await;
    add_and_play(&h, &["u1"]).await;
    eventually("fake playing", 3, || h.launcher.remote().is_some_and(|r| !r.paused())).await;
    assert!(h.remote().commands().contains(&FakeCmd::Load(MediaSource::Url("fake://u1".into()))));
    eventually("in sync", 5, || h.synced()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn ui_seek_moves_player() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let h = host(&server, FakeConfig::default()).await;
    add_and_play(&h, &["u1"]).await;
    h.send(UiCommand::Seek(120_000));
    eventually("player near 120 s", 5, || (120_000..125_000).contains(&h.remote().position_ms()))
        .await;
    eventually("in sync", 5, || h.synced()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn user_pause_in_player_propagates() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 5, || a.synced() && b.synced()).await;
    a.remote().user_pause(true);
    eventually("b paused", 2, || b.remote().paused()).await;
    let (pa, pb) = (a.remote().position_ms() as i64, b.remote().position_ms() as i64);
    eventually("same frame", 2, || {
        (a.remote().position_ms() as i64 - b.remote().position_ms() as i64).abs() < 100
    })
    .await;
    assert!((pa - pb).abs() < 1000, "{pa} vs {pb}");
    assert!(!is_playing(&a));
    server.shutdown().await;
}

#[tokio::test]
async fn user_seek_in_player_propagates() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 5, || a.synced() && b.synced()).await;
    b.remote().user_seek(300_000);
    eventually("a jumped", 3, || a.remote().position_ms() > 299_000).await;
    eventually("both in sync", 5, || a.synced() && b.synced()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn denied_user_pause_reverted() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    a.send(UiCommand::SetControlMode(ControlMode::HostOnly));
    add_and_play(&a, &["u1"]).await;
    eventually("b in sync", 5, || b.synced()).await;
    b.remote().user_pause(true);
    eventually("b resumed", 3, || !b.remote().paused()).await;
    assert!(
        b.remote().commands().contains(&FakeCmd::Osd("No permission to control playback".into()))
    );
    assert!(is_playing(&a));
    eventually("b in sync again", 5, || b.synced()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn item_end_advances_both() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let fake = FakeConfig { duration_ms: 2_000, ..FakeConfig::default() };
    let a = host(&server, fake.clone()).await;
    let b = guest(&server, "bob", fake).await;
    add_and_play(&a, &["u1", "u2"]).await;
    let loaded_u2 = |p: &Peer| {
        p.remote().commands().contains(&FakeCmd::Load(MediaSource::Url("fake://u2".into())))
    };
    eventually("both on item 2", 8, || loaded_u2(&a) && loaded_u2(&b)).await;
    // Item 2 is short and may already have ended: check that it started (hold released).
    eventually("room started item 2", 5, || {
        snap(&a.handle).playback.is_some_and(|p| p.item_id == Some(ItemId(2)) && p.hold.is_none())
    })
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn player_crash_restarts_once() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    a.remote().exit(Some(1));
    eventually("relaunched", 3, || a.launcher.launches() == 2).await;
    eventually("reloaded", 3, || {
        a.remote().commands().contains(&FakeCmd::Load(MediaSource::Url("fake://u1".into())))
    })
    .await;
    eventually("in sync", 5, || a.synced()).await;
    assert!(snap(&a.handle).player_open);
    server.shutdown().await;
}

fn my_status(p: &Peer) -> Option<MediaStatus> {
    let s = snap(&p.handle);
    let me = s.me?;
    s.room?.participants.into_iter().find(|x| x.id == me).map(|x| x.status)
}

#[tokio::test]
async fn second_crash_marks_closed() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    a.remote().exit(None);
    eventually("relaunched", 3, || a.launcher.launches() == 2).await;
    a.remote().exit(None);
    eventually("closed", 3, || !snap(&a.handle).player_open).await;
    eventually("server knows", 3, || my_status(&a) == Some(MediaStatus::PlayerClosed)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.launcher.launches(), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn clean_exit_marks_closed_and_open_player_relaunches() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    b.remote().exit(Some(0));
    eventually("b closed", 3, || my_status(&b) == Some(MediaStatus::PlayerClosed)).await;
    assert_eq!(b.launcher.launches(), 1);
    assert!(is_playing(&a), "room goes on without b");
    b.send(UiCommand::OpenPlayer);
    eventually("b relaunched", 3, || b.launcher.launches() == 2).await;
    eventually("b in sync", 5, || b.synced()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn load_failure_reported_room_continues() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    b.remote().fail_load("yt-dlp failed");
    eventually("failed status", 3, || matches!(my_status(&b), Some(MediaStatus::Failed { .. })))
        .await;
    assert!(is_playing(&a));
    eventually("a in sync", 5, || a.synced()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn load_failure_everywhere_skips_item() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    add_and_play(&a, &["u1", "u2"]).await;
    a.remote().fail_load("yt-dlp failed");
    eventually("moved to item 2", 3, || {
        snap(&a.handle).playback.is_some_and(|p| p.item_id == Some(ItemId(2)))
    })
    .await;
    let _ = PlayerEvent::Eof;
    server.shutdown().await;
}
