mod common;
mod support;

use common::*;
use sppy_client_core::api::{UiCommand, UiEvent};
use sppy_player::fake::FakeConfig;
use sppy_server::{MediaRoot, ServerConfig};
use support::*;

fn names(h: &Peer) -> Option<Vec<String>> {
    snap(&h.handle).server_folders.map(|f| f.folders.into_iter().map(|f| f.name).collect())
}

#[tokio::test]
async fn host_manages_the_server_folders() {
    let (dir, movies, series) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let cfg = ServerConfig {
        media: vec![MediaRoot { name: "Movies".into(), path: movies.path().to_path_buf() }],
        ..test_config()
    };
    let server = start_server(dir.path(), cfg).await;
    let h = host(&server, FakeConfig::default()).await;
    eventually("folders shown", 5, || names(&h) == Some(vec!["Movies".into()])).await;
    assert!(snap(&h.handle).server_folders.unwrap().editable);

    let path = series.path().to_string_lossy().into_owned();
    h.send(UiCommand::AddServerFolder { path, name: Some("TV".into()) });
    eventually("folder added", 5, || names(&h) == Some(vec!["Movies".into(), "TV".into()])).await;

    let mut events = h.handle.subscribe_events();
    h.send(UiCommand::AddServerFolder { path: "relative".into(), name: None });
    let mut error = None;
    eventually("refusal shown", 5, || {
        while let Ok(ev) = events.try_recv() {
            if let UiEvent::Error(t) = ev {
                error = Some(t);
            }
        }
        error.is_some()
    })
    .await;
    assert_eq!(error.as_deref(), Some("Enter the full path of a folder on the server"));

    h.send(UiCommand::RemoveServerFolder { name: "Movies".into() });
    eventually("folder removed", 5, || names(&h) == Some(vec!["TV".into()])).await;

    let g = guest(&server, "g", FakeConfig::default()).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(names(&g), None, "guests do not see the server's folders");
    server.shutdown().await;
}
