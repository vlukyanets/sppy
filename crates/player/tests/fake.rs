use std::time::Duration;

use sppy_player::fake::{FakeCmd, FakeConfig, FakePlayer};
use sppy_player::{MediaSource, Player, PlayerEvent};

async fn load(p: &mut FakePlayer) {
    p.load(MediaSource::Url("fake://x".into()), vec![]).await.unwrap();
}

/// Drain events until `pred` matches (fails after 10 virtual seconds).
async fn wait_for(p: &mut FakePlayer, pred: impl Fn(&PlayerEvent) -> bool) -> PlayerEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ev = p.next_event().await.expect("player alive");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("event did not arrive")
}

#[tokio::test(start_paused = true)]
async fn fake_load_reports_file_loaded() {
    let (mut p, remote) =
        FakePlayer::new(FakeConfig { duration_ms: 90_000, ..FakeConfig::default() });
    load(&mut p).await;
    let ev = wait_for(&mut p, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await;
    assert!(matches!(ev, PlayerEvent::FileLoaded { duration_ms: Some(90_000), .. }));
    assert_eq!(remote.commands()[0], FakeCmd::Load(MediaSource::Url("fake://x".into())));
}

#[tokio::test(start_paused = true)]
async fn fake_position_advances_with_time() {
    let (mut p, remote) = FakePlayer::new(FakeConfig::default());
    load(&mut p).await;
    p.play().await.unwrap();
    let start = remote.position_ms();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let pos = remote.position_ms() - start;
    assert!((1990..=2010).contains(&pos), "{pos}");
    p.set_speed(2.0).await.unwrap();
    let before = remote.position_ms();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let delta = remote.position_ms() - before;
    assert!((1990..=2010).contains(&delta), "{delta}");
}

#[tokio::test(start_paused = true)]
async fn fake_reports_time_pos() {
    let (mut p, _remote) = FakePlayer::new(FakeConfig::default());
    load(&mut p).await;
    p.play().await.unwrap();
    let ev = wait_for(&mut p, |e| matches!(e, PlayerEvent::TimePos(ms) if *ms >= 500)).await;
    assert!(matches!(ev, PlayerEvent::TimePos(_)));
}

#[tokio::test(start_paused = true)]
async fn fake_seek_emits_seeked_after_delay() {
    let (mut p, remote) =
        FakePlayer::new(FakeConfig { seek_delay_ms: 300, ..FakeConfig::default() });
    load(&mut p).await;
    let t0 = tokio::time::Instant::now();
    p.seek(30_000).await.unwrap();
    let ev = wait_for(&mut p, |e| matches!(e, PlayerEvent::Seeked { .. })).await;
    assert_eq!(ev, PlayerEvent::Seeked { position_ms: 30_000 });
    assert!(t0.elapsed() >= Duration::from_millis(300));
    assert_eq!(remote.position_ms(), 30_000);
}

#[tokio::test(start_paused = true)]
async fn fake_command_emits_property_change_once() {
    let (mut p, remote) = FakePlayer::new(FakeConfig::default());
    load(&mut p).await;
    p.play().await.unwrap();
    wait_for(&mut p, |e| *e == PlayerEvent::Pause(false)).await;
    assert!(!remote.paused());
}

#[tokio::test(start_paused = true)]
async fn fake_user_pause_emits_event_without_command() {
    let (mut p, remote) = FakePlayer::new(FakeConfig::default());
    load(&mut p).await;
    p.play().await.unwrap();
    remote.user_pause(true);
    wait_for(&mut p, |e| *e == PlayerEvent::Pause(true)).await;
    assert!(!remote.commands().contains(&FakeCmd::Pause));
}

#[tokio::test(start_paused = true)]
async fn fake_buffering_freezes_position() {
    let (mut p, remote) = FakePlayer::new(FakeConfig::default());
    load(&mut p).await;
    p.play().await.unwrap();
    remote.set_buffering(true);
    wait_for(&mut p, |e| *e == PlayerEvent::Buffering(true)).await;
    let frozen = remote.position_ms();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(remote.position_ms(), frozen);
}

#[tokio::test(start_paused = true)]
async fn fake_eof() {
    let (mut p, _remote) =
        FakePlayer::new(FakeConfig { duration_ms: 1_000, ..FakeConfig::default() });
    load(&mut p).await;
    p.play().await.unwrap();
    wait_for(&mut p, |e| *e == PlayerEvent::Eof).await;
}

#[tokio::test(start_paused = true)]
async fn fake_exit_ends_event_stream() {
    let (mut p, remote) = FakePlayer::new(FakeConfig::default());
    remote.exit(Some(0));
    wait_for(&mut p, |e| *e == PlayerEvent::Exited { code: Some(0) }).await;
    assert_eq!(p.next_event().await, None);
    assert!(p.play().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn fake_overlapping_seeks_report_once() {
    let (mut p, _remote) =
        FakePlayer::new(FakeConfig { seek_delay_ms: 300, ..FakeConfig::default() });
    load(&mut p).await;
    p.seek(10_000).await.unwrap();
    p.seek(50_000).await.unwrap();
    let ev = wait_for(&mut p, |e| matches!(e, PlayerEvent::Seeked { .. })).await;
    assert_eq!(ev, PlayerEvent::Seeked { position_ms: 50_000 });
    let more = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(PlayerEvent::Seeked { .. }) = p.next_event().await {
                return;
            }
        }
    })
    .await;
    assert!(more.is_err(), "a superseded seek must not report a second Seeked");
}
