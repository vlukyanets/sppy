//! Player abstraction and backends (mpv). The only crate that knows concrete players.

pub mod mpv;

#[cfg(feature = "fake")]
pub mod fake;

use std::future::Future;
use std::path::PathBuf;

/// What to open: a local file or a URL (server media URL or any URL mpv/yt-dlp can open).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSource {
    Path(PathBuf),
    Url(String),
}

/// Optional features a backend supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub speed: bool,
    pub sub_add: bool,
    pub osd: bool,
}

/// Raw events from the player. Classifying them as "ours" vs "user" happens in client-core.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    TimePos(u64),
    Pause(bool),
    Seeked {
        position_ms: u64,
    },
    Speed(f64),
    FileLoaded {
        duration_ms: Option<u64>,
        title: Option<String>,
    },
    Buffering(bool),
    CacheAhead {
        ms: u64,
        eof: bool,
    },
    Eof,
    /// The player could not open the media (e.g. yt-dlp failure, missing file).
    LoadFailed {
        message: String,
    },
    Exited {
        code: Option<i32>,
    },
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum PlayerError {
    #[error("cannot launch player: {0}")]
    Launch(String),
    /// The player program does not exist (not installed or wrong path).
    #[error("player not found: {0}")]
    NotInstalled(String),
    #[error("player IPC error: {0}")]
    Ipc(String),
    #[error("player is gone")]
    Gone,
}

pub type PlayerResult<T = ()> = Result<T, PlayerError>;

/// A running player instance controlled by sppy.
pub trait Player: Send + 'static {
    fn capabilities(&self) -> Capabilities;
    fn load(
        &mut self,
        media: MediaSource,
        subtitles: Vec<MediaSource>,
    ) -> impl Future<Output = PlayerResult> + Send;
    fn play(&mut self) -> impl Future<Output = PlayerResult> + Send;
    fn pause(&mut self) -> impl Future<Output = PlayerResult> + Send;
    /// Exact seek to an absolute position.
    fn seek(&mut self, position_ms: u64) -> impl Future<Output = PlayerResult> + Send;
    fn set_speed(&mut self, speed: f64) -> impl Future<Output = PlayerResult> + Send;
    fn osd(&mut self, text: &str, duration_ms: u64) -> impl Future<Output = PlayerResult> + Send;
    /// Next raw event; `None` once the player is gone.
    fn next_event(&mut self) -> impl Future<Output = Option<PlayerEvent>> + Send;
}

/// Starts player instances (used for the initial launch and for restarts).
pub trait PlayerLauncher: Send + Sync + 'static {
    type P: Player;
    fn launch(&self) -> impl Future<Output = PlayerResult<Self::P>> + Send;
}
