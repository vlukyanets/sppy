//! Preload downloader: one file at a time, current item first, resume via Range, retries (spec §7.1).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::StatusCode;
use sppy_proto::FileId;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub const DEFAULT_BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];
/// Spec §5.3: progress is reported at most once per second.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, PartialEq)]
pub struct DownloadJob {
    pub file_id: FileId,
    pub url: String,
    /// Final location; data goes to `<dest>.part` until complete.
    pub dest: PathBuf,
    /// `(url, path)` of subtitles fetched after the video (best effort).
    pub subs: Vec<(String, PathBuf)>,
    pub total_hint: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Current,
    Prefetch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEvent {
    Progress { file_id: FileId, bytes: u64, total: u64 },
    Done { file_id: FileId },
    Failed { file_id: FileId, attempts: u32 },
}

/// `<dest>.part`
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

enum Cmd {
    Enqueue(DownloadJob, Priority),
    Cancel(FileId),
    Pause(bool),
}

#[derive(Clone)]
pub struct Downloader {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Downloader {
    pub fn spawn(client: reqwest::Client) -> (Downloader, mpsc::UnboundedReceiver<DownloadEvent>) {
        Self::with_backoff(client, DEFAULT_BACKOFF.to_vec())
    }

    /// `backoff[i]` is the pause after failed attempt `i + 1`; its length is the attempt limit.
    pub fn with_backoff(
        client: reqwest::Client,
        backoff: Vec<Duration>,
    ) -> (Downloader, mpsc::UnboundedReceiver<DownloadEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(worker(client, backoff, rx, ev_tx));
        (Downloader { tx }, ev_rx)
    }

    pub fn enqueue(&self, job: DownloadJob, priority: Priority) {
        let _ = self.tx.send(Cmd::Enqueue(job, priority));
    }

    pub fn cancel(&self, file_id: &FileId) {
        let _ = self.tx.send(Cmd::Cancel(file_id.clone()));
    }

    #[doc(hidden)]
    pub fn pause_for_test(&self, paused: bool) {
        let _ = self.tx.send(Cmd::Pause(paused));
    }
}

struct Active {
    job: DownloadJob,
    priority: Priority,
    handle: JoinHandle<()>,
}

async fn worker(
    client: reqwest::Client,
    backoff: Vec<Duration>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    ev_tx: mpsc::UnboundedSender<DownloadEvent>,
) {
    let mut queue: VecDeque<(DownloadJob, Priority)> = VecDeque::new();
    let mut active: Option<Active> = None;
    let mut paused = false;
    loop {
        if active.is_none() && !paused {
            let next = queue
                .iter()
                .position(|(_, p)| *p == Priority::Current)
                .or((!queue.is_empty()).then_some(0));
            if let Some(i) = next {
                let (job, priority) = queue.remove(i).expect("index from position");
                let handle = tokio::spawn(run_job(
                    client.clone(),
                    backoff.clone(),
                    job.clone(),
                    ev_tx.clone(),
                ));
                active = Some(Active { job, priority, handle });
            }
        }
        let finished = async {
            match &mut active {
                Some(a) => {
                    let _ = (&mut a.handle).await;
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = finished => active = None,
            cmd = rx.recv() => match cmd {
                None => {
                    if let Some(a) = active.take() {
                        a.handle.abort();
                    }
                    return;
                }
                Some(Cmd::Pause(p)) => paused = p,
                Some(Cmd::Cancel(id)) => {
                    queue.retain(|(j, _)| j.file_id != id);
                    if active.as_ref().is_some_and(|a| a.job.file_id == id) {
                        active.take().expect("checked").handle.abort();
                    }
                }
                Some(Cmd::Enqueue(job, priority)) => {
                    if active.as_ref().is_some_and(|a| a.job.file_id == job.file_id) {
                        if let Some(a) = active.as_mut() && priority == Priority::Current {
                            a.priority = Priority::Current;
                        }
                        continue;
                    }
                    if let Some(entry) = queue.iter_mut().find(|(j, _)| j.file_id == job.file_id) {
                        if priority == Priority::Current {
                            entry.1 = Priority::Current;
                        }
                        continue;
                    }
                    // The current item preempts a running prefetch; the prefetch resumes later.
                    if priority == Priority::Current
                        && active.as_ref().is_some_and(|a| a.priority == Priority::Prefetch)
                    {
                        let a = active.take().expect("checked");
                        a.handle.abort();
                        queue.push_back((a.job, Priority::Prefetch));
                    }
                    queue.push_back((job, priority));
                }
            },
        }
    }
}

async fn run_job(
    client: reqwest::Client,
    backoff: Vec<Duration>,
    job: DownloadJob,
    ev_tx: mpsc::UnboundedSender<DownloadEvent>,
) {
    let limit = backoff.len().max(1) as u32;
    let mut attempts = 0;
    loop {
        match download_once(&client, &job, &ev_tx).await {
            Ok(()) => break,
            Err(e) => {
                attempts += 1;
                tracing::warn!("download of {} failed (attempt {attempts}): {e}", job.url);
                if attempts >= limit {
                    let _ = ev_tx.send(DownloadEvent::Failed { file_id: job.file_id, attempts });
                    return;
                }
                tokio::time::sleep(backoff[attempts as usize - 1]).await;
            }
        }
    }
    for (url, path) in &job.subs {
        if let Err(e) = fetch_small(&client, url, path).await {
            tracing::warn!("cannot download subtitle {url}: {e}");
        }
    }
    let _ = ev_tx.send(DownloadEvent::Done { file_id: job.file_id });
}

/// Total size from `Content-Range: bytes a-b/T` (or `bytes */T`).
fn content_range_total(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?
        .rsplit('/')
        .next()?
        .parse()
        .ok()
}

fn content_range_start(resp: &reqwest::Response) -> Option<u64> {
    let v = resp.headers().get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    v.strip_prefix("bytes ")?.split('-').next()?.parse().ok()
}

async fn download_once(
    client: &reqwest::Client,
    job: &DownloadJob,
    ev_tx: &mpsc::UnboundedSender<DownloadEvent>,
) -> Result<(), String> {
    let part = part_path(&job.dest);
    if let Some(dir) = job.dest.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    }
    let have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(&job.url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let (mut bytes, total, append) = match resp.status() {
        StatusCode::PARTIAL_CONTENT if content_range_start(&resp) == Some(have) => {
            let total = content_range_total(&resp).ok_or("206 without Content-Range")?;
            (have, total, true)
        }
        StatusCode::OK => {
            let total = resp.content_length().or(job.total_hint).ok_or("unknown size")?;
            (0, total, false)
        }
        StatusCode::RANGE_NOT_SATISFIABLE if content_range_total(&resp) == Some(have) => {
            // The part file is already complete.
            tokio::fs::rename(&part, &job.dest).await.map_err(|e| e.to_string())?;
            let _ = ev_tx.send(DownloadEvent::Progress {
                file_id: job.file_id.clone(),
                bytes: have,
                total: have,
            });
            return Ok(());
        }
        status => {
            if status == StatusCode::RANGE_NOT_SATISFIABLE || status == StatusCode::PARTIAL_CONTENT
            {
                let _ = tokio::fs::remove_file(&part).await;
            }
            return Err(format!("HTTP {status}"));
        }
    };
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&part)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = resp.bytes_stream();
    let mut last = Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        bytes += chunk.len() as u64;
        if last.elapsed() >= PROGRESS_INTERVAL {
            last = Instant::now();
            let _ =
                ev_tx.send(DownloadEvent::Progress { file_id: job.file_id.clone(), bytes, total });
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    if bytes != total {
        return Err(format!("incomplete download: {bytes} of {total} bytes"));
    }
    tokio::fs::rename(&part, &job.dest).await.map_err(|e| e.to_string())?;
    let _ = ev_tx.send(DownloadEvent::Progress { file_id: job.file_id.clone(), bytes, total });
    Ok(())
}

async fn fetch_small(client: &reqwest::Client, url: &str, path: &Path) -> Result<(), String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    tokio::fs::write(path, &body).await.map_err(|e| e.to_string())
}
