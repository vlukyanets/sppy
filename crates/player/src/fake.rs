//! A deterministic in-memory player for tests (works with paused tokio time).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::{
    Capabilities, MediaSource, Player, PlayerError, PlayerEvent, PlayerLauncher, PlayerResult,
};

#[derive(Debug, Clone)]
pub struct FakeConfig {
    /// Delay before a command takes effect.
    pub cmd_latency_ms: u64,
    /// Time a seek takes (position frozen meanwhile).
    pub seek_delay_ms: u64,
    pub time_pos_interval_ms: u64,
    pub duration_ms: u64,
}

impl Default for FakeConfig {
    fn default() -> Self {
        FakeConfig {
            cmd_latency_ms: 5,
            seek_delay_ms: 100,
            time_pos_interval_ms: 50,
            duration_ms: 600_000,
        }
    }
}

/// Commands the fake received, for assertions.
#[derive(Debug, Clone, PartialEq)]
pub enum FakeCmd {
    Load(MediaSource),
    SubAdd(MediaSource),
    Play,
    Pause,
    Seek(u64),
    SetSpeed(f64),
    Osd(String),
}

struct State {
    cfg: FakeConfig,
    events: Option<mpsc::UnboundedSender<PlayerEvent>>,
    loaded: bool,
    paused: bool,
    speed: f64,
    buffering: bool,
    seeking: bool,
    /// Incremented per seek; only the latest seek reports `Seeked` (like mpv).
    seek_gen: u64,
    eof_sent: bool,
    base_pos: f64,
    base_at: Instant,
    commands: Vec<FakeCmd>,
}

impl State {
    fn advancing(&self) -> bool {
        self.loaded && !self.paused && !self.buffering && !self.seeking && !self.eof_sent
    }

    fn position(&self) -> f64 {
        let pos = if self.advancing() {
            self.base_pos + self.base_at.elapsed().as_secs_f64() * 1000.0 * self.speed
        } else {
            self.base_pos
        };
        pos.min(self.cfg.duration_ms as f64)
    }

    /// Freeze the current position as the new base.
    fn rebase(&mut self) {
        self.base_pos = self.position();
        self.base_at = Instant::now();
    }

    fn emit(&self, ev: PlayerEvent) {
        if let Some(tx) = &self.events {
            let _ = tx.send(ev);
        }
    }

    fn alive(&self) -> bool {
        self.events.is_some()
    }
}

type Shared = Arc<Mutex<State>>;

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, State> {
    s.lock().expect("fake player state")
}

pub struct FakePlayer {
    state: Shared,
    rx: mpsc::UnboundedReceiver<PlayerEvent>,
}

/// Test-side handle: simulates the human in front of the player and inspects state.
#[derive(Clone)]
pub struct FakeRemote {
    state: Shared,
}

impl FakePlayer {
    pub fn new(cfg: FakeConfig) -> (FakePlayer, FakeRemote) {
        let (tx, rx) = mpsc::unbounded_channel();
        let interval = Duration::from_millis(cfg.time_pos_interval_ms.max(1));
        let state = Arc::new(Mutex::new(State {
            cfg,
            events: Some(tx),
            loaded: false,
            paused: true,
            speed: 1.0,
            buffering: false,
            seeking: false,
            seek_gen: 0,
            eof_sent: false,
            base_pos: 0.0,
            base_at: Instant::now(),
            commands: Vec::new(),
        }));
        let ticker = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let mut s = lock(&ticker);
                if !s.alive() {
                    return;
                }
                if !s.loaded {
                    continue;
                }
                let pos = s.position();
                s.emit(PlayerEvent::TimePos(pos.round() as u64));
                if s.advancing() && pos >= s.cfg.duration_ms as f64 {
                    s.rebase();
                    s.eof_sent = true;
                    s.emit(PlayerEvent::Eof);
                }
            }
        });
        (FakePlayer { state: state.clone(), rx }, FakeRemote { state })
    }

    async fn command(&self, cmd: FakeCmd) -> PlayerResult {
        let latency = {
            let mut s = lock(&self.state);
            if !s.alive() {
                return Err(PlayerError::Gone);
            }
            s.commands.push(cmd.clone());
            s.cfg.cmd_latency_ms
        };
        tokio::time::sleep(Duration::from_millis(latency)).await;
        match cmd {
            FakeCmd::Load(_) => {
                let mut s = lock(&self.state);
                s.loaded = true;
                s.eof_sent = false;
                s.base_pos = 0.0;
                s.base_at = Instant::now();
                let duration = s.cfg.duration_ms;
                s.emit(PlayerEvent::FileLoaded {
                    duration_ms: Some(duration),
                    title: Some("fake".into()),
                });
                s.emit(PlayerEvent::CacheAhead { ms: duration, eof: true });
            }
            FakeCmd::Play => set_pause(&self.state, false),
            FakeCmd::Pause => set_pause(&self.state, true),
            FakeCmd::Seek(ms) => seek(&self.state, ms),
            FakeCmd::SetSpeed(x) => set_speed(&self.state, x),
            FakeCmd::SubAdd(_) | FakeCmd::Osd(_) => {}
        }
        Ok(())
    }
}

fn set_pause(state: &Shared, paused: bool) {
    let mut s = lock(state);
    if s.paused != paused {
        s.rebase();
        s.paused = paused;
        s.emit(PlayerEvent::Pause(paused));
    }
}

fn set_speed(state: &Shared, speed: f64) {
    let mut s = lock(state);
    if s.speed != speed {
        s.rebase();
        s.speed = speed;
        s.emit(PlayerEvent::Speed(speed));
    }
}

fn seek(state: &Shared, target: u64) {
    let delay = {
        let mut s = lock(state);
        s.base_pos = target.min(s.cfg.duration_ms) as f64;
        s.base_at = Instant::now();
        s.seeking = true;
        s.eof_sent = false;
        s.seek_gen += 1;
        (s.cfg.seek_delay_ms, s.seek_gen)
    };
    let (delay, generation) = delay;
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        let mut s = lock(&state);
        if s.seek_gen != generation {
            return;
        }
        s.seeking = false;
        s.base_at = Instant::now();
        let position_ms = s.base_pos.round() as u64;
        s.emit(PlayerEvent::Seeked { position_ms });
    });
}

impl Drop for FakePlayer {
    fn drop(&mut self) {
        lock(&self.state).events = None;
    }
}

impl Player for FakePlayer {
    fn capabilities(&self) -> Capabilities {
        Capabilities { speed: true, sub_add: true, osd: true }
    }

    async fn load(&mut self, media: MediaSource, subtitles: Vec<MediaSource>) -> PlayerResult {
        self.command(FakeCmd::Load(media)).await?;
        for sub in subtitles {
            self.command(FakeCmd::SubAdd(sub)).await?;
        }
        Ok(())
    }

    async fn play(&mut self) -> PlayerResult {
        self.command(FakeCmd::Play).await
    }

    async fn pause(&mut self) -> PlayerResult {
        self.command(FakeCmd::Pause).await
    }

    async fn seek(&mut self, position_ms: u64) -> PlayerResult {
        self.command(FakeCmd::Seek(position_ms)).await
    }

    async fn set_speed(&mut self, speed: f64) -> PlayerResult {
        self.command(FakeCmd::SetSpeed(speed)).await
    }

    async fn osd(&mut self, text: &str, _duration_ms: u64) -> PlayerResult {
        self.command(FakeCmd::Osd(text.to_owned())).await
    }

    async fn next_event(&mut self) -> Option<PlayerEvent> {
        self.rx.recv().await
    }
}

impl FakeRemote {
    pub fn user_pause(&self, paused: bool) {
        set_pause(&self.state, paused);
    }

    pub fn user_seek(&self, position_ms: u64) {
        seek(&self.state, position_ms);
    }

    pub fn user_speed(&self, speed: f64) {
        set_speed(&self.state, speed);
    }

    pub fn set_buffering(&self, buffering: bool) {
        let mut s = lock(&self.state);
        if s.buffering != buffering {
            s.rebase();
            s.buffering = buffering;
            s.emit(PlayerEvent::Buffering(buffering));
        }
    }

    pub fn set_cache_ahead(&self, ms: u64, eof: bool) {
        lock(&self.state).emit(PlayerEvent::CacheAhead { ms, eof });
    }

    /// The media cannot be opened.
    pub fn fail_load(&self, message: &str) {
        lock(&self.state).emit(PlayerEvent::LoadFailed { message: message.to_owned() });
    }

    /// The player process exits: emits `Exited`, then the event stream ends.
    pub fn exit(&self, code: Option<i32>) {
        let mut s = lock(&self.state);
        s.emit(PlayerEvent::Exited { code });
        s.events = None;
    }

    pub fn position_ms(&self) -> u64 {
        lock(&self.state).position().round() as u64
    }

    pub fn paused(&self) -> bool {
        lock(&self.state).paused
    }

    pub fn speed(&self) -> f64 {
        lock(&self.state).speed
    }

    pub fn alive(&self) -> bool {
        lock(&self.state).alive()
    }

    pub fn commands(&self) -> Vec<FakeCmd> {
        lock(&self.state).commands.clone()
    }
}

/// Launches a new `FakePlayer` per call and keeps every remote for inspection.
#[derive(Clone, Default)]
pub struct FakeLauncher {
    pub cfg: FakeConfig,
    remotes: Arc<Mutex<Vec<FakeRemote>>>,
}

impl FakeLauncher {
    pub fn new(cfg: FakeConfig) -> Self {
        FakeLauncher { cfg, remotes: Arc::default() }
    }

    pub fn launches(&self) -> usize {
        self.remotes.lock().expect("remotes").len()
    }

    /// Remote of the most recently launched player.
    pub fn remote(&self) -> Option<FakeRemote> {
        self.remotes.lock().expect("remotes").last().cloned()
    }
}

impl PlayerLauncher for FakeLauncher {
    type P = FakePlayer;

    async fn launch(&self) -> PlayerResult<FakePlayer> {
        let (player, remote) = FakePlayer::new(self.cfg.clone());
        self.remotes.lock().expect("remotes").push(remote);
        Ok(player)
    }
}
