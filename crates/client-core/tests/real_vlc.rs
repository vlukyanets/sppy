//! A client with real (headless) mpv and one with real VLC stay in sync. Skipped without
//! either player. Unix only: VLC runs with its data and config in a temporary folder.
//!
//! A real-time measurement, not a unit test: it runs only by hand:
//! `cargo test -p sppy-client-core --test real_vlc -- --ignored --nocapture`.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use sppy_client_core::api::{ClientHandle, ConnState, UiCommand, spawn_client};
use sppy_client_core::config::ClientConfig;
use sppy_player::mpv::{MpvConfig, MpvLauncher};
use sppy_player::vlc::{VlcConfig, VlcLauncher};
use sppy_proto::Source;

/// A clip with sound: without an audio track VLC's time moves in whole video frames
/// (40 ms here), which real files do not do.
fn make_clip(dir: &Path, secs: u32) -> bool {
    std::process::Command::new("mpv")
        .args(["--no-config", "--no-terminal", "--of=matroska", "--ovc=mpeg4", "--oac=aac"])
        .arg(format!("av://lavfi:testsrc=duration={secs}:size=160x120:rate=25"))
        .arg(format!("--audio-file=av://lavfi:sine=frequency=440:duration={secs}"))
        .arg(format!("--o={}", dir.join("clip.mkv").display()))
        .status()
        .is_ok_and(|s| s.success())
}

fn vlc_available() -> bool {
    std::process::Command::new("vlc").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn drift(h: &ClientHandle) -> Option<i64> {
    let pos = h.state.borrow().player_position_ms? as i64;
    Some(pos - h.expected_position_ms()? as i64)
}

async fn wait(what: &str, secs: u64, pred: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !pred() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real-time measurement with real mpv and VLC: run by hand"]
async fn real_mpv_and_vlc_clients_stay_in_sync() {
    let dir = tempfile::tempdir().unwrap();
    if !vlc_available() || !make_clip(dir.path(), 60) {
        eprintln!("mpv or VLC not available, skipping");
        return;
    }
    let cfg = sppy_server::ServerConfig {
        media: vec![sppy_server::MediaRoot {
            name: "Clips".into(),
            path: dir.path().to_path_buf(),
        }],
        ..test_config()
    };
    let server = start_server(&dir.path().join("server"), cfg).await;
    let file_id = sppy_server::library::file_id("Clips", Path::new("clip.mkv"));
    let mpv = MpvLauncher::new(MpvConfig { headless: true, ..MpvConfig::default() });
    let vlc = VlcLauncher::new(VlcConfig {
        headless: true,
        isolated_home: Some(dir.path().join("vlc-home")),
        ..VlcConfig::default()
    });
    let cfg = |nick: &str| ClientConfig { nickname: nick.into(), ..ClientConfig::default() };
    let a = spawn_client(cfg("a"), mpv);
    let b = spawn_client(cfg("b"), vlc);
    a.commands
        .send(UiCommand::BecomeHostConnect {
            listen: server.local_addr,
            host_token: server.host_token.clone(),
        })
        .unwrap();
    let invite = format!("sppy://{}/{}", server.local_addr, server.invite_token());
    b.commands.send(UiCommand::Connect { address: invite, nickname: "b".into() }).unwrap();
    wait("connected", 10, || {
        a.state.borrow().connection == ConnState::Connected
            && b.state.borrow().connection == ConnState::Connected
    })
    .await;
    a.commands.send(UiCommand::PlaylistAdd(vec![Source::File { file_id }])).unwrap();
    wait("ready", 10, || {
        a.state.borrow().playback.as_ref().is_some_and(|p| p.item_id.is_some() && p.hold.is_none())
    })
    .await;
    a.commands.send(UiCommand::Play).unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    let (mut worst_a, mut worst_b) = (0, 0);
    for _ in 0..40 {
        let (da, db) = (drift(&a).expect("a position"), drift(&b).expect("b position"));
        worst_a = worst_a.max(da.abs());
        worst_b = worst_b.max(db.abs());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!("worst drift: mpv {worst_a} ms, VLC {worst_b} ms");
    assert!(worst_a < 80 && worst_b < 80, "worst drift: mpv {worst_a} ms, VLC {worst_b} ms");
    // A seek in the VLC client reaches the mpv one, and back.
    b.commands.send(UiCommand::Seek(40_000)).unwrap();
    wait("a jumped", 5, || a.state.borrow().player_position_ms.is_some_and(|p| p >= 40_000)).await;
    a.commands.send(UiCommand::Seek(10_000)).unwrap();
    wait("b jumped back", 5, || {
        b.state.borrow().player_position_ms.is_some_and(|p| (10_000..20_000).contains(&p))
    })
    .await;
    server.shutdown().await;
}
