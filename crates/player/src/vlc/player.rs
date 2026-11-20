//! `Player` implementation for VLC 3 (spec §8.3).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::tracker::{Action, Tracker};
use super::wire::{self, Command, MODULE_VERSION, WireError};
use super::ytdlp;
use crate::{
    Capabilities, MediaSource, Player, PlayerError, PlayerEvent, PlayerLauncher, PlayerResult,
    TrackLanguages,
};

/// The Lua interface module VLC runs.
pub const MODULE: &str = include_str!("sppy.lua");
const MODULE_FILE: &str = "sppy.lua";

/// How long VLC may take to start and connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the module may take to answer one command.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the status is polled: the module cannot speak first.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const OSD_US_PER_MS: u64 = 1000;
/// How long yt-dlp may take to find the media of a link.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct VlcConfig {
    pub path: PathBuf,
    pub extra_args: Vec<String>,
    /// No window, no audio (tests).
    pub headless: bool,
    /// Unix: run VLC with its data, config and cache under this folder and put the module
    /// there instead of the user's own VLC folders (tests).
    pub isolated_home: Option<PathBuf>,
    /// yt-dlp, which finds the media of links; without it VLC tries its own scripts.
    pub ytdlp: PathBuf,
}

impl Default for VlcConfig {
    fn default() -> Self {
        VlcConfig {
            path: PathBuf::from("vlc"),
            extra_args: Vec::new(),
            headless: false,
            isolated_home: None,
            ytdlp: PathBuf::from("yt-dlp"),
        }
    }
}

/// VLC's per-user Lua interface folder, where `--lua-intf` looks for modules (it does not
/// take a path). `env` reads an environment variable.
pub fn user_lua_intf_dir(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let data = if cfg!(windows) {
        absolute(env("APPDATA"))?.join("vlc")
    } else if cfg!(target_os = "macos") {
        absolute(env("HOME"))?.join("Library/Application Support/org.videolan.vlc")
    } else {
        match absolute(env("XDG_DATA_HOME")) {
            Some(d) => d.join("vlc"),
            None => absolute(env("HOME"))?.join(".local/share/vlc"),
        }
    };
    Some(data.join("lua").join("intf"))
}

/// Put the module into `dir` unless the same version is already there.
pub fn install_module(dir: &Path) -> std::io::Result<PathBuf> {
    let file = dir.join(MODULE_FILE);
    if std::fs::read(&file).is_ok_and(|old| old == MODULE.as_bytes()) {
        return Ok(file);
    }
    std::fs::create_dir_all(dir)?;
    // A VLC starting right now must not read half a file.
    let tmp = dir.join(format!(".{MODULE_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, MODULE)?;
    std::fs::rename(&tmp, &file)?;
    Ok(file)
}

/// The VLC program to start: a bare `vlc` on Windows is looked up where the installer puts
/// it, since the installer does not add VLC to PATH.
fn resolve_program(path: &Path) -> PathBuf {
    if cfg!(windows) && path.parent().is_none_or(|p| p.as_os_str().is_empty()) {
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(var) {
                let exe = PathBuf::from(dir).join("VideoLAN").join("VLC").join("vlc.exe");
                if exe.is_file() {
                    return exe;
                }
            }
        }
    }
    path.to_owned()
}

/// A secret the module repeats so that another local program cannot pose as it.
fn new_secret() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    // Every RandomState gets fresh keys seeded from the OS.
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

impl VlcConfig {
    fn args(&self, port: u16, secret: &str) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if self.headless {
            args.extend(["-I", "dummy", "--vout=dummy", "--aout=dummy"].map(String::from));
        } else {
            args.extend(["--qt-continue=0", "--no-qt-privacy-ask"].map(String::from));
        }
        args.extend(
            [
                "--extraintf=luaintf",
                "--lua-intf=sppy",
                "--no-one-instance",
                "--no-one-instance-when-started-from-file",
                // The end of an item must stop it: a pause there looks like the user's.
                "--no-play-and-pause",
                "--no-play-and-exit",
                "--no-loop",
                "--no-repeat",
                "--no-random",
                "--no-video-title-show",
            ]
            .map(String::from),
        );
        args.push(format!("--lua-config=sppy={{port='{port}',secret='{secret}'}}"));
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// Launches VLC processes. Clones share the configuration (see [`VlcLauncher::set_config`]).
#[derive(Debug, Clone, Default)]
pub struct VlcLauncher {
    cfg: Arc<Mutex<VlcConfig>>,
}

impl VlcLauncher {
    pub fn new(cfg: VlcConfig) -> Self {
        VlcLauncher { cfg: Arc::new(Mutex::new(cfg)) }
    }

    pub fn set_config(&self, cfg: VlcConfig) {
        *self.cfg.lock().expect("vlc config") = cfg;
    }
}

impl PlayerLauncher for VlcLauncher {
    type P = VlcPlayer;

    async fn launch(&self) -> PlayerResult<VlcPlayer> {
        let cfg = self.cfg.lock().expect("vlc config").clone();
        VlcPlayer::launch(&cfg).await
    }
}

#[derive(Debug)]
enum Op {
    Load { media: Target, subtitles: Vec<String> },
    Languages(TrackLanguages),
    Seek(u64),
    Play,
    Pause,
    Speed(f64),
    Osd { text: String, duration_ms: u64 },
}

#[derive(Debug, Clone)]
enum Target {
    /// Opened as it is.
    Uri(String),
    /// Resolved with yt-dlp first.
    Link(String),
}

type Request = (Op, oneshot::Sender<PlayerResult>);

pub struct VlcPlayer {
    requests: mpsc::UnboundedSender<Request>,
    events: mpsc::UnboundedReceiver<PlayerEvent>,
    kill: Option<oneshot::Sender<()>>,
}

fn source_uri(src: &MediaSource) -> String {
    match src {
        MediaSource::Url(u) | MediaSource::Link(u) => u.clone(),
        MediaSource::Path(p) => wire::file_uri(p),
    }
}

fn ipc_error(e: impl std::fmt::Display) -> PlayerError {
    PlayerError::Ipc(e.to_string())
}

impl From<WireError> for PlayerError {
    fn from(e: WireError) -> Self {
        PlayerError::Ipc(e.to_string())
    }
}

/// Accept the module's connection: the first one that knows the secret.
async fn accept_module(
    listener: &TcpListener,
    secret: &str,
) -> PlayerResult<(BufReader<OwnedReadHalf>, OwnedWriteHalf)> {
    loop {
        let (stream, _) = listener.accept().await.map_err(ipc_error)?;
        let _ = stream.set_nodelay(true);
        let (read, write) = stream.into_split();
        let mut read = BufReader::new(read);
        let mut line = String::new();
        let hello = tokio::time::timeout(REPLY_TIMEOUT, read.read_line(&mut line)).await;
        if !matches!(hello, Ok(Ok(n)) if n > 0) {
            continue;
        }
        match wire::parse_hello(line.trim_end()) {
            Some((s, version)) if s == secret => {
                if version != MODULE_VERSION {
                    return Err(PlayerError::Launch(format!(
                        "VLC runs another version of the sppy module ({version}, need \
                         {MODULE_VERSION})"
                    )));
                }
                return Ok((read, write));
            }
            _ => tracing::warn!("a local connection did not know the VLC module's secret"),
        }
    }
}

impl VlcPlayer {
    async fn launch(cfg: &VlcConfig) -> PlayerResult<VlcPlayer> {
        let lua_dir = match &cfg.isolated_home {
            Some(home) => home.join("data").join("vlc").join("lua").join("intf"),
            None => user_lua_intf_dir(|k| std::env::var_os(k)).ok_or_else(|| {
                PlayerError::Launch("cannot find VLC's folder for Lua modules".into())
            })?,
        };
        install_module(&lua_dir).map_err(|e| {
            PlayerError::Launch(format!(
                "cannot put the sppy module into {}: {e}",
                lua_dir.display()
            ))
        })?;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(ipc_error)?;
        let port = listener.local_addr().map_err(ipc_error)?.port();
        let secret = new_secret();
        let program = resolve_program(&cfg.path);
        let mut command = tokio::process::Command::new(&program);
        command
            .args(cfg.args(port, &secret))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(home) = &cfg.isolated_home {
            command
                .env("XDG_DATA_HOME", home.join("data"))
                .env("XDG_CONFIG_HOME", home.join("config"))
                .env("XDG_CACHE_HOME", home.join("cache"));
        }
        let mut child = command.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PlayerError::NotInstalled { player: "VLC", path: cfg.path.display().to_string() }
            } else {
                PlayerError::Launch(format!("{}: {e}", program.display()))
            }
        })?;

        let connected = tokio::select! {
            c = tokio::time::timeout(CONNECT_TIMEOUT, accept_module(&listener, &secret)) => c,
            status = child.wait() => {
                let code = status.ok().and_then(|s| s.code());
                return Err(PlayerError::Launch(format!(
                    "VLC exited (code {code:?}) before it started the sppy module"
                )));
            }
        };
        let (read, write) = match connected {
            Ok(Ok(io)) => io,
            Ok(Err(e)) => {
                let _ = child.kill().await;
                return Err(e);
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(PlayerError::Launch(
                    "VLC did not start the sppy module; is it VLC 3 with Lua?".into(),
                ));
            }
        };
        drop(listener);

        let (tx, events) = mpsc::unbounded_channel();
        let (requests, rx) = mpsc::unbounded_channel();
        tokio::spawn(drive(Link { read, write }, rx, tx.clone(), cfg.ytdlp.clone()));

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
        });
        Ok(VlcPlayer { requests, events, kill: Some(kill_tx) })
    }

    async fn request(&self, op: Op) -> PlayerResult {
        let (reply, rx) = oneshot::channel();
        self.requests.send((op, reply)).map_err(|_| PlayerError::Gone)?;
        rx.await.map_err(|_| PlayerError::Gone)?
    }
}

impl Drop for VlcPlayer {
    fn drop(&mut self) {
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
    }
}

struct Link {
    read: BufReader<OwnedReadHalf>,
    write: OwnedWriteHalf,
}

impl Link {
    async fn call(&mut self, cmd: &Command) -> PlayerResult<String> {
        let mut line = cmd.encode()?;
        line.push('\n');
        self.write.write_all(line.as_bytes()).await.map_err(ipc_error)?;
        let mut reply = String::new();
        match tokio::time::timeout(REPLY_TIMEOUT, self.read.read_line(&mut reply)).await {
            Ok(Ok(0)) => Err(PlayerError::Gone),
            Ok(Ok(_)) => Ok(reply.trim_end_matches(['\r', '\n']).to_owned()),
            Ok(Err(e)) => Err(ipc_error(e)),
            Err(_) => Err(PlayerError::Ipc("VLC did not answer".into())),
        }
    }

    async fn send(&mut self, cmd: &Command) -> PlayerResult {
        let reply = self.call(cmd).await?;
        Ok(wire::parse_ack(&reply)?)
    }
}

/// The current item, kept to open it again after VLC's Stop or the end.
struct Item {
    load: Command,
    subtitles: Vec<String>,
}

#[derive(Debug)]
enum ResolveError {
    NotInstalled,
    Failed(String),
}

async fn resolve(program: PathBuf, url: String) -> Result<ytdlp::Resolved, ResolveError> {
    let mut command = tokio::process::Command::new(&program);
    command.args(ytdlp::args(&url)).stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        // yt-dlp is a console program; the app has no console to share.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    match tokio::time::timeout(RESOLVE_TIMEOUT, command.output()).await {
        Err(_) => Err(ResolveError::Failed("took too long".into())),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Err(ResolveError::NotInstalled),
        Ok(Err(e)) => Err(ResolveError::Failed(e.to_string())),
        Ok(Ok(out)) if !out.status.success() => {
            Err(ResolveError::Failed(ytdlp::error_message(&String::from_utf8_lossy(&out.stderr))))
        }
        Ok(Ok(out)) => {
            ytdlp::parse(&String::from_utf8_lossy(&out.stdout)).map_err(ResolveError::Failed)
        }
    }
}

/// A finished yt-dlp run: the load it belongs to, the link, its subtitles and the result.
type ResolvedLink = (u64, String, Vec<String>, Result<ytdlp::Resolved, ResolveError>);

/// Owns the connection: serves requests and polls the status in between.
struct Driver {
    link: Link,
    tracker: Tracker,
    languages: TrackLanguages,
    item: Option<Item>,
    events: mpsc::UnboundedSender<PlayerEvent>,
    ytdlp: PathBuf,
    /// Counts loads, so that a link resolved for an earlier one is dropped.
    generation: u64,
    resolved_tx: mpsc::UnboundedSender<ResolvedLink>,
    start: Instant,
}

async fn drive(
    link: Link,
    mut requests: mpsc::UnboundedReceiver<Request>,
    events: mpsc::UnboundedSender<PlayerEvent>,
    ytdlp: PathBuf,
) {
    let (resolved_tx, mut resolved_rx) = mpsc::unbounded_channel();
    let mut d = Driver {
        link,
        tracker: Tracker::new(),
        languages: TrackLanguages::default(),
        item: None,
        events,
        ytdlp,
        generation: 0,
        resolved_tx,
        start: Instant::now(),
    };
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let actions = tokio::select! {
            req = requests.recv() => {
                let Some((op, reply)) = req else { break };
                let result = d.serve(op).await;
                let gone = matches!(result, Err(PlayerError::Gone));
                let _ = reply.send(result);
                if gone {
                    return;
                }
                continue;
            }
            Some((generation, url, subtitles, result)) = resolved_rx.recv() => {
                if generation != d.generation {
                    continue;
                }
                match d.resolved(url, subtitles, result).await {
                    Ok(actions) => actions,
                    Err(PlayerError::Gone) => return,
                    Err(e) => {
                        tracing::warn!("VLC: {e}");
                        continue;
                    }
                }
            }
            _ = tick.tick() => {
                let status = match d.link.call(&Command::Status).await {
                    Ok(line) => wire::parse_status(&line),
                    Err(PlayerError::Gone) => return,
                    Err(e) => {
                        tracing::warn!("VLC status: {e}");
                        continue;
                    }
                };
                match status {
                    Ok(s) => d.tracker.observe(d.now_ms(), s.as_ref()),
                    Err(e) => {
                        tracing::warn!("VLC status: {e}");
                        continue;
                    }
                }
            }
        };
        for action in actions {
            match d.act(action).await {
                Err(PlayerError::Gone) => return,
                Err(e) => tracing::warn!("VLC: {e}"),
                Ok(()) => {}
            }
        }
    }
    // The player was dropped: close VLC rather than leave it unsynchronised.
    let _ = d.link.send(&Command::Quit).await;
}

impl Driver {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    async fn act(&mut self, action: Action) -> PlayerResult {
        match action {
            Action::Event(e) => {
                let _ = self.events.send(e);
                Ok(())
            }
            Action::AddSubtitles => self.add_subtitles().await,
            Action::Reload => match &self.item {
                Some(it) => self.link.send(&it.load).await,
                None => Ok(()),
            },
            Action::Restore { position_ms, paused } => {
                let seek = self.link.send(&Command::Seek { us: position_ms * 1000 }).await;
                let state =
                    self.link.send(if paused { &Command::Pause } else { &Command::Play }).await;
                seek.and(state)
            }
        }
    }

    async fn add_subtitles(&mut self) -> PlayerResult {
        let Some(item) = &self.item else { return Ok(()) };
        // Each subtitle added becomes the shown one; the caller lists the preferred first.
        for sub in item.subtitles.iter().rev() {
            if let Err(e) = self.link.send(&Command::Sub(sub.clone())).await {
                tracing::warn!("cannot add subtitle {sub}: {e}");
            }
        }
        Ok(())
    }

    /// Send an item to VLC.
    async fn open(
        &mut self,
        uri: String,
        title: String,
        options: Vec<String>,
        subtitles: Vec<String>,
    ) -> PlayerResult {
        let load = Command::Load {
            uri,
            audio_languages: crate::languages::language_codes(&self.languages.audio),
            subtitle_languages: crate::languages::language_codes(&self.languages.subtitles),
            title,
            options,
        };
        // Checked before VLC loses the current item.
        load.encode()?;
        let now = self.now_ms();
        self.tracker.load_sent(now);
        let result = self.link.send(&load).await;
        self.item = Some(Item { load, subtitles });
        result
    }

    async fn resolved(
        &mut self,
        url: String,
        subtitles: Vec<String>,
        result: Result<ytdlp::Resolved, ResolveError>,
    ) -> PlayerResult<Vec<Action>> {
        match result {
            Ok(r) => {
                let options = ytdlp::item_options(&r);
                let title = r.title.unwrap_or_default();
                self.open(r.media, title, options, subtitles).await?;
            }
            Err(ResolveError::NotInstalled) => {
                tracing::warn!("yt-dlp not found; VLC opens the link with its own scripts");
                self.open(url, String::new(), Vec::new(), subtitles).await?;
            }
            Err(ResolveError::Failed(reason)) => {
                return Ok(self.tracker.fail(format!("yt-dlp: {reason}")));
            }
        }
        Ok(Vec::new())
    }

    async fn serve(&mut self, op: Op) -> PlayerResult {
        let now = self.now_ms();
        match op {
            Op::Load { media, subtitles } => {
                for s in &subtitles {
                    Command::Sub(s.clone()).encode()?;
                }
                self.generation += 1;
                match media {
                    Target::Uri(uri) => self.open(uri, String::new(), Vec::new(), subtitles).await,
                    Target::Link(url) => {
                        self.tracker.resolving();
                        self.item = None;
                        let (generation, tx) = (self.generation, self.resolved_tx.clone());
                        let program = self.ytdlp.clone();
                        tokio::spawn(async move {
                            let result = resolve(program, url.clone()).await;
                            let _ = tx.send((generation, url, subtitles, result));
                        });
                        // The previous item must not go on playing meanwhile.
                        self.link.send(&Command::Clear).await
                    }
                }
            }
            Op::Languages(l) => {
                self.languages = l;
                Ok(())
            }
            Op::Seek(position_ms) => {
                if self.tracker.seek_sent(position_ms, now) {
                    match &self.item {
                        Some(it) => self.link.send(&it.load).await,
                        None => Ok(()),
                    }
                } else {
                    self.link.send(&Command::Seek { us: position_ms * 1000 }).await
                }
            }
            Op::Play => self.link.send(&Command::Play).await,
            Op::Pause => self.link.send(&Command::Pause).await,
            Op::Speed(speed) => {
                self.tracker.speed_sent(speed);
                self.link.send(&Command::Rate { ppm: wire::speed_to_ppm(speed) }).await
            }
            Op::Osd { text, duration_ms } => {
                self.link.send(&Command::Osd { us: duration_ms * OSD_US_PER_MS, text }).await
            }
        }
    }
}

impl Player for VlcPlayer {
    fn capabilities(&self) -> Capabilities {
        Capabilities { speed: true, sub_add: true, osd: true, cache_ahead: false }
    }

    async fn load(&mut self, media: MediaSource, subtitles: Vec<MediaSource>) -> PlayerResult {
        let media = match &media {
            MediaSource::Link(url) => Target::Link(url.clone()),
            other => Target::Uri(source_uri(other)),
        };
        let subtitles = subtitles.iter().map(source_uri).collect();
        self.request(Op::Load { media, subtitles }).await
    }

    async fn set_track_languages(&mut self, languages: &TrackLanguages) -> PlayerResult {
        self.request(Op::Languages(languages.clone())).await
    }

    async fn play(&mut self) -> PlayerResult {
        self.request(Op::Play).await
    }

    async fn pause(&mut self) -> PlayerResult {
        self.request(Op::Pause).await
    }

    async fn seek(&mut self, position_ms: u64) -> PlayerResult {
        self.request(Op::Seek(position_ms)).await
    }

    async fn set_speed(&mut self, speed: f64) -> PlayerResult {
        self.request(Op::Speed(speed)).await
    }

    async fn osd(&mut self, text: &str, duration_ms: u64) -> PlayerResult {
        self.request(Op::Osd { text: text.to_owned(), duration_ms }).await
    }

    async fn next_event(&mut self) -> Option<PlayerEvent> {
        self.events.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn module_folder_follows_xdg_data_home() {
        assert_eq!(
            user_lua_intf_dir(env(&[("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.local/share/vlc/lua/intf"))
        );
        assert_eq!(
            user_lua_intf_dir(env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/d")])),
            Some(PathBuf::from("/d/vlc/lua/intf"))
        );
        // The base directory spec ignores relative values.
        assert_eq!(
            user_lua_intf_dir(env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "rel")])),
            Some(PathBuf::from("/home/u/.local/share/vlc/lua/intf"))
        );
        assert_eq!(user_lua_intf_dir(env(&[])), None);
    }

    #[cfg(windows)]
    #[test]
    fn module_folder_is_under_appdata() {
        assert_eq!(
            user_lua_intf_dir(env(&[("APPDATA", r"C:\Users\u\AppData\Roaming")])),
            Some(PathBuf::from(r"C:\Users\u\AppData\Roaming\vlc\lua\intf"))
        );
    }

    #[test]
    fn module_is_written_once_and_replaced_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let intf = dir.path().join("vlc").join("lua").join("intf");
        let file = install_module(&intf).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), MODULE);
        std::fs::write(&file, "-- an older sppy").unwrap();
        install_module(&intf).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), MODULE);
        let names: Vec<_> =
            std::fs::read_dir(&intf).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![OsString::from("sppy.lua")]);
    }

    #[test]
    fn module_announces_the_version_the_backend_expects() {
        assert!(MODULE.contains(&format!("local VERSION = \"{MODULE_VERSION}\"")));
    }

    #[test]
    fn secrets_differ() {
        assert_ne!(new_secret(), new_secret());
        assert_eq!(new_secret().len(), 32);
    }
}
