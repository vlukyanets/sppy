//! `Player` implementation for mpv (spec §8.2).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use super::ipc::{IpcPath, MpvIpc, new_ipc_path};
use super::wire::MpvIncoming;
use crate::{
    Capabilities, MediaSource, Player, PlayerError, PlayerEvent, PlayerLauncher, PlayerResult,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const OBSERVED: &[&str] = &[
    "time-pos",
    "pause",
    "speed",
    "eof-reached",
    "paused-for-cache",
    "demuxer-cache-duration",
    "demuxer-cache-state",
];

#[derive(Debug, Clone)]
pub struct MpvConfig {
    pub path: PathBuf,
    pub extra_args: Vec<String>,
    /// No window, no audio, no user config (tests).
    pub headless: bool,
}

impl Default for MpvConfig {
    fn default() -> Self {
        MpvConfig { path: PathBuf::from("mpv"), extra_args: Vec::new(), headless: false }
    }
}

impl MpvConfig {
    fn args(&self, ipc: &IpcPath) -> Vec<String> {
        let mut args: Vec<String> = [
            "--idle=yes",
            "--keep-open=yes",
            // Without this mpv pauses at the end, which would look like a user pause.
            "--keep-open-pause=no",
            "--pause",
            "--hr-seek=yes",
            "--audio-pitch-correction=yes",
            "--title=sppy",
            "--no-terminal",
        ]
        .map(String::from)
        .to_vec();
        args.push(format!("--input-ipc-server={}", ipc.as_arg()));
        if self.headless {
            args.extend(["--vo=null", "--ao=null", "--no-config"].map(String::from));
        } else {
            args.push("--force-window=yes".into());
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// Launches mpv processes. Clones share the configuration, so the app can change the mpv path
/// or arguments for the next launch (see [`MpvLauncher::set_config`]).
#[derive(Debug, Clone, Default)]
pub struct MpvLauncher {
    cfg: Arc<Mutex<MpvConfig>>,
}

impl MpvLauncher {
    pub fn new(cfg: MpvConfig) -> Self {
        MpvLauncher { cfg: Arc::new(Mutex::new(cfg)) }
    }

    pub fn set_config(&self, cfg: MpvConfig) {
        *self.cfg.lock().expect("mpv config") = cfg;
    }
}

impl PlayerLauncher for MpvLauncher {
    type P = MpvPlayer;

    async fn launch(&self) -> PlayerResult<MpvPlayer> {
        let cfg = self.cfg.lock().expect("mpv config").clone();
        MpvPlayer::launch(&cfg).await
    }
}

pub struct MpvPlayer {
    ipc: Arc<MpvIpc>,
    events: mpsc::UnboundedReceiver<PlayerEvent>,
    pending_subs: Arc<Mutex<Vec<String>>>,
    kill: Option<oneshot::Sender<()>>,
    /// Temporary UTF-8 symlinks for non-UTF-8 paths of the current item.
    links: Vec<PathBuf>,
    ipc_path: IpcPath,
}

/// mpv's JSON IPC only carries UTF-8. A path that is not valid UTF-8 is opened through a
/// UTF-8-named symlink next to the IPC sockets (Unix); the link is kept until the next load.
fn source_arg(src: &MediaSource, links: &mut Vec<PathBuf>) -> PlayerResult<String> {
    match src {
        MediaSource::Url(u) => Ok(u.clone()),
        MediaSource::Path(p) => match p.to_str() {
            Some(s) => Ok(s.to_owned()),
            None => utf8_alias(p, links),
        },
    }
}

#[cfg(unix)]
fn utf8_alias(path: &std::path::Path, links: &mut Vec<PathBuf>) -> PlayerResult<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("sppy-media-{}-{n}.{ext}", std::process::id());
    let link = super::ipc::socket_dir().join(name);
    std::os::unix::fs::symlink(path, &link)
        .map_err(|e| PlayerError::Ipc(format!("cannot alias {}: {e}", path.display())))?;
    links.push(link.clone());
    Ok(link.to_string_lossy().into_owned())
}

#[cfg(not(unix))]
fn utf8_alias(path: &std::path::Path, _links: &mut Vec<PathBuf>) -> PlayerResult<String> {
    Err(PlayerError::Ipc(format!("path is not valid Unicode: {}", path.display())))
}

/// mpv's `alang` / `slang`: the languages as given plus their other ISO 639 form, since
/// files name languages either way (`en` / `eng`) and older mpv versions match them
/// literally.
fn mpv_languages(languages: &[String]) -> String {
    const PAIRS: &[(&str, &[&str])] = &[
        ("en", &["eng"]),
        ("ru", &["rus"]),
        ("uk", &["ukr"]),
        ("de", &["deu", "ger"]),
        ("fr", &["fra", "fre"]),
        ("es", &["spa"]),
        ("it", &["ita"]),
        ("pt", &["por"]),
        ("pl", &["pol"]),
        ("ja", &["jpn"]),
        ("ko", &["kor"]),
        ("zh", &["zho", "chi"]),
    ];
    let mut out: Vec<String> = Vec::new();
    let mut push = |code: &str| {
        if !code.is_empty() && !out.iter().any(|c| c == code) {
            out.push(code.to_owned());
        }
    };
    for lang in languages {
        let lang = lang.trim().to_ascii_lowercase();
        push(&lang);
        for (two, three) in PAIRS {
            if lang == *two {
                three.iter().for_each(|c| push(c));
            } else if three.contains(&lang.as_str()) {
                push(two);
                three.iter().for_each(|c| push(c));
            }
        }
    }
    out.join(",")
}

fn remove_links(links: &mut Vec<PathBuf>) {
    for l in links.drain(..) {
        let _ = std::fs::remove_file(l);
    }
}

fn secs_to_ms(v: &Value) -> Option<u64> {
    v.as_f64().filter(|s| s.is_finite() && *s >= 0.0).map(|s| (s * 1000.0).round() as u64)
}

impl MpvPlayer {
    async fn launch(cfg: &MpvConfig) -> PlayerResult<MpvPlayer> {
        #[cfg(unix)]
        {
            super::ipc::cleanup_stale_sockets(&super::ipc::socket_dir());
            super::ipc::cleanup_stale_sockets(&std::env::temp_dir());
        }
        let ipc_path = new_ipc_path();
        let mut child = tokio::process::Command::new(&cfg.path)
            .args(cfg.args(&ipc_path))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    PlayerError::NotInstalled(cfg.path.display().to_string())
                } else {
                    PlayerError::Launch(format!("{}: {e}", cfg.path.display()))
                }
            })?;
        let (ipc, raw_events) = match MpvIpc::connect(&ipc_path, CONNECT_TIMEOUT).await {
            Ok(x) => x,
            Err(e) => {
                let _ = child.kill().await;
                return Err(PlayerError::Launch(e.to_string()));
            }
        };
        let ipc = Arc::new(ipc);
        for (id, name) in OBSERVED.iter().enumerate() {
            ipc.command(vec![json!("observe_property"), json!(id + 1), json!(name)]).await?;
        }

        let (tx, events) = mpsc::unbounded_channel();
        let pending_subs: Arc<Mutex<Vec<String>>> = Arc::default();
        tokio::spawn(pump(ipc.clone(), raw_events, tx.clone(), pending_subs.clone()));

        let path_for_player = ipc_path.clone();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let code = tokio::select! {
                status = child.wait() => status.ok().and_then(|s| s.code()),
                _ = kill_rx => {
                    let _ = child.kill().await;
                    None
                }
            };
            let _ = tx.send(PlayerEvent::Exited { code });
            #[cfg(unix)]
            {
                let IpcPath::Socket(p) = &ipc_path;
                let _ = std::fs::remove_file(p);
            }
        });
        Ok(MpvPlayer {
            ipc,
            events,
            pending_subs,
            kill: Some(kill_tx),
            links: Vec::new(),
            ipc_path: path_for_player,
        })
    }

    /// Where this player's IPC socket / pipe lives.
    pub fn ipc_path(&self) -> &IpcPath {
        &self.ipc_path
    }

    /// Read an mpv property (diagnostics and tests).
    pub async fn get_property(&self, name: &str) -> PlayerResult<Value> {
        self.ipc.command(vec![json!("get_property"), json!(name)]).await
    }

    /// Ask mpv to quit (exit code 0).
    pub async fn quit(&self) {
        let _ = self.ipc.command(vec![json!("quit"), json!(0)]).await;
    }

    async fn cmd(&self, args: Vec<Value>) -> PlayerResult {
        self.ipc.command(args).await.map(|_| ())
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) {
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
        remove_links(&mut self.links);
        // A killed mpv cannot remove its own socket.
        #[cfg(unix)]
        {
            let IpcPath::Socket(p) = &self.ipc_path;
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Translate raw mpv events into `PlayerEvent`s.
async fn pump(
    ipc: Arc<MpvIpc>,
    mut raw: mpsc::UnboundedReceiver<MpvIncoming>,
    tx: mpsc::UnboundedSender<PlayerEvent>,
    pending_subs: Arc<Mutex<Vec<String>>>,
) {
    let mut seeking = false;
    let mut cache_ms = 0u64;
    let mut cache_eof = false;
    while let Some(msg) = raw.recv().await {
        let MpvIncoming::Event { event, name, data, reason } = msg else { continue };
        let out = match event.as_str() {
            "property-change" => match (name.as_deref().unwrap_or(""), data.unwrap_or(Value::Null))
            {
                ("time-pos", v) => secs_to_ms(&v).map(PlayerEvent::TimePos),
                ("pause", Value::Bool(b)) => Some(PlayerEvent::Pause(b)),
                ("speed", v) => v.as_f64().map(PlayerEvent::Speed),
                ("eof-reached", Value::Bool(true)) => Some(PlayerEvent::Eof),
                ("paused-for-cache", Value::Bool(b)) => Some(PlayerEvent::Buffering(b)),
                ("demuxer-cache-duration", v) => {
                    cache_ms = secs_to_ms(&v).unwrap_or(0);
                    Some(PlayerEvent::CacheAhead { ms: cache_ms, eof: cache_eof })
                }
                ("demuxer-cache-state", v) => {
                    let eof = v.get("eof").and_then(Value::as_bool).unwrap_or(false);
                    if eof != cache_eof {
                        cache_eof = eof;
                        Some(PlayerEvent::CacheAhead { ms: cache_ms, eof })
                    } else {
                        None
                    }
                }
                _ => None,
            },
            "seek" => {
                seeking = true;
                None
            }
            "playback-restart" if seeking => {
                seeking = false;
                let pos = ipc.command(vec![json!("get_property"), json!("time-pos")]).await;
                pos.ok()
                    .and_then(|v| secs_to_ms(&v))
                    .map(|position_ms| PlayerEvent::Seeked { position_ms })
            }
            "file-loaded" => {
                let duration = ipc.command(vec![json!("get_property"), json!("duration")]).await;
                let title = ipc.command(vec![json!("get_property"), json!("media-title")]).await;
                let subs = std::mem::take(&mut *pending_subs.lock().expect("subs"));
                // The first subtitle (the caller orders them by preference) is shown right away;
                // the others are only added as tracks.
                for (i, sub) in subs.into_iter().enumerate() {
                    let flag = if i == 0 { "select" } else { "auto" };
                    if let Err(e) =
                        ipc.command(vec![json!("sub-add"), json!(sub), json!(flag)]).await
                    {
                        tracing::warn!("cannot add subtitle {sub}: {e}");
                    }
                }
                Some(PlayerEvent::FileLoaded {
                    duration_ms: duration.ok().and_then(|v| secs_to_ms(&v)),
                    title: title.ok().and_then(|v| v.as_str().map(str::to_owned)),
                })
            }
            "end-file" if reason.as_deref() == Some("error") => {
                Some(PlayerEvent::LoadFailed { message: "mpv could not open the media".into() })
            }
            _ => None,
        };
        if let Some(ev) = out
            && tx.send(ev).is_err()
        {
            return;
        }
    }
}

impl Player for MpvPlayer {
    fn capabilities(&self) -> Capabilities {
        Capabilities { speed: true, sub_add: true, osd: true }
    }

    async fn load(&mut self, media: MediaSource, subtitles: Vec<MediaSource>) -> PlayerResult {
        remove_links(&mut self.links);
        let mut links = Vec::new();
        let subs = subtitles
            .iter()
            .map(|s| source_arg(s, &mut links))
            .collect::<PlayerResult<Vec<_>>>()?;
        let src = source_arg(&media, &mut links)?;
        self.links = links;
        *self.pending_subs.lock().expect("subs") = subs;
        self.cmd(vec![json!("loadfile"), json!(src), json!("replace")]).await
    }

    async fn set_track_languages(&mut self, languages: &crate::TrackLanguages) -> PlayerResult {
        let alang = mpv_languages(&languages.audio);
        self.cmd(vec![json!("set_property"), json!("alang"), json!(alang)]).await?;
        let slang = mpv_languages(&languages.subtitles);
        self.cmd(vec![json!("set_property"), json!("slang"), json!(slang)]).await
    }

    async fn play(&mut self) -> PlayerResult {
        self.cmd(vec![json!("set_property"), json!("pause"), json!(false)]).await
    }

    async fn pause(&mut self) -> PlayerResult {
        self.cmd(vec![json!("set_property"), json!("pause"), json!(true)]).await
    }

    async fn seek(&mut self, position_ms: u64) -> PlayerResult {
        let secs = position_ms as f64 / 1000.0;
        self.cmd(vec![json!("seek"), json!(secs), json!("absolute+exact")]).await
    }

    async fn set_speed(&mut self, speed: f64) -> PlayerResult {
        self.cmd(vec![json!("set_property"), json!("speed"), json!(speed)]).await
    }

    async fn osd(&mut self, text: &str, duration_ms: u64) -> PlayerResult {
        self.cmd(vec![json!("show-text"), json!(text), json!(duration_ms)]).await
    }

    async fn next_event(&mut self) -> Option<PlayerEvent> {
        self.events.recv().await
    }
}

#[cfg(test)]
mod language_tests {
    use super::mpv_languages as slang;

    #[test]
    fn languages_get_both_iso_forms_in_order() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(slang(&l(&["ru", "en"])), "ru,rus,en,eng");
        assert_eq!(slang(&l(&["ENG", " ger "])), "eng,en,ger,de,deu");
        assert_eq!(slang(&l(&["en", "eng", "xx"])), "en,eng,xx");
        assert_eq!(slang(&[]), "");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use super::*;

    /// The link shows where the file is, so it goes to the per-user runtime dir (where the
    /// IPC sockets are), not to the temp dir every local user can list.
    #[test]
    fn non_utf8_path_is_aliased_in_the_private_dir() {
        let dir = tempfile::tempdir().unwrap();
        let name = std::ffi::OsStr::from_bytes(b"caf\xe9.mkv");
        let file = dir.path().join(name);
        std::fs::write(&file, b"x").unwrap();
        let mut links = Vec::new();
        let arg = source_arg(&MediaSource::Path(file.clone()), &mut links).unwrap();
        let link = PathBuf::from(&arg);
        assert_eq!(link.parent(), Some(super::super::ipc::socket_dir().as_path()));
        assert_eq!(std::fs::read_link(&link).unwrap(), file);
        assert!(arg.ends_with(".mkv"));
        remove_links(&mut links);
        assert!(!link.exists());
    }
}
