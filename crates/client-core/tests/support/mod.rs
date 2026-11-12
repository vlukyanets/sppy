#![allow(dead_code)]
//! Shared helpers for client-core integration tests.

use std::time::Duration;

use sppy_client_core::api::{ClientHandle, ConnState, Snapshot, UiCommand, spawn_client};
use sppy_client_core::config::ClientConfig;
use sppy_player::fake::{FakeConfig, FakeLauncher};

use sppy_proto::*;
use sppy_server::ServerHandle;

pub fn config(nick: &str) -> ClientConfig {
    ClientConfig { nickname: nick.into(), ..ClientConfig::default() }
}

/// Poll the snapshot (and fakes) until `pred` holds.
pub async fn eventually(what: &str, secs: u64, mut pred: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !pred() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub fn snap(h: &ClientHandle) -> Snapshot {
    h.state.borrow().clone()
}

pub struct Peer {
    pub handle: ClientHandle,
    pub launcher: FakeLauncher,
}

impl Peer {
    pub fn remote(&self) -> sppy_player::fake::FakeRemote {
        self.launcher.remote().expect("player launched")
    }
    pub fn send(&self, cmd: UiCommand) {
        self.handle.commands.send(cmd).unwrap();
    }
    /// Playing and within the dead band of the room position.
    pub fn synced(&self) -> bool {
        self.launcher.remote().is_some_and(|r| !r.paused()) && self.drift().abs() < 80
    }
    pub fn drift(&self) -> i64 {
        let expected = self.handle.expected_position_ms().expect("expected position") as i64;
        self.remote().position_ms() as i64 - expected
    }
}

pub async fn host(server: &ServerHandle, fake: FakeConfig) -> Peer {
    host_with(server, fake, config("host")).await
}

pub async fn host_with(server: &ServerHandle, fake: FakeConfig, cfg: ClientConfig) -> Peer {
    let launcher = FakeLauncher::new(fake);
    let handle = spawn_client(cfg, launcher.clone());
    handle
        .commands
        .send(UiCommand::BecomeHostConnect {
            listen: server.local_addr,
            host_token: server.host_token.clone(),
        })
        .unwrap();
    let p = Peer { handle, launcher };
    eventually("host connected", 5, || snap(&p.handle).connection == ConnState::Connected).await;
    p
}

pub async fn guest(server: &ServerHandle, nick: &str, fake: FakeConfig) -> Peer {
    let launcher = FakeLauncher::new(fake);
    let handle = spawn_client(config(nick), launcher.clone());
    let address = format!("sppy://{}/{}", server.local_addr, server.invite_token());
    handle.commands.send(UiCommand::Connect { address, nickname: nick.into() }).unwrap();
    let p = Peer { handle, launcher };
    eventually("guest connected", 5, || snap(&p.handle).connection == ConnState::Connected).await;
    p
}

pub fn url(u: &str) -> Source {
    Source::Url { url: format!("https://example.invalid/{u}") }
}

pub fn is_playing(p: &Peer) -> bool {
    snap(&p.handle).playback.is_some_and(|pb| pb.is_playing())
}

pub async fn add_and_play(h: &Peer, urls: &[&str]) {
    h.send(UiCommand::PlaylistAdd(urls.iter().map(|u| url(u)).collect()));
    eventually("item ready (hold released)", 5, || {
        snap(&h.handle).playback.is_some_and(|pb| pb.item_id.is_some() && pb.hold.is_none())
    })
    .await;
    h.send(UiCommand::Play);
    eventually("room playing", 5, || is_playing(h)).await;
}
