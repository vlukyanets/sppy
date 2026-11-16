use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sppy_client_core::download::{DownloadEvent, DownloadJob, Downloader, Priority};
use sppy_proto::FileId;
use tokio::sync::mpsc;
use tower::ServiceExt;
use tower_http::services::ServeFile;

#[derive(Clone)]
struct Srv {
    file: PathBuf,
    fail_first: Arc<AtomicUsize>,
    ranges: Arc<Mutex<Vec<Option<String>>>>,
}

async fn serve(State(s): State<Srv>, req: Request) -> Response {
    s.ranges
        .lock()
        .unwrap()
        .push(req.headers().get("range").map(|v| v.to_str().unwrap().to_owned()));
    if s.fail_first.load(Ordering::SeqCst) > 0 {
        s.fail_first.fetch_sub(1, Ordering::SeqCst);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    ServeFile::new(&s.file).oneshot(req).await.unwrap().map(Body::new)
}

async fn serve_full(State(s): State<Srv>) -> Response {
    tokio::fs::read(&s.file).await.unwrap().into_response()
}

/// A hostile server: 64 KiB chunks forever, no Content-Length.
async fn serve_endless() -> Response {
    let chunk = axum::body::Bytes::from(vec![0u8; 64 * 1024]);
    let stream = futures_util::stream::repeat_with(move || Ok::<_, std::io::Error>(chunk.clone()));
    Body::from_stream(stream).into_response()
}

fn content(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 % 256) as u8).collect()
}

async fn start(dir: &Path, data: &[u8], fail_first: usize) -> (SocketAddr, Srv) {
    let file = dir.join("source.bin");
    std::fs::write(&file, data).unwrap();
    let srv =
        Srv { file, fail_first: Arc::new(AtomicUsize::new(fail_first)), ranges: Arc::default() };
    let app = Router::new()
        .route("/f", get(serve))
        .route("/full", get(serve_full))
        .route("/endless", get(serve_endless))
        .with_state(srv.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, srv)
}

fn job(id: &str, url: String, dest: PathBuf) -> DownloadJob {
    DownloadJob {
        file_id: FileId(id.into()),
        url,
        dest,
        subs: vec![],
        total_hint: None,
        max_bytes: u64::MAX,
    }
}

fn fast() -> Vec<Duration> {
    vec![Duration::from_millis(10); 5]
}

async fn until_final(
    rx: &mut mpsc::UnboundedReceiver<DownloadEvent>,
) -> (DownloadEvent, Vec<DownloadEvent>) {
    let mut seen = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
        if matches!(ev, DownloadEvent::Done { .. } | DownloadEvent::Failed { .. }) {
            return (ev, seen);
        }
        seen.push(ev);
    }
}

#[tokio::test]
async fn downloads_whole_file() {
    let dir = tempfile::tempdir().unwrap();
    let data = content(300_000);
    let (addr, _) = start(dir.path(), &data, 0).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    let dest = dir.path().join("out/a");
    dl.enqueue(job("a", format!("http://{addr}/f"), dest.clone()), Priority::Current);
    let (ev, _) = until_final(&mut rx).await;
    assert_eq!(ev, DownloadEvent::Done { file_id: FileId("a".into()) });
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert!(!dest.with_extension("part").exists());
}

#[tokio::test]
async fn resumes_partial_with_range() {
    let dir = tempfile::tempdir().unwrap();
    let data = content(200_000);
    let (addr, srv) = start(dir.path(), &data, 0).await;
    let dest = dir.path().join("a");
    std::fs::write(sppy_client_core::download::part_path(&dest), &data[..80_000]).unwrap();
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    dl.enqueue(job("a", format!("http://{addr}/f"), dest.clone()), Priority::Current);
    let (ev, _) = until_final(&mut rx).await;
    assert!(matches!(ev, DownloadEvent::Done { .. }));
    assert_eq!(srv.ranges.lock().unwrap()[0].as_deref(), Some("bytes=80000-"));
    assert_eq!(std::fs::read(&dest).unwrap(), data);
}

#[tokio::test]
async fn restarts_when_server_ignores_range() {
    let dir = tempfile::tempdir().unwrap();
    let data = content(50_000);
    let (addr, _) = start(dir.path(), &data, 0).await;
    let dest = dir.path().join("a");
    std::fs::write(sppy_client_core::download::part_path(&dest), b"garbage!").unwrap();
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    dl.enqueue(job("a", format!("http://{addr}/full"), dest.clone()), Priority::Current);
    let (ev, _) = until_final(&mut rx).await;
    assert!(matches!(ev, DownloadEvent::Done { .. }));
    assert_eq!(std::fs::read(&dest).unwrap(), data);
}

#[tokio::test]
async fn retries_then_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let data = content(10_000);
    let (addr, _) = start(dir.path(), &data, 3).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    dl.enqueue(job("a", format!("http://{addr}/f"), dir.path().join("a")), Priority::Current);
    let (ev, _) = until_final(&mut rx).await;
    assert!(matches!(ev, DownloadEvent::Done { .. }));
}

#[tokio::test]
async fn fails_after_5_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, srv) = start(dir.path(), &content(10), 100).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    dl.enqueue(job("a", format!("http://{addr}/f"), dir.path().join("a")), Priority::Current);
    let (ev, _) = until_final(&mut rx).await;
    assert_eq!(ev, DownloadEvent::Failed { file_id: FileId("a".into()), attempts: 5 });
    assert_eq!(srv.ranges.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn reports_progress() {
    let dir = tempfile::tempdir().unwrap();
    let data = content(100_000);
    let (addr, _) = start(dir.path(), &data, 0).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    dl.enqueue(job("a", format!("http://{addr}/f"), dir.path().join("a")), Priority::Current);
    let (_, seen) = until_final(&mut rx).await;
    let last = seen.iter().rev().find_map(|e| match e {
        DownloadEvent::Progress { bytes, total, .. } => Some((*bytes, *total)),
        _ => None,
    });
    assert_eq!(last, Some((100_000, 100_000)), "final progress is always reported: {seen:?}");
}

#[tokio::test]
async fn current_priority_before_prefetch() {
    let dir = tempfile::tempdir().unwrap();
    let data = content(1_000);
    let (addr, _) = start(dir.path(), &data, 0).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    dl.pause_for_test(true);
    dl.enqueue(job("p", format!("http://{addr}/f"), dir.path().join("p")), Priority::Prefetch);
    dl.enqueue(job("c", format!("http://{addr}/f"), dir.path().join("c")), Priority::Current);
    dl.pause_for_test(false);
    let (first, _) = until_final(&mut rx).await;
    let (second, _) = until_final(&mut rx).await;
    assert_eq!(first, DownloadEvent::Done { file_id: FileId("c".into()) });
    assert_eq!(second, DownloadEvent::Done { file_id: FileId("p".into()) });
}

#[tokio::test]
async fn downloads_subtitles_with_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let data = content(1_000);
    let (addr, _) = start(dir.path(), &data, 0).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    let mut j = job("a", format!("http://{addr}/f"), dir.path().join("a"));
    j.subs = vec![(format!("http://{addr}/full"), dir.path().join("a.sub0.srt"))];
    dl.enqueue(j, Priority::Current);
    until_final(&mut rx).await;
    assert_eq!(std::fs::read(dir.path().join("a.sub0.srt")).unwrap(), data);
}

fn files(dir: &Path) -> Vec<(String, u64)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().into_string().unwrap(), e.metadata().unwrap().len()))
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn stops_when_the_server_sends_more_than_announced() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _) = start(dir.path(), &content(10), 0).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    let mut j = job("a", format!("http://{addr}/endless"), dir.path().join("a"));
    j.total_hint = Some(100_000);
    dl.enqueue(j, Priority::Current);
    let (last, _) = until_final(&mut rx).await;
    assert!(matches!(last, DownloadEvent::Failed { .. }), "{last:?}");
    assert!(!dir.path().join("a").exists());
    assert!(files(dir.path()).iter().all(|(_, size)| *size <= 100_000), "{:?}", files(dir.path()));
}

#[tokio::test]
async fn refuses_a_file_bigger_than_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, srv) = start(dir.path(), &content(100_000), 0).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    let mut j = job("a", format!("http://{addr}/f"), dir.path().join("a"));
    j.max_bytes = 50_000;
    dl.enqueue(j, Priority::Current);
    let (last, _) = until_final(&mut rx).await;
    assert_eq!(last, DownloadEvent::Failed { file_id: FileId("a".into()), attempts: 1 });
    assert!(!dir.path().join("a").exists() && !dir.path().join("a.part").exists());
    assert_eq!(srv.ranges.lock().unwrap().len(), 1, "a size refusal is not retried");
}

#[tokio::test]
async fn refuses_a_size_other_than_the_fingerprint_says() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _) = start(dir.path(), &content(100_000), 0).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    let mut j = job("a", format!("http://{addr}/f"), dir.path().join("a"));
    j.total_hint = Some(99_999);
    dl.enqueue(j, Priority::Current);
    let (last, _) = until_final(&mut rx).await;
    assert_eq!(last, DownloadEvent::Failed { file_id: FileId("a".into()), attempts: 1 });
    assert!(!dir.path().join("a").exists());
}

#[tokio::test]
async fn an_endless_subtitle_is_cut_off_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _) = start(dir.path(), &content(1_000), 0).await;
    let (dl, mut rx) = Downloader::with_backoff(reqwest::Client::new(), fast());
    let mut j = job("a", format!("http://{addr}/f"), dir.path().join("a"));
    j.max_bytes = 1 << 20;
    j.subs = vec![(format!("http://{addr}/endless"), dir.path().join("a.sub0.srt"))];
    dl.enqueue(j, Priority::Current);
    let (last, _) = until_final(&mut rx).await;
    assert_eq!(last, DownloadEvent::Done { file_id: FileId("a".into()) });
    assert!(dir.path().join("a").exists(), "the video itself is fine");
    let left: Vec<_> = files(dir.path()).into_iter().filter(|(n, _)| n.contains(".sub0")).collect();
    assert!(left.is_empty(), "nothing of the subtitle is left: {left:?}");
}
