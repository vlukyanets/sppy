//! Preferred audio and subtitle languages reach the player before every item, so it picks
//! the tracks inside the file (not only subtitle files next to it).

mod common;
mod support;

use common::*;
use sppy_client_core::api::UiCommand;
use sppy_client_core::config::ClientConfig;
use sppy_player::fake::{FakeCmd, FakeConfig};
use sppy_player::{MediaSource, TrackLanguages};
use support::*;

/// The languages the player had when it loaded `url`.
fn languages_at_load(p: &Peer, url: &str) -> Option<TrackLanguages> {
    let cmds = p.launcher.remote()?.commands();
    let load = cmds.iter().position(|c| *c == FakeCmd::Load(MediaSource::Url(url.into())))?;
    cmds[..load].iter().rev().find_map(|c| match c {
        FakeCmd::TrackLanguages(l) => Some(l.clone()),
        _ => None,
    })
}

fn langs(audio: &[&str], subtitles: &[&str]) -> TrackLanguages {
    let v = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    TrackLanguages { audio: v(audio), subtitles: v(subtitles) }
}

#[tokio::test]
async fn languages_are_set_before_every_load_and_follow_the_settings() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    let cfg = ClientConfig {
        audio_languages: vec!["ja".into()],
        subtitle_languages: vec!["en".into()],
        ..config("host")
    };
    let h = host_with(&server, FakeConfig::default(), cfg).await;
    add_and_play(&h, &["u1", "u2"]).await;
    eventually("u1 loaded with ja / en", 5, || {
        languages_at_load(&h, "https://example.invalid/u1") == Some(langs(&["ja"], &["en"]))
    })
    .await;

    h.send(UiCommand::SetTrackLanguages(langs(&["ru"], &["ru", "en"])));
    let u2 = snap(&h.handle).room.unwrap().playlist[1].item_id;
    h.send(UiCommand::PlaylistSelect(u2));
    eventually("u2 loaded with ru / ru, en", 5, || {
        languages_at_load(&h, "https://example.invalid/u2") == Some(langs(&["ru"], &["ru", "en"]))
    })
    .await;
    server.shutdown().await;
}
