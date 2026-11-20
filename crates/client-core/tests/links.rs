//! Links others add to unknown sites are opened only after asking (the player would
//! connect to whatever site the link names).

mod common;
mod support;

use common::*;
use sppy_client_core::api::UiCommand;
use sppy_player::MediaSource;
use sppy_player::fake::{FakeCmd, FakeConfig};
use sppy_proto::*;
use support::*;

const LINK: &str = "https://videos.example.net/a.mp4";

fn loaded(p: &Peer, url: &str) -> bool {
    p.launcher
        .remote()
        .is_some_and(|r| r.commands().contains(&FakeCmd::Load(MediaSource::Link(url.into()))))
}

fn my_status(p: &Peer) -> Option<MediaStatus> {
    let s = snap(&p.handle);
    s.room?.participants.into_iter().find(|x| Some(x.id) == s.me).map(|x| x.status)
}

struct Room {
    _dir: tempfile::TempDir,
    server: sppy_server::ServerHandle,
    h: Peer,
    g: Peer,
    item: ItemId,
}

/// The guest adds `link`; the host is the one who did not.
async fn room_with_a_guest_link(link: &str) -> Room {
    let dir = tempfile::tempdir().unwrap();
    let server = start_server(dir.path(), test_config()).await;
    // A slow player: what the test sees of the player and of the snapshot comes at
    // different times, so every check must wait for its condition.
    let h = host(&server, FakeConfig { cmd_latency_ms: 300, ..FakeConfig::default() }).await;
    let g = guest(&server, "g", FakeConfig::default()).await;
    g.send(UiCommand::PlaylistAdd(vec![Source::Url { url: link.into() }]));
    eventually("item current", 5, || snap(&h.handle).playback.is_some_and(|p| p.item_id.is_some()))
        .await;
    let item = snap(&h.handle).playback.unwrap().item_id.unwrap();
    Room { _dir: dir, server, h, g, item }
}

#[tokio::test]
async fn a_link_to_an_unknown_site_waits_for_my_answer() {
    let Room { _dir, server, h, g, item } = room_with_a_guest_link(LINK).await;
    eventually("host asked", 5, || {
        snap(&h.handle).link_question.is_some_and(|q| {
            q.item_id == item && q.site == "videos.example.net" && q.added_by == "g"
        })
    })
    .await;
    eventually("whoever added it plays it", 5, || loaded(&g, LINK)).await;
    assert!(!loaded(&h, LINK), "not opened before the answer");
    assert_eq!(my_status(&h), Some(MediaStatus::Loading), "the room waits for the answer");

    h.send(UiCommand::AllowLink(item));
    eventually("opened once allowed, question gone", 5, || {
        loaded(&h, LINK) && snap(&h.handle).link_question.is_none()
    })
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn skipping_a_link_lets_the_room_go_on_without_me() {
    let Room { _dir, server, h, g: _g, item } = room_with_a_guest_link(LINK).await;
    eventually("host asked", 5, || snap(&h.handle).link_question.is_some()).await;
    h.send(UiCommand::SkipLink(item));
    eventually("skipped, question gone", 5, || {
        matches!(my_status(&h), Some(MediaStatus::Failed { .. }))
            && snap(&h.handle).link_question.is_none()
    })
    .await;
    assert!(!loaded(&h, LINK));
    eventually("the room no longer waits for me", 5, || {
        snap(&h.handle).playback.is_some_and(|p| p.hold.is_none())
    })
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn trusting_the_site_opens_its_links() {
    let Room { _dir, server, h, g: _g, .. } = room_with_a_guest_link(LINK).await;
    eventually("host asked", 5, || snap(&h.handle).link_question.is_some()).await;
    h.send(UiCommand::SetTrustedSites(vec!["example.net".into()]));
    eventually("opened, question gone", 5, || {
        loaded(&h, LINK) && snap(&h.handle).link_question.is_none()
    })
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn known_video_sites_play_without_asking() {
    let link = "https://www.youtube.com/watch?v=abc";
    let Room { _dir, server, h, g: _g, .. } = room_with_a_guest_link(link).await;
    eventually("opened", 5, || loaded(&h, link)).await;
    assert_eq!(snap(&h.handle).link_question, None);
    server.shutdown().await;
}
