//! One window per data directory for invite links: a second start with a link hands it to
//! the running app (Unix socket / Windows named pipe) and exits.

use std::io::Write;
use std::path::Path;

use tokio::io::AsyncReadExt;

/// More than any invite link: a message longer than this is not one.
const MAX_MESSAGE: u64 = 4096;

/// A name for the running instance of the app that keeps its files in `data_dir`.
pub fn endpoint_name(data_dir: &Path) -> String {
    // FNV-1a: stable across builds, unlike the standard hasher.
    let hash = data_dir.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("sppy-app-{:016x}", hash)
}

#[cfg(unix)]
fn socket_path(data_dir: &Path) -> std::path::PathBuf {
    // The runtime dir is private to the user; a path under the data dir could be too long
    // for a socket.
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("{}.sock", endpoint_name(data_dir)))
}

#[cfg(windows)]
fn pipe_name(data_dir: &Path) -> String {
    format!(r"\\.\pipe\{}", endpoint_name(data_dir))
}

/// Hand `link` to the app already running for `data_dir`; false when there is none.
pub fn forward(data_dir: &Path, link: &str) -> bool {
    #[cfg(unix)]
    let conn = std::os::unix::net::UnixStream::connect(socket_path(data_dir));
    #[cfg(windows)]
    let conn = std::fs::OpenOptions::new().write(true).open(pipe_name(data_dir));
    match conn {
        Ok(mut c) => c.write_all(link.as_bytes()).is_ok(),
        Err(_) => false,
    }
}

/// Receive links that later starts forward, and pass each invite link to `on_link`.
/// Does nothing if another app already listens for `data_dir`.
pub fn listen(
    rt: &tokio::runtime::Runtime,
    data_dir: &Path,
    on_link: impl Fn(String) + Send + 'static,
) {
    #[cfg(unix)]
    {
        let path = socket_path(data_dir);
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return;
        }
        // Left behind by an app that did not exit cleanly.
        let _ = std::fs::remove_file(&path);
        let _enter = rt.enter();
        let listener = match tokio::net::UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("cannot listen for invite links: {e}");
                return;
            }
        };
        rt.spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                if let Some(link) = read_link(stream).await {
                    on_link(link);
                }
            }
        });
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ServerOptions;
        let name = pipe_name(data_dir);
        let _enter = rt.enter();
        let mut server = match ServerOptions::new().first_pipe_instance(true).create(&name) {
            Ok(s) => s,
            // Another app has it.
            Err(_) => return,
        };
        rt.spawn(async move {
            loop {
                if server.connect().await.is_err() {
                    return;
                }
                let next = match ServerOptions::new().create(&name) {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let client = std::mem::replace(&mut server, next);
                if let Some(link) = read_link(client).await {
                    on_link(link);
                }
            }
        });
    }
}

async fn read_link(stream: impl tokio::io::AsyncRead + Unpin) -> Option<String> {
    let mut text = String::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.take(MAX_MESSAGE).read_to_string(&mut text),
    )
    .await;
    let link = text.trim().to_owned();
    (matches!(read, Ok(Ok(_))) && crate::links::is_invite_link(&link)).then_some(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_data_dir_has_its_own_name() {
        let a = endpoint_name(Path::new("/home/u/.local/share/sppy"));
        assert_eq!(a, endpoint_name(Path::new("/home/u/.local/share/sppy")));
        assert_ne!(a, endpoint_name(Path::new("/tmp/second")));
        assert!(a.starts_with("sppy-app-") && a.len() == "sppy-app-".len() + 16);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_start_hands_its_link_to_the_running_app() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert!(!forward(dir.path(), "sppy://h/t"), "nobody is listening yet");

        let (tx, rx) = std::sync::mpsc::channel();
        listen(&rt, dir.path(), move |l| tx.send(l).unwrap());
        // A second app for the same directory does not take over.
        let (tx2, rx2) = std::sync::mpsc::channel();
        listen(&rt, dir.path(), move |l| tx2.send(l).unwrap());

        assert!(forward(dir.path(), "sppy://box:8740/abc"));
        let got = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(got, "sppy://box:8740/abc");
        assert!(rx2.try_recv().is_err());

        // Anything that is not an invite link is dropped.
        assert!(forward(dir.path(), "rm -rf /"));
        assert!(forward(dir.path(), "sppy://box:8740/second"));
        let got = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(got, "sppy://box:8740/second");
        let _ = std::fs::remove_file(socket_path(dir.path()));
    }
}
