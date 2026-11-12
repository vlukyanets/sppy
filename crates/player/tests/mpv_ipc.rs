use std::time::Duration;

use serde_json::json;
use sppy_player::mpv::ipc::{IpcPath, MpvIpc, new_ipc_path};
use sppy_player::mpv::wire::MpvIncoming;

fn mpv_available() -> bool {
    std::process::Command::new("mpv").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn spawn_mpv(path: &IpcPath) -> tokio::process::Child {
    tokio::process::Command::new("mpv")
        .args(["--idle=yes", "--no-terminal", "--vo=null", "--ao=null", "--no-config"])
        .arg(format!("--input-ipc-server={}", path.as_arg()))
        .kill_on_drop(true)
        .spawn()
        .expect("spawn mpv")
}

#[tokio::test]
async fn real_mpv_get_property() {
    if !mpv_available() {
        eprintln!("mpv not found, skipping");
        return;
    }
    let path = new_ipc_path();
    let _child = spawn_mpv(&path);
    let (ipc, _events) = MpvIpc::connect(&path, Duration::from_secs(5)).await.expect("connect");
    let v = ipc.command(vec![json!("get_property"), json!("mpv-version")]).await.unwrap();
    assert!(v.as_str().unwrap().starts_with("mpv"), "{v}");
    let err = ipc.command(vec![json!("get_property"), json!("no-such-property")]).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn real_mpv_events_and_close() {
    if !mpv_available() {
        eprintln!("mpv not found, skipping");
        return;
    }
    let path = new_ipc_path();
    let _child = spawn_mpv(&path);
    let (ipc, mut events) = MpvIpc::connect(&path, Duration::from_secs(5)).await.expect("connect");
    ipc.command(vec![json!("observe_property"), json!(1), json!("pause")]).await.unwrap();
    let ev = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
    assert!(
        matches!(ev, MpvIncoming::Event { ref event, ref name, .. } if event == "property-change" && name.as_deref() == Some("pause"))
    );
    let _ = ipc.command(vec![json!("quit")]).await;
    // After mpv quits, the event stream ends and commands fail.
    tokio::time::timeout(Duration::from_secs(5), async { while events.recv().await.is_some() {} })
        .await
        .expect("event stream ends");
    assert!(ipc.command(vec![json!("get_property"), json!("pause")]).await.is_err());
}

#[tokio::test]
async fn connect_times_out_without_mpv() {
    let path = new_ipc_path();
    let r = MpvIpc::connect(&path, Duration::from_millis(200)).await;
    assert!(r.is_err());
}
