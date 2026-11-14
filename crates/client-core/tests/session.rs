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
    assert!(
        h.remote()
            .commands()
            .contains(&FakeCmd::Load(MediaSource::Url("https://example.invalid/u1".into())))
    );
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
        p.remote()
            .commands()
            .contains(&FakeCmd::Load(MediaSource::Url("https://example.invalid/u2".into())))
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
        a.remote()
            .commands()
            .contains(&FakeCmd::Load(MediaSource::Url("https://example.invalid/u1".into())))
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

#[tokio::test]
async fn reselecting_the_current_item_does_not_wait_forever() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    // Double-click on the item that is already playing: the server restarts it and resets
    // everyone to Loading; the players already have the file loaded.
    let seq = snap(&a.handle).playback.unwrap().seq;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    b.send(UiCommand::PlaylistSelect(ItemId(1)));
    eventually("restarted from the beginning", 3, || {
        snap(&a.handle).playback.is_some_and(|p| p.seq > seq && p.position_ms < 2_000)
    })
    .await;
    eventually("hold released again", 5, || {
        snap(&a.handle).playback.is_some_and(|p| p.hold.is_none())
    })
    .await;
    eventually("both playing in sync", 8, || a.synced() && b.synced()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn pressing_play_while_the_room_waits_explains_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = test_config();
    cfg.stall_hold_secs = 1;
    let server = start_server(dir.path(), cfg).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    b.remote().set_cache_ahead(0, false);
    b.remote().set_buffering(true);
    eventually("room waits for bob, a paused", 5, || a.remote().paused()).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let waiting = |p: &Peer| {
        p.remote()
            .commands()
            .iter()
            .filter(|c| matches!(c, FakeCmd::Osd(t) if t.starts_with("Waiting for")))
            .count()
    };
    let before = waiting(&a);
    a.remote().user_pause(false);
    eventually("explained in the player", 3, || waiting(&a) > before).await;
    eventually("a paused again", 3, || a.remote().paused()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn drift_of_each_participant_is_visible_to_everyone() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    let bob = snap(&b.handle).me.unwrap();
    eventually("a sees bob's drift", 5, || {
        snap(&a.handle).progress.get(&bob).and_then(|e| e.drift_ms).is_some_and(|d| d.abs() < 200)
    })
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn own_actions_are_not_repeated_on_the_osd() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    a.remote().user_pause(true);
    let osd = |p: &Peer, text: &str| {
        p.remote().commands().iter().any(|c| matches!(c, FakeCmd::Osd(t) if t == text))
    };
    eventually("bob is told", 3, || osd(&b, "host paused")).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!osd(&a, "host paused"), "the host saw their own pause");
    assert!(!osd(&b, "host joined") && !osd(&b, "bob joined"), "joins stay out of the OSD");
    server.shutdown().await;
}

#[tokio::test]
async fn restart_player_relaunches_and_resyncs() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("in sync", 8, || a.synced()).await;
    a.send(UiCommand::RestartPlayer);
    eventually("relaunched", 3, || a.launcher.launches() == 2).await;
    eventually("in sync again", 8, || a.synced()).await;
    assert!(snap(&a.handle).player_open);
    server.shutdown().await;
}

#[tokio::test]
async fn missing_player_is_reported_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let launcher = sppy_player::fake::FakeLauncher::new(FakeConfig::default());
    launcher.set_not_installed(true);
    let handle = sppy_client_core::api::spawn_client(
        sppy_client_core::config::ClientConfig { nickname: "host".into(), ..Default::default() },
        launcher,
    );
    let mut events = handle.subscribe_events();
    handle
        .commands
        .send(UiCommand::BecomeHostConnect {
            listen: server.local_addr,
            host_token: server.host_token.clone(),
        })
        .unwrap();
    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(e @ sppy_client_core::api::UiEvent::PlayerUnavailable(_)) =
                events.recv().await
            {
                return e;
            }
        }
    })
    .await
    .expect("PlayerUnavailable event");
    let sppy_client_core::api::UiEvent::PlayerUnavailable(text) = ev else { unreachable!() };
    assert!(text.contains("not found"), "{text}");
    server.shutdown().await;
}
