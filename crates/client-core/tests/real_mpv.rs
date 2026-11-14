//! Two clients with real (headless) mpv instances stay in sync. Skipped without mpv.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use sppy_client_core::api::{ClientHandle, ConnState, UiCommand, spawn_client};
use sppy_client_core::config::ClientConfig;
use sppy_player::mpv::{MpvConfig, MpvLauncher};
use sppy_proto::Source;

fn make_clip(dir: &Path, secs: u32) -> Option<PathBuf> {
    let out = dir.join("clip.mkv");
    let ok = std::process::Command::new("mpv")
        .args(["--no-config", "--no-terminal", "--ao=null", "--of=matroska", "--ovc=mpeg4"])
        .arg(format!("av://lavfi:testsrc=duration={secs}:size=160x120:rate=25"))
        .arg(format!("--o={}", out.display()))
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(out)
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
async fn two_real_mpv_clients_stay_in_sync() {
    let dir = tempfile::tempdir().unwrap();
    let Some(clip) = make_clip(dir.path(), 60) else {
        eprintln!("mpv not available or cannot encode, skipping");
        return;
    };
    let cfg = sppy_server::ServerConfig {
        media: vec![sppy_server::MediaRoot {
            name: "Clips".into(),
            path: dir.path().to_path_buf(),
        }],
        ..test_config()
    };
    let server = start_server(&dir.path().join("server"), cfg).await;
    let file_id = sppy_server::library::file_id("Clips", Path::new("clip.mkv"));
    let _ = &clip;
    let launcher = MpvLauncher::new(MpvConfig { headless: true, ..MpvConfig::default() });
    let cfg = |nick: &str| ClientConfig { nickname: nick.into(), ..ClientConfig::default() };
    let a = spawn_client(cfg("a"), launcher.clone());
    let b = spawn_client(cfg("b"), launcher);
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
    let mut worst = 0;
    for _ in 0..40 {
        let (da, db) = (drift(&a).expect("a position"), drift(&b).expect("b position"));
        worst = worst.max(da.abs()).max(db.abs());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!("real mpv worst drift: {worst} ms");
    assert!(worst < 80, "worst drift {worst} ms");
    // A seek from b reaches a.
    b.commands.send(UiCommand::Seek(40_000)).unwrap();
    wait("a jumped", 5, || a.state.borrow().player_position_ms.is_some_and(|p| p >= 40_000)).await;
    server.shutdown().await;
}
