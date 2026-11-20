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
async fn fall_behind(b: &Peer) {
    fall_behind_by(b, 1000).await;
}

async fn fall_behind_by(b: &Peer, ms: u64) {
    b.remote().set_buffering(true);
    tokio::time::sleep(Duration::from_millis(ms)).await;
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

/// The stall comes right after b's own start seek, before the controller looked at the
/// seek's result: it must not take the stall for a slow seek.
#[tokio::test]
async fn a_stall_right_after_our_seek_is_closed_by_speed() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("b seeked to the room", 8, || seeks(&b) > 0).await;
    let before = seeks(&b);
    b.remote().set_buffering(true);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    b.remote().set_buffering(false);
    eventually("b sped up", 5, || !speed_changes(&b).is_empty()).await;
    // The old code seeked twice within a second of the settle time ending.
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert_eq!(seeks(&b), before, "no seek with a lead learned from the stall");
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

#[tokio::test]
async fn the_host_makes_the_room_wait_for_latecomers() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    let policy = |p: &Peer| snap(&p.handle).room.map(|r| r.lag_policy);
    assert_eq!(policy(&b), Some(sppy_proto::LagPolicy::CatchUpAlone));
    a.send(sppy_client_core::api::UiCommand::SetLagPolicy(sppy_proto::LagPolicy::FollowSlowest));
    eventually("b sees the new policy", 5, || {
        policy(&b) == Some(sppy_proto::LagPolicy::FollowSlowest)
    })
    .await;
    // Only the host may change it.
    b.send(sppy_client_core::api::UiCommand::SetLagPolicy(sppy_proto::LagPolicy::CatchUpAlone));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(policy(&a), Some(sppy_proto::LagPolicy::FollowSlowest));
    server.shutdown().await;
}

#[tokio::test]
async fn a_waiting_room_goes_back_to_the_latecomer() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    a.send(sppy_client_core::api::UiCommand::SetLagPolicy(sppy_proto::LagPolicy::FollowSlowest));
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    let (a_before, b_before) = (seeks(&a), seeks(&b));
    fall_behind_by(&b, 2_000).await;
    // a, now 2 s ahead of the room, jumps back; b waits for the room instead of jumping.
    eventually("a went back", 5, || seeks(&a) > a_before).await;
    eventually("both in sync again", 5, || a.synced() && b.synced()).await;
    assert_eq!(seeks(&b), b_before, "b caught up by itself as well");
    server.shutdown().await;
}
