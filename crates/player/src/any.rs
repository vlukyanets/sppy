//! The player the user picked in the settings, chosen at run time.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::mpv::{MpvConfig, MpvLauncher, MpvPlayer};
use crate::vlc::{VlcConfig, VlcLauncher, VlcPlayer};
use crate::{
    Capabilities, MediaSource, Player, PlayerEvent, PlayerLauncher, PlayerResult, TrackLanguages,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayerKind {
    #[default]
    Mpv,
    Vlc,
}

impl PlayerKind {
    pub const ALL: [PlayerKind; 2] = [PlayerKind::Mpv, PlayerKind::Vlc];

    /// The name in the config file.
    pub fn id(self) -> &'static str {
        match self {
            PlayerKind::Mpv => "mpv",
            PlayerKind::Vlc => "vlc",
        }
    }

    pub fn from_id(id: &str) -> Option<PlayerKind> {
        PlayerKind::ALL.into_iter().find(|k| k.id().eq_ignore_ascii_case(id.trim()))
    }

    /// The name people know it by.
    pub fn name(self) -> &'static str {
        match self {
            PlayerKind::Mpv => "mpv",
            PlayerKind::Vlc => "VLC",
        }
    }

    /// The program to start when no path is set.
    pub fn default_path(self) -> PathBuf {
        PathBuf::from(self.id())
    }
}

/// Which player to start and how.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerSetup {
    pub kind: PlayerKind,
    pub path: PathBuf,
    pub extra_args: Vec<String>,
}

pub enum AnyPlayer {
    Mpv(MpvPlayer),
    Vlc(VlcPlayer),
}

impl Player for AnyPlayer {
    fn capabilities(&self) -> Capabilities {
        match self {
            AnyPlayer::Mpv(p) => p.capabilities(),
            AnyPlayer::Vlc(p) => p.capabilities(),
        }
    }

    async fn load(&mut self, media: MediaSource, subtitles: Vec<MediaSource>) -> PlayerResult {
        match self {
            AnyPlayer::Mpv(p) => p.load(media, subtitles).await,
            AnyPlayer::Vlc(p) => p.load(media, subtitles).await,
        }
    }

    async fn set_track_languages(&mut self, languages: &TrackLanguages) -> PlayerResult {
        match self {
            AnyPlayer::Mpv(p) => p.set_track_languages(languages).await,
            AnyPlayer::Vlc(p) => p.set_track_languages(languages).await,
        }
    }

    async fn play(&mut self) -> PlayerResult {
        match self {
            AnyPlayer::Mpv(p) => p.play().await,
            AnyPlayer::Vlc(p) => p.play().await,
        }
    }

    async fn pause(&mut self) -> PlayerResult {
        match self {
            AnyPlayer::Mpv(p) => p.pause().await,
            AnyPlayer::Vlc(p) => p.pause().await,
        }
    }

    async fn seek(&mut self, position_ms: u64) -> PlayerResult {
        match self {
            AnyPlayer::Mpv(p) => p.seek(position_ms).await,
            AnyPlayer::Vlc(p) => p.seek(position_ms).await,
        }
    }

    async fn set_speed(&mut self, speed: f64) -> PlayerResult {
        match self {
            AnyPlayer::Mpv(p) => p.set_speed(speed).await,
            AnyPlayer::Vlc(p) => p.set_speed(speed).await,
        }
    }

    async fn osd(&mut self, text: &str, duration_ms: u64) -> PlayerResult {
        match self {
            AnyPlayer::Mpv(p) => p.osd(text, duration_ms).await,
            AnyPlayer::Vlc(p) => p.osd(text, duration_ms).await,
        }
    }

    async fn next_event(&mut self) -> Option<PlayerEvent> {
        match self {
            AnyPlayer::Mpv(p) => p.next_event().await,
            AnyPlayer::Vlc(p) => p.next_event().await,
        }
    }
}

/// Starts the configured player. Clones share the setup, so the app can switch players or
/// change the path for the next launch (see [`AnyLauncher::set_setup`]).
#[derive(Debug, Clone, Default)]
pub struct AnyLauncher {
    setup: Arc<Mutex<PlayerSetup>>,
}

impl AnyLauncher {
    pub fn new(setup: PlayerSetup) -> Self {
        AnyLauncher { setup: Arc::new(Mutex::new(setup)) }
    }

    pub fn set_setup(&self, setup: PlayerSetup) {
        *self.setup.lock().expect("player setup") = setup;
    }
}

impl PlayerLauncher for AnyLauncher {
    type P = AnyPlayer;

    async fn launch(&self) -> PlayerResult<AnyPlayer> {
        let PlayerSetup { kind, path, extra_args } =
            self.setup.lock().expect("player setup").clone();
        match kind {
            PlayerKind::Mpv => {
                let cfg = MpvConfig { path, extra_args, headless: false };
                MpvLauncher::new(cfg).launch().await.map(AnyPlayer::Mpv)
            }
            PlayerKind::Vlc => {
                let cfg = VlcConfig { path, extra_args, ..VlcConfig::default() };
                VlcLauncher::new(cfg).launch().await.map(AnyPlayer::Vlc)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_config_names() {
        for k in PlayerKind::ALL {
            assert_eq!(PlayerKind::from_id(k.id()), Some(k));
        }
        assert_eq!(PlayerKind::from_id(" VLC "), Some(PlayerKind::Vlc));
        assert_eq!(PlayerKind::from_id("mplayer"), None);
    }
}
