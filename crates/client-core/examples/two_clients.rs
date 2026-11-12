//! Manual check: an embedded server and two clients, each with its own mpv window.
//!
//! ```text
//! cargo run -p sppy-client-core --example two_clients [-- <media file>]
//! ```
//! Pause or seek in either window; the other one follows. Drift is printed every second.
//! Without an argument a 5-minute test clip is generated with mpv first. Ctrl-C quits.

use std::time::Duration;

use sppy_client_core::api::{ClientHandle, ConnState, UiCommand, UiEvent, spawn_client};
use sppy_client_core::config::ClientConfig;
use sppy_player::mpv::{MpvConfig, MpvLauncher};
use sppy_proto::Source;
use sppy_server::ServerConfig;

fn drift(h: &ClientHandle) -> String {
    let pos = h.state.borrow().player_position_ms;
    match (pos, h.expected_position_ms()) {
        (Some(p), Some(e)) => format!("{:+5} ms", p as i64 - e as i64),
        _ => "  n/a".into(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let media_dir = dir.path().join("media");
    std::fs::create_dir_all(&media_dir)?;
    match std::env::args().nth(1) {
        Some(file) => {
            let src = std::path::PathBuf::from(&file);
            let name = src.file_name().ok_or_else(|| anyhow::anyhow!("not a file: {file}"))?;
            std::fs::copy(&src, media_dir.join(name))?;
        }
        None => {
            // lavfi sources are not seekable: encode a real clip.
            println!("generating a test clip…");
            let ok = std::process::Command::new("mpv")
                .args(["--no-config", "--no-terminal", "--ao=null", "--of=matroska", "--ovc=mpeg4"])
                .arg("av://lavfi:testsrc=duration=300:size=640x360:rate=25")
                .arg(format!("--o={}", media_dir.join("clip.mkv").display()))
                .status()?
                .success();
            anyhow::ensure!(ok, "mpv could not encode the test clip; pass a media file instead");
        }
    }
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse()?,
        media: vec![sppy_server::MediaRoot { name: "Media".into(), path: media_dir.clone() }],
        ..ServerConfig::default()
    };
    let server = sppy_server::start(config, dir.path().join("server")).await?;
    let file_id = std::fs::read_dir(&media_dir)?
        .flatten()
        .next()
        .map(|e| sppy_server::library::file_id("Media", std::path::Path::new(&e.file_name())))
        .ok_or_else(|| anyhow::anyhow!("no media file"))?;
    let launcher = MpvLauncher(MpvConfig::default());
    let cfg = |nick: &str| ClientConfig { nickname: nick.into(), ..ClientConfig::default() };
    let a = spawn_client(cfg("alice"), launcher.clone());
    let b = spawn_client(cfg("bob"), launcher);
    a.commands.send(UiCommand::BecomeHostConnect {
        listen: server.local_addr,
        host_token: server.host_token.clone(),
    })?;
    let invite = format!("sppy://{}/{}", server.local_addr, server.invite_token());
    println!("invite link (for a third client): {invite}");
    b.commands.send(UiCommand::Connect { address: invite, nickname: "bob".into() })?;
    while a.state.borrow().connection != ConnState::Connected
        || b.state.borrow().connection != ConnState::Connected
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    a.commands.send(UiCommand::PlaylistAdd(vec![Source::File { file_id }]))?;
    let mut events = a.subscribe_events();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let started = std::sync::atomic::AtomicBool::new(false);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            ev = events.recv() => if let Ok(UiEvent::Notice(t) | UiEvent::Error(t)) = ev { println!("· {t}") },
            _ = tick.tick() => {
                let ready = a.state.borrow().playback.as_ref().is_some_and(|p| p.item_id.is_some() && p.hold.is_none());
                if ready && !started.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    a.commands.send(UiCommand::Play)?;
                }
                println!("drift  alice {}   bob {}", drift(&a), drift(&b));
            }
        }
    }
    server.shutdown().await;
    Ok(())
}
