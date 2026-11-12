mod common;
mod support;

use std::time::Duration;

use common::*;
use sppy_client_core::api::{UiCommand, UiEvent};
use sppy_player::fake::FakeConfig;
use support::*;

fn slow() -> FakeConfig {
    FakeConfig { cmd_latency_ms: 60, seek_delay_ms: 400, ..FakeConfig::default() }
}

#[tokio::test]
async fn two_clients_stay_within_80ms() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", slow()).await;
    add_and_play(&a, &["u1"]).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut worst_pair = 0i64;
    let mut worst_a = 0i64;
    let mut worst_b = 0i64;
    for _ in 0..(20_000 / 250) {
        let (pa, pb) = (a.remote().position_ms() as i64, b.remote().position_ms() as i64);
        worst_pair = worst_pair.max((pa - pb).abs());
        worst_a = worst_a.max(a.drift().abs());
        worst_b = worst_b.max(b.drift().abs());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(worst_pair < 160, "pairwise {worst_pair}");
    assert!(worst_a < 80, "a {worst_a}");
    assert!(worst_b < 80, "b {worst_b}");
    server.shutdown().await;
}

#[tokio::test]
async fn concurrent_seeks_converge_without_request_storm() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", slow()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    let mut notices = a.handle.subscribe_events();
    a.remote().user_seek(100_000);
    tokio::time::sleep(Duration::from_millis(10)).await;
    b.remote().user_seek(200_000);
    tokio::time::sleep(Duration::from_secs(4)).await;
    eventually("both in sync after seeks", 6, || a.synced() && b.synced()).await;
    let pos = a.remote().position_ms();
    assert!((100_000..215_000).contains(&pos), "landed on one of the seeks: {pos}");
    let mut seeks = 0;
    while let Ok(ev) = notices.try_recv() {
        if matches!(&ev, UiEvent::Notice(t) if t.contains("jumped to")) {
            seeks += 1;
        }
    }
    assert!(seeks <= 2, "seek requests: {seeks}");
    server.shutdown().await;
}

#[tokio::test]
async fn short_stall_does_not_hold_room() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    b.remote().set_buffering(true);
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!a.remote().paused(), "a must keep playing during b's short stall");
    }
    b.remote().set_buffering(false);
    eventually("b caught up", 5, || b.synced()).await;
    assert!(!a.remote().paused());
    server.shutdown().await;
}

#[tokio::test]
async fn long_stall_holds_and_resumes_with_reserve() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = test_config();
    cfg.stall_hold_secs = 2;
    let server = start_server(dir.path(), cfg).await;
    let a = host(&server, FakeConfig::default()).await;
    let b = guest(&server, "bob", FakeConfig::default()).await;
    add_and_play(&a, &["u1"]).await;
    eventually("both in sync", 8, || a.synced() && b.synced()).await;
    b.remote().set_cache_ahead(0, false);
    b.remote().set_buffering(true);
    eventually("room held, a paused", 5, || a.remote().paused()).await;
    b.remote().set_cache_ahead(3_000, false);
    b.remote().set_buffering(false);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(a.remote().paused(), "3 s of reserve is not enough to resume");
    b.remote().set_cache_ahead(10_000, false);
    eventually("room resumed", 3, || !a.remote().paused()).await;
    eventually("both in sync", 5, || a.synced() && b.synced()).await;
    let _ = UiCommand::Play;
    server.shutdown().await;
}
