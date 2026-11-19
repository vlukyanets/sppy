//! Preferred subtitle languages reach the player, so it also picks tracks inside the file
//! (not only the subtitle files next to it).

mod common;
mod support;

use common::*;
use sppy_client_core::api::UiCommand;
use sppy_client_core::config::ClientConfig;
use sppy_player::MediaSource;
use sppy_player::fake::{FakeCmd, FakeConfig};
use support::*;

/// The languages the player had when it loaded `url`.
fn languages_at_load(p: &Peer, url: &str) -> Option<Vec<String>> {
    let cmds = p.launcher.remote()?.commands();
    let load = cmds.iter().position(|c| *c == FakeCmd::Load(MediaSource::Url(url.into())))?;
    cmds[..load].iter().rev().find_map(|c| match c {
        FakeCmd::SubtitleLanguages(l) => Some(l.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn languages_are_set_before_every_load_and_follow_the_settings() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let cfg = ClientConfig { subtitle_languages: vec!["en".into()], ..config("host") };
    let h = host_with(&server, FakeConfig::default(), cfg).await;
    add_and_play(&h, &["u1", "u2"]).await;
    eventually("u1 loaded with en", 5, || {
        languages_at_load(&h, "https://example.invalid/u1") == Some(vec!["en".into()])
    })
    .await;

    h.send(UiCommand::SetSubtitleLanguages(vec!["ru".into(), "en".into()]));
    let u2 = snap(&h.handle).room.unwrap().playlist[1].item_id;
    h.send(UiCommand::PlaylistSelect(u2));
    eventually("u2 loaded with ru, en", 5, || {
        languages_at_load(&h, "https://example.invalid/u2") == Some(vec!["ru".into(), "en".into()])
    })
    .await;
    server.shutdown().await;
}
