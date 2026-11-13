//! mpv JSON IPC connection: request/response matching by `request_id` plus an event stream.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::wire::{MpvIncoming, command_line, parse_line};
use crate::PlayerError;

/// Where mpv listens (`--input-ipc-server`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcPath {
    #[cfg(unix)]
    Socket(PathBuf),
    #[cfg(windows)]
    Pipe(String),
}

impl IpcPath {
    /// Value for mpv's `--input-ipc-server=`.
    pub fn as_arg(&self) -> String {
        match self {
            #[cfg(unix)]
            IpcPath::Socket(p) => p.display().to_string(),
            #[cfg(windows)]
            IpcPath::Pipe(name) => name.clone(),
        }
    }
}

/// A fresh, unique IPC path for one mpv instance.
pub fn new_ipc_path() -> IpcPath {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let unique = format!(
        "sppy-{}-{}-{:x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        nanos
    );
    #[cfg(unix)]
    {
        IpcPath::Socket(std::env::temp_dir().join(format!("{unique}.sock")))
    }
    #[cfg(windows)]
    {
        IpcPath::Pipe(format!(r"\\.\pipe\{unique}"))
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, PlayerError>>>>>;
type Writer = Box<dyn AsyncWrite + Send + Unpin>;

pub struct MpvIpc {
    writer: tokio::sync::Mutex<Writer>,
    pending: Pending,
    next_id: AtomicU64,
}

impl MpvIpc {
    /// Connect, retrying every 50 ms until `timeout` (mpv creates the socket asynchronously).
    pub async fn connect(
        path: &IpcPath,
        timeout: Duration,
    ) -> Result<(MpvIpc, mpsc::UnboundedReceiver<MpvIncoming>), PlayerError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match open(path).await {
                Ok((reader, writer)) => return Ok(Self::start(reader, writer)),
                Err(e) if tokio::time::Instant::now() >= deadline => {
                    return Err(PlayerError::Ipc(format!("cannot connect to mpv IPC: {e}")));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    fn start(
        reader: Box<dyn AsyncRead + Send + Unpin>,
        writer: Writer,
    ) -> (MpvIpc, mpsc::UnboundedReceiver<MpvIncoming>) {
        let pending: Pending = Arc::default();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                // mpv may pass through non-UTF-8 bytes (e.g. odd tags): never drop the link for that.
                let line = String::from_utf8_lossy(&buf);
                match parse_line(line.trim_end()) {
                    Ok(MpvIncoming::Reply { request_id, error, data }) => {
                        let tx = reader_pending.lock().expect("pending").remove(&request_id);
                        if let Some(tx) = tx {
                            let result = if error == "success" {
                                Ok(data)
                            } else {
                                Err(PlayerError::Ipc(error))
                            };
                            let _ = tx.send(result);
                        }
                    }
                    Ok(event) => {
                        let _ = events_tx.send(event);
                    }
                    Err(e) => tracing::debug!("ignoring mpv line: {e}"),
                }
            }
            // Connection closed: fail every waiting command.
            reader_pending.lock().expect("pending").clear();
        });
        (
            MpvIpc { writer: tokio::sync::Mutex::new(writer), pending, next_id: AtomicU64::new(1) },
            events_rx,
        )
    }

    /// Send a command and wait for its reply.
    pub async fn command(&self, args: Vec<Value>) -> Result<Value, PlayerError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending").insert(id, tx);
        let line = command_line(id, &args);
        {
            let mut w = self.writer.lock().await;
            if let Err(e) = w.write_all(line.as_bytes()).await {
                self.pending.lock().expect("pending").remove(&id);
                return Err(PlayerError::Ipc(e.to_string()));
            }
        }
        rx.await.unwrap_or(Err(PlayerError::Gone))
    }
}

type Halves = (Box<dyn AsyncRead + Send + Unpin>, Writer);

#[cfg(unix)]
async fn open(path: &IpcPath) -> std::io::Result<Halves> {
    let IpcPath::Socket(p) = path;
    let stream = tokio::net::UnixStream::connect(p).await?;
    let (r, w) = stream.into_split();
    Ok((Box::new(r), Box::new(w)))
}

#[cfg(windows)]
async fn open(path: &IpcPath) -> std::io::Result<Halves> {
    let IpcPath::Pipe(name) = path;
    let client = tokio::net::windows::named_pipe::ClientOptions::new().open(name)?;
    let (r, w) = tokio::io::split(client);
    Ok((Box::new(r), Box::new(w)))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::*;

    #[tokio::test]
    async fn invalid_utf8_line_does_not_kill_the_connection() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(ours);
        let (ipc, mut events) = MpvIpc::start(Box::new(r), Box::new(w));
        let (their_r, mut their_w) = tokio::io::split(theirs);
        tokio::spawn(async move {
            // An event whose title is not valid UTF-8, then the reply to the command.
            their_w
                .write_all(b"{\"event\":\"x\",\"name\":\"media-title\",\"data\":\"\xff\xfe\"}\n")
                .await
                .unwrap();
            let mut lines = BufReader::new(their_r).lines();
            let _cmd = lines.next_line().await.unwrap();
            their_w
                .write_all(b"{\"request_id\":1,\"error\":\"success\",\"data\":42}\n")
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let v = ipc
            .command(vec![serde_json::json!("get_property"), serde_json::json!("x")])
            .await
            .unwrap();
        assert_eq!(v, serde_json::json!(42));
        assert!(events.recv().await.is_some(), "the lossy event is still delivered");
    }
}
