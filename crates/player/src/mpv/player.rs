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

/// Launches mpv processes.
#[derive(Debug, Clone, Default)]
pub struct MpvLauncher(pub MpvConfig);

impl PlayerLauncher for MpvLauncher {
    type P = MpvPlayer;

    async fn launch(&self) -> PlayerResult<MpvPlayer> {
        MpvPlayer::launch(&self.0).await
    }
}

pub struct MpvPlayer {
    ipc: Arc<MpvIpc>,
    events: mpsc::UnboundedReceiver<PlayerEvent>,
    pending_subs: Arc<Mutex<Vec<String>>>,
    kill: Option<oneshot::Sender<()>>,
}

fn source_arg(src: &MediaSource) -> String {
    match src {
        MediaSource::Path(p) => p.display().to_string(),
        MediaSource::Url(u) => u.clone(),
    }
}

fn secs_to_ms(v: &Value) -> Option<u64> {
    v.as_f64().filter(|s| s.is_finite() && *s >= 0.0).map(|s| (s * 1000.0).round() as u64)
}

impl MpvPlayer {
    async fn launch(cfg: &MpvConfig) -> PlayerResult<MpvPlayer> {
        let ipc_path = new_ipc_path();
        let mut child = tokio::process::Command::new(&cfg.path)
            .args(cfg.args(&ipc_path))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| PlayerError::Launch(format!("{}: {e}", cfg.path.display())))?;
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
        Ok(MpvPlayer { ipc, events, pending_subs, kill: Some(kill_tx) })
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
                for sub in subs {
                    if let Err(e) =
                        ipc.command(vec![json!("sub-add"), json!(sub), json!("auto")]).await
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
        *self.pending_subs.lock().expect("subs") = subtitles.iter().map(source_arg).collect();
        self.cmd(vec![json!("loadfile"), json!(source_arg(&media)), json!("replace")]).await
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
