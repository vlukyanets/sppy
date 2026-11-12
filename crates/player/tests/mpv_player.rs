use std::time::Duration;

use sppy_player::mpv::{MpvConfig, MpvLauncher, MpvPlayer};
use sppy_player::{MediaSource, Player, PlayerEvent, PlayerLauncher};

const MEDIA: &str = "av://lavfi:testsrc=duration=10:size=320x240:rate=25";

fn mpv_available() -> bool {
    std::process::Command::new("mpv").arg("--version").output().is_ok_and(|o| o.status.success())
}

async fn launch() -> Option<MpvPlayer> {
    if !mpv_available() {
        eprintln!("mpv not found, skipping");
        return None;
    }
    let cfg = MpvConfig { headless: true, ..MpvConfig::default() };
    Some(MpvLauncher(cfg).launch().await.expect("launch mpv"))
}

async fn wait_for(
    p: &mut MpvPlayer,
    secs: u64,
    pred: impl Fn(&PlayerEvent) -> bool,
) -> PlayerEvent {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            let ev = p.next_event().await.expect("player alive");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("event did not arrive in time")
}

async fn load(p: &mut MpvPlayer) -> PlayerEvent {
    p.load(MediaSource::Url(MEDIA.into()), vec![]).await.unwrap();
    wait_for(p, 10, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await
}

/// Encode a short real file with mpv itself (lavfi sources have no duration).
fn make_clip(dir: &std::path::Path, secs: u32) -> Option<std::path::PathBuf> {
    let out = dir.join("clip.mkv");
    let ok = std::process::Command::new("mpv")
        .args(["--no-config", "--no-terminal", "--ao=null", "--of=matroska", "--ovc=mpeg4"])
        .arg(format!("av://lavfi:testsrc=duration={secs}:size=160x120:rate=10"))
        .arg(format!("--o={}", out.display()))
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(out)
}

#[tokio::test]
async fn mpv_load_reports_duration() {
    let Some(mut p) = launch().await else { return };
    let dir = tempfile::tempdir().unwrap();
    let Some(clip) = make_clip(dir.path(), 3) else {
        eprintln!("mpv cannot encode, skipping");
        return;
    };
    p.load(MediaSource::Path(clip), vec![]).await.unwrap();
    let ev = wait_for(&mut p, 10, |e| matches!(e, PlayerEvent::FileLoaded { .. })).await;
    let PlayerEvent::FileLoaded { duration_ms: Some(d), .. } = ev else { panic!("{ev:?}") };
    assert!((2_500..=3_100).contains(&d), "{d}");
}

#[tokio::test]
async fn mpv_load_play_reports_time() {
    let Some(mut p) = launch().await else { return };
    load(&mut p).await;
    p.play().await.unwrap();
    wait_for(&mut p, 5, |e| matches!(e, PlayerEvent::TimePos(ms) if *ms >= 500)).await;
}

#[tokio::test]
async fn mpv_seek_reports_seeked() {
    let Some(mut p) = launch().await else { return };
    load(&mut p).await;
    p.seek(5000).await.unwrap();
    let ev = wait_for(&mut p, 5, |e| matches!(e, PlayerEvent::Seeked { .. })).await;
    let PlayerEvent::Seeked { position_ms } = ev else { unreachable!() };
    assert!((4_900..=5_100).contains(&position_ms), "{position_ms}");
}

#[tokio::test]
async fn mpv_pause_event() {
    let Some(mut p) = launch().await else { return };
    load(&mut p).await;
    p.play().await.unwrap();
    wait_for(&mut p, 5, |e| *e == PlayerEvent::Pause(false)).await;
    p.pause().await.unwrap();
    wait_for(&mut p, 5, |e| *e == PlayerEvent::Pause(true)).await;
}

#[tokio::test]
async fn mpv_speed_event() {
    let Some(mut p) = launch().await else { return };
    load(&mut p).await;
    p.set_speed(1.5).await.unwrap();
    wait_for(&mut p, 5, |e| *e == PlayerEvent::Speed(1.5)).await;
}

#[tokio::test]
async fn mpv_eof_without_pause() {
    let Some(mut p) = launch().await else { return };
    load(&mut p).await;
    p.seek(9_500).await.unwrap();
    wait_for(&mut p, 5, |e| matches!(e, PlayerEvent::Seeked { .. })).await;
    p.play().await.unwrap();
    // keep-open must not pause at the end (that would look like a user pause).
    let ev =
        wait_for(&mut p, 5, |e| matches!(e, PlayerEvent::Eof | PlayerEvent::Pause(true))).await;
    assert_eq!(ev, PlayerEvent::Eof);
}

#[tokio::test]
async fn mpv_osd_ok() {
    let Some(mut p) = launch().await else { return };
    p.osd("hello", 1000).await.unwrap();
}

#[tokio::test]
async fn mpv_load_failure_reported() {
    let Some(mut p) = launch().await else { return };
    p.load(MediaSource::Path("/nonexistent/sppy-missing.mkv".into()), vec![]).await.unwrap();
    wait_for(&mut p, 5, |e| matches!(e, PlayerEvent::LoadFailed { .. })).await;
}

#[tokio::test]
async fn mpv_quit_gives_exited() {
    let Some(mut p) = launch().await else { return };
    p.quit().await;
    let ev = wait_for(&mut p, 5, |e| matches!(e, PlayerEvent::Exited { .. })).await;
    assert_eq!(ev, PlayerEvent::Exited { code: Some(0) });
    let rest = tokio::time::timeout(Duration::from_secs(5), p.next_event()).await.unwrap();
    assert_eq!(rest, None);
}
