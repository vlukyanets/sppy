//! The client session loop: connection + player + sync, in one task (spec §7.1).

use std::time::Duration;

use sppy_player::{MediaSource, Player, PlayerError, PlayerEvent, PlayerLauncher};
use sppy_proto::{
    ClientMsg, CloseReason, Delivery, ErrorCode, Event, FileId, Item, ItemId, MediaStatus,
    PlaybackState, ReqId, ServerMsg, Source,
};
use sppy_sync::{
    Classified, Correction, DriftController, EchoFilter, Observed, PendingOp, RawEvent, Target,
    UserAction,
};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::Instant;

use crate::api::{ConnState, Snapshot, UiCommand, UiEvent};
use crate::cache::MediaCache;
use crate::config::{ClientConfig, DeliveryMode};
use crate::connection::{AuthToken, ConnEvent, ConnectionHandle, spawn_connection_at};
use crate::download::{DownloadEvent, DownloadJob, Downloader, Priority};
use crate::link::{ServerAddress, parse_address};
use crate::local_index::LocalIndex;
use crate::media::{Resolved, companion_url, file_url, is_vobsub, stream_source, subtitle_url};
use crate::notice::{nickname, render_notice};

const TICK: Duration = Duration::from_millis(250);
const ECHO_TIMEOUT_MS: u64 = 1000;
/// After a user action, leave the player alone until the server answers (or this long).
const AWAIT_REPLY_MS: u64 = 2000;
/// How long to wait for the `Seeked` of a position jump we did not cause.
const FOREIGN_JUMP_GRACE_MS: u64 = 1500;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const OSD_MS: u64 = 3000;
const CHAT_HISTORY: usize = 50;
/// The drift goes to the server at most this often (spec §5.3: progress ≤ 1 Hz).
const DRIFT_REPORT_MS: u64 = 1000;
pub(crate) const NO_PERMISSION: &str = "No permission to control playback";

/// What the player currently has and reports.
#[derive(Debug, Clone)]
struct MediaState {
    item: Option<ItemId>,
    file_loaded: bool,
    pos_ms: u64,
    pos_at_ms: u64,
    paused: bool,
    speed: f64,
    buffering: bool,
    cache_ms: u64,
    cache_eof: bool,
    local: bool,
    /// Waiting for a preload download of this item.
    downloading: bool,
    /// How the loaded item arrives (re-sent with the drift once per second).
    delivery: Option<Delivery>,
    last_drift_report_ms: u64,
    /// Session token baked into the loaded media URLs (server streams only).
    token: Option<String>,
    /// A position jump we did not cause: probably the user seeking; hold corrections until then.
    foreign_jump_until: u64,
    duration_ms: Option<u64>,
    title: Option<String>,
    status: MediaStatus,
}

impl Default for MediaState {
    fn default() -> Self {
        MediaState {
            item: None,
            file_loaded: false,
            pos_ms: 0,
            pos_at_ms: 0,
            paused: true,
            speed: 1.0,
            buffering: false,
            cache_ms: 0,
            cache_eof: false,
            local: false,
            downloading: false,
            delivery: None,
            last_drift_report_ms: 0,
            token: None,
            foreign_jump_until: 0,
            duration_ms: None,
            title: None,
            status: MediaStatus::Loading,
        }
    }
}

pub(crate) struct Session<L: PlayerLauncher> {
    config: ClientConfig,
    launcher: L,
    epoch: Instant,
    snap: Snapshot,
    state_tx: watch::Sender<Snapshot>,
    events_tx: broadcast::Sender<UiEvent>,
    conn: Option<ConnectionHandle>,
    session_token: Option<String>,
    http_base: Option<String>,
    player: Option<L::P>,
    last_exit_code: Option<Option<i32>>,
    last_crash: Option<Instant>,
    controller: DriftController,
    echo: EchoFilter,
    next_req: ReqId,
    awaiting: Option<(ReqId, u64)>,
    media: MediaState,
    downloader: Downloader,
    dl_rx: mpsc::UnboundedReceiver<DownloadEvent>,
    /// Items whose download failed: streamed instead.
    stream_fallback: std::collections::HashSet<ItemId>,
    cache: MediaCache,
    http: reqwest::Client,
    local_index: std::sync::Arc<std::sync::Mutex<LocalIndex>>,
    /// Signals that a (re)scan of my media folders finished.
    index_rx: mpsc::UnboundedReceiver<()>,
    /// Files chosen by hand per item.
    overrides: std::collections::HashMap<ItemId, std::path::PathBuf>,
    /// The user closed the player: do not reopen it on reconnect.
    player_closed_by_user: bool,
}

async fn next_conn_event(conn: &mut Option<ConnectionHandle>) -> Option<ConnEvent> {
    match conn {
        Some(c) => c.events.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_player_event<P: Player>(player: &mut Option<P>) -> Option<PlayerEvent> {
    match player {
        Some(p) => p.next_event().await,
        None => std::future::pending().await,
    }
}

impl<L: PlayerLauncher> Session<L> {
    pub(crate) fn new(
        config: ClientConfig,
        launcher: L,
        epoch: Instant,
        state_tx: watch::Sender<Snapshot>,
        events_tx: broadcast::Sender<UiEvent>,
    ) -> Self {
        let controller = DriftController::new(config.advanced.clone());
        let mut snap = state_tx.borrow().clone();
        snap.local.cache_used_bytes = MediaCache::new(config.media_cache_dir(), 0).used_bytes();
        let cache = MediaCache::new(
            config.media_cache_dir(),
            config.cache_limit_gb.saturating_mul(1 << 30),
        );
        let local_index = std::sync::Arc::new(std::sync::Mutex::new(LocalIndex::default()));
        let (index_tx, index_rx) = mpsc::unbounded_channel();
        spawn_local_index(local_index.clone(), config.local_media.clone(), index_tx);
        let client = reqwest::Client::new();
        let http = client.clone();
        let (downloader, dl_rx) = if config.download_backoff_ms.is_empty() {
            Downloader::spawn(client)
        } else {
            Downloader::with_backoff(
                client,
                config.download_backoff_ms.iter().map(|ms| Duration::from_millis(*ms)).collect(),
            )
        };
        Session {
            snap,
            config,
            launcher,
            epoch,
            state_tx,
            events_tx,
            conn: None,
            session_token: None,
            http_base: None,
            player: None,
            last_exit_code: None,
            last_crash: None,
            controller,
            echo: EchoFilter::new(ECHO_TIMEOUT_MS),
            next_req: 0,
            awaiting: None,
            media: MediaState::default(),
            downloader,
            dl_rx,
            stream_fallback: Default::default(),
            cache,
            http,
            local_index,
            index_rx,
            overrides: Default::default(),
            player_closed_by_user: false,
        }
    }

    pub(crate) async fn run(mut self, mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(cmd) => self.on_command(cmd).await,
                    None => return,
                },
                ev = next_conn_event(&mut self.conn) => self.on_conn_event(ev).await,
                ev = next_player_event(&mut self.player) => self.on_player_event(ev).await,
                ev = self.dl_rx.recv() => if let Some(ev) = ev { self.on_download_event(ev).await },
                Some(()) = self.index_rx.recv() => self.recheck_local().await,
                _ = tick.tick() => self.on_tick().await,
            }
            self.publish();
        }
    }

    fn local_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn publish(&mut self) {
        let now = self.local_ms();
        self.snap.player_position_ms = self.media.file_loaded.then(|| self.observed_position(now));
        self.snap.server_offset_ms =
            self.conn.as_ref().and_then(|c| c.clock.lock().expect("clock").offset_ms());
        self.state_tx.send_replace(self.snap.clone());
    }

    fn emit(&self, ev: UiEvent) {
        let _ = self.events_tx.send(ev);
    }

    fn send(&self, msg: ClientMsg) {
        if let Some(c) = &self.conn {
            let _ = c.tx.send(msg);
        }
    }

    fn request(&mut self, make: impl FnOnce(ReqId) -> ClientMsg) -> ReqId {
        self.next_req += 1;
        let req = self.next_req;
        self.send(make(req));
        req
    }

    async fn osd(&mut self, text: &str) {
        if let Some(p) = &mut self.player {
            let _ = p.osd(text, OSD_MS).await;
        }
    }

    // ---- UI commands -------------------------------------------------------------------

    async fn on_command(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::Connect { address, nickname } => match parse_address(&address) {
                Ok(addr) => match addr.token.clone() {
                    Some(token) => {
                        self.config.nickname = nickname;
                        self.start_connection(addr, AuthToken::Invite(token));
                    }
                    None => self.emit(UiEvent::Error("The invite link has no token".into())),
                },
                Err(e) => self.emit(UiEvent::Error(e.to_string())),
            },
            UiCommand::BecomeHostConnect { listen, host_token } => {
                let target = crate::netaddr::local_connect_addr(listen);
                let addr = ServerAddress {
                    secure: false,
                    host: target.ip().to_string(),
                    port: target.port(),
                    token: None,
                };
                self.start_connection(addr, AuthToken::Host(host_token));
            }
            UiCommand::Disconnect => {
                self.forget_server_media();
                self.conn = None;
                self.snap.connection = ConnState::Idle;
                self.snap.room = None;
                self.snap.playback = None;
            }
            UiCommand::Play => {
                self.request(|req_id| ClientMsg::Play { req_id, position_ms: None });
            }
            UiCommand::Pause => {
                self.request(|req_id| ClientMsg::Pause { req_id, position_ms: None });
            }
            UiCommand::Seek(position_ms) => {
                self.request(|req_id| ClientMsg::Seek { req_id, position_ms });
            }
            UiCommand::SetSpeed(speed) => {
                self.request(|req_id| ClientMsg::SetSpeed { req_id, speed });
            }
            UiCommand::PlaylistAdd(sources) => {
                let after =
                    self.snap.room.as_ref().and_then(|r| r.playlist.last()).map(|i| i.item_id);
                self.request(|req_id| ClientMsg::PlaylistAdd { req_id, sources, after });
            }
            UiCommand::PlaylistRemove(item_id) => {
                self.request(|req_id| ClientMsg::PlaylistRemove { req_id, item_id });
            }
            UiCommand::PlaylistMove { item_id, after } => {
                self.request(|req_id| ClientMsg::PlaylistMove { req_id, item_id, after });
            }
            UiCommand::PlaylistSelect(item_id) => {
                self.request(|req_id| ClientMsg::PlaylistSelect { req_id, item_id });
            }
            UiCommand::Chat(text) => self.send(ClientMsg::Chat { text }),
            UiCommand::GetLibrary => self.send(ClientMsg::GetLibrary),
            UiCommand::SetControlMode(mode) => {
                self.request(|req_id| ClientMsg::SetControlMode { req_id, mode });
            }
            UiCommand::SetCanControl(participant_id, allowed) => {
                self.request(|req_id| ClientMsg::SetCanControl { req_id, participant_id, allowed });
            }
            UiCommand::Kick(participant_id) => {
                self.request(|req_id| ClientMsg::Kick { req_id, participant_id });
            }
            UiCommand::RotateInvite => {
                self.request(|req_id| ClientMsg::RotateInvite { req_id });
            }
            UiCommand::RescanLibrary => {
                self.request(|req_id| ClientMsg::RescanLibrary { req_id });
            }
            UiCommand::RestartPlayer => {
                // Dropping the player kills it; a new one starts with the current launcher config.
                self.player = None;
                self.player_closed_by_user = false;
                self.last_crash = None;
                self.ensure_player().await;
            }
            UiCommand::StartWithoutWaiting => {
                self.request(|req_id| ClientMsg::StartWithoutWaiting { req_id });
            }
            UiCommand::OpenPlayer => {
                self.player_closed_by_user = false;
                self.ensure_player().await;
            }
            UiCommand::SetMode(mode) => {
                self.config.mode = mode;
                self.snap.local.mode = mode;
            }
            UiCommand::ClearCache => {
                if let Err(e) = self.cache.clear(&self.protected_files()) {
                    self.emit(UiEvent::Error(format!("Cannot clear the cache: {e}")));
                }
                self.snap.local.cache_used_bytes = self.cache.used_bytes();
            }
            UiCommand::ChooseLocalFile { item_id, path } => {
                self.choose_local_file(item_id, path).await
            }
        }
    }

    /// Item ids and tokens belong to one server: forget them when leaving it.
    fn forget_server_media(&mut self) {
        self.cancel_stale_download(None);
        self.media = MediaState::default();
        self.stream_fallback.clear();
        self.overrides.clear();
        self.session_token = None;
    }

    fn start_connection(&mut self, addr: ServerAddress, auth: AuthToken) {
        self.forget_server_media();
        self.http_base = Some(addr.http_base());
        self.conn = Some(spawn_connection_at(addr, auth, self.config.nickname.clone(), self.epoch));
        self.snap.connection = ConnState::Connecting;
    }

    // ---- connection events ---------------------------------------------------------------

    async fn on_conn_event(&mut self, ev: Option<ConnEvent>) {
        let Some(ev) = ev else {
            self.conn = None;
            if !matches!(self.snap.connection, ConnState::Rejected(_)) {
                self.snap.connection = ConnState::Idle;
            }
            return;
        };
        match ev {
            ConnEvent::Connected { welcome } => {
                self.snap.connection = ConnState::Connected;
                self.snap.me = Some(welcome.participant_id);
                self.snap.is_host = welcome.is_host;
                self.snap.room = Some(welcome.room);
                self.snap.playback = Some(welcome.playback);
                self.snap.chat_history = welcome.chat_history;
                let token_changed = self.session_token.as_ref() != Some(&welcome.session_token);
                self.session_token = Some(welcome.session_token);
                self.awaiting = None;
                if self.player.is_none() {
                    if !self.player_closed_by_user {
                        self.ensure_player().await;
                    }
                } else {
                    // Media URLs carry the session token: a new session invalidates them.
                    if token_changed && (self.media.token.is_some() || self.media.downloading) {
                        self.cancel_stale_download(None);
                        self.media = MediaState::default();
                    } else if self.media.item.is_some() && self.media.item == self.current_item_id()
                    {
                        // A fresh participant on the server starts as Loading: tell it where we are.
                        self.send_status();
                    }
                    self.apply_playback().await;
                }
            }
            ConnEvent::Message(msg) => self.on_server_msg(msg).await,
            ConnEvent::Disconnected { reason } => {
                tracing::info!("disconnected: {reason}");
                self.snap.connection = ConnState::Reconnecting { attempt: 0 };
                self.osd("Connection lost").await;
            }
            ConnEvent::Reconnecting { attempt, reason } => {
                tracing::debug!("reconnecting (attempt {attempt}): {reason}");
                self.snap.connection = ConnState::Reconnecting { attempt };
            }
            ConnEvent::Rejected { code, message } => {
                self.snap.connection = ConnState::Rejected(code);
                self.emit(UiEvent::Error(message));
            }
        }
    }

    async fn on_server_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Playback(p) => {
                if let (Some((req, _)), Some(cause)) = (self.awaiting, p.cause)
                    && Some(cause.participant_id) == self.snap.me
                    && cause.req_id == req
                {
                    self.awaiting = None;
                }
                self.snap.playback = Some(p);
                self.apply_playback().await;
            }
            ServerMsg::Room(r) => {
                if let Some(me) = self.snap.me
                    && let Some(p) = r.participants.iter().find(|p| p.id == me)
                {
                    self.snap.is_host = p.is_host;
                }
                self.snap.room = Some(r);
                self.reassert_status();
                let ids: std::collections::HashSet<ItemId> = self
                    .snap
                    .room
                    .iter()
                    .flat_map(|r| r.playlist.iter().map(|i| i.item_id))
                    .collect();
                self.overrides.retain(|id, _| ids.contains(id));
                self.recheck_local().await;
            }
            ServerMsg::ProgressBatch { entries } => {
                self.snap.progress = entries.into_iter().map(|e| (e.participant_id, e)).collect();
            }
            ServerMsg::Event(Event::Chat(c)) => {
                let line = format!("{}: {}", c.nickname, c.text);
                self.snap.chat_history.push(c.clone());
                if self.snap.chat_history.len() > CHAT_HISTORY {
                    self.snap.chat_history.remove(0);
                }
                let from_me = Some(c.from) == self.snap.me;
                self.emit(UiEvent::Chat(c));
                if !from_me {
                    self.osd(&line).await;
                }
            }
            ServerMsg::Event(Event::Notice { actor, action }) => {
                let Some(room) = &self.snap.room else { return };
                let actor_id = actor;
                let actor = actor.map(|a| nickname(room, a));
                let text = render_notice(actor.as_deref(), &action, room);
                let mine = actor_id.is_some() && actor_id == self.snap.me;
                self.emit(UiEvent::Notice(text.clone()));
                if crate::notice::osd_worthy(&action, mine) {
                    self.osd(&text).await;
                }
            }
            ServerMsg::Denied { req_id, code } => {
                if self.awaiting.is_some_and(|(r, _)| r == req_id) {
                    self.awaiting = None;
                }
                let text = match code {
                    ErrorCode::NotPermitted => NO_PERMISSION.to_owned(),
                    ErrorCode::NotHost => "Only the host can do that".to_owned(),
                    other => format!("Request rejected: {other:?}"),
                };
                self.osd(&text).await;
                self.emit(UiEvent::Error(text));
            }
            ServerMsg::Library { roots } => self.snap.library = Some(roots),
            ServerMsg::InviteToken { token } => self.snap.invite_token = Some(token),
            ServerMsg::Closing { reason } => match reason {
                CloseReason::Kicked => {
                    self.emit(UiEvent::Error("You were removed from the room".into()));
                    self.snap.connection = ConnState::Idle;
                }
                CloseReason::ServerShutdown => {
                    self.emit(UiEvent::Notice("The server is shutting down".into()));
                }
                CloseReason::ProtocolError => {
                    self.emit(UiEvent::Error("Protocol error".into()));
                }
            },
            ServerMsg::Welcome(_) | ServerMsg::Reject { .. } | ServerMsg::Pong { .. } => {}
        }
    }

    /// "Waiting for alice, bob" while the server holds the room.
    fn waiting_text(&self) -> Option<String> {
        let hold = self.snap.playback.as_ref()?.hold.as_ref()?;
        let room = self.snap.room.as_ref()?;
        Some(crate::notice::waiting_line(hold, room, &self.snap.progress, self.snap.me))
    }

    fn current_item_id(&self) -> Option<ItemId> {
        self.snap.playback.as_ref().and_then(|p| p.item_id)
    }

    /// Load the room's current item if the player has something else.
    async fn apply_playback(&mut self) {
        let wanted = self.current_item_id();
        if wanted == self.media.item || self.player.is_none() {
            return;
        }
        self.cancel_stale_download(wanted);
        let Some(id) = wanted else {
            self.media = MediaState::default();
            return;
        };
        let Some(item) = self.item(id) else { return };
        for (url, path) in self.missing_vobsubs(&item) {
            if let Err(e) = fetch_to_file(&self.http.clone(), &url, &path).await {
                tracing::warn!("cannot fetch VobSub subtitle: {e}");
            }
        }
        let Some(resolved) = self.resolve(&item) else { return };
        self.start_item(id, resolved).await;
    }

    fn item(&self, id: ItemId) -> Option<Item> {
        self.snap.room.as_ref()?.playlist.iter().find(|i| i.item_id == id).cloned()
    }

    fn cache_paths(
        &self,
        file_id: &FileId,
        item: &Item,
    ) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
        let dir = self.cache.dir();
        let subs = item
            .subtitles
            .iter()
            .map(|s| dir.join(format!("{}.sub{}.{}", file_id.0, s.index, s.ext)))
            .collect();
        (self.cache.path_for(file_id), subs)
    }

    /// Library files referenced by the playlist (never evicted).
    fn protected_files(&self) -> std::collections::HashSet<FileId> {
        self.snap
            .room
            .iter()
            .flat_map(|r| r.playlist.iter())
            .filter_map(|i| match &i.source {
                Source::File { file_id } => Some(file_id.clone()),
                Source::Url { .. } => None,
            })
            .collect()
    }

    /// A file on this machine for the item: chosen by hand, or found by fingerprint.
    fn local_copy(&self, item: &Item) -> Option<std::path::PathBuf> {
        if let Some(p) = self.overrides.get(&item.item_id) {
            return Some(p.clone());
        }
        if !matches!(item.source, Source::File { .. }) {
            return None;
        }
        let fp = item.fingerprint.as_ref()?;
        self.local_index.lock().expect("local index").find(fp)
    }

    /// While we are still downloading the current item, switch to a local copy if one appears
    /// (the fingerprint arrived later, or the folder scan finished).
    async fn recheck_local(&mut self) {
        if !self.media.downloading {
            return;
        }
        let Some(item) = self.media.item.and_then(|id| self.item(id)) else { return };
        if self.local_copy(&item).is_none() {
            return;
        }
        if let Source::File { file_id } = &item.source {
            self.downloader.cancel(file_id);
        }
        if let Some(resolved) = self.resolve(&item) {
            self.start_item(item.item_id, resolved).await;
        }
    }

    async fn choose_local_file(&mut self, item_id: ItemId, path: std::path::PathBuf) {
        let Some(item) = self.item(item_id) else { return };
        let fp = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || sppy_common::media::fingerprint(&path)).await
        };
        match fp {
            Ok(Ok(fp)) if item.fingerprint.as_ref() == Some(&fp) => {}
            Ok(Ok(_)) => self.emit(UiEvent::Error(
                "The file does not match the server's copy; sync will rely on the position only"
                    .into(),
            )),
            _ => {
                self.emit(UiEvent::Error(format!("Cannot read {}", path.display())));
                return;
            }
        }
        self.overrides.insert(item_id, path);
        if self.media.item == Some(item_id) {
            self.cancel_stale_download(None);
            self.media = MediaState::default();
            self.apply_playback().await;
        }
    }

    /// Stop downloading an item that is no longer current (the `.part` stays for later).
    fn cancel_stale_download(&mut self, wanted: Option<ItemId>) {
        if !self.media.downloading || self.media.item == wanted {
            return;
        }
        if let Some(Source::File { file_id }) =
            self.media.item.and_then(|i| self.item(i)).map(|i| i.source)
        {
            self.downloader.cancel(&file_id);
        }
    }

    fn download_job(&self, item: &Item, file_id: &FileId) -> Option<DownloadJob> {
        let (base, token) = (self.http_base.as_ref()?, self.session_token.as_ref()?);
        let (dest, subs) = self.cache_paths(file_id, item);
        let subs = item
            .subtitles
            .iter()
            .zip(subs)
            .flat_map(|(s, path)| {
                let mut v = vec![(subtitle_url(base, &file_id.0, s.index, token), path.clone())];
                if is_vobsub(s) {
                    v.push((
                        companion_url(base, &file_id.0, s.index, token),
                        path.with_extension("sub"),
                    ));
                }
                v
            })
            .collect();
        Some(DownloadJob {
            file_id: file_id.clone(),
            url: file_url(base, &file_id.0, token),
            dest,
            subs,
            total_hint: item.fingerprint.as_ref().map(|f| f.size),
        })
    }

    /// Preload mode: fetch the item after the current one in the background.
    fn maybe_prefetch(&self) {
        if !self.config.prefetch_next || self.config.mode != DeliveryMode::Preload {
            return;
        }
        let Some(next) = self.media.item.and_then(|id| {
            let list = &self.snap.room.as_ref()?.playlist;
            let pos = list.iter().position(|i| i.item_id == id)?;
            list.get(pos + 1).cloned()
        }) else {
            return;
        };
        if let Source::File { file_id } = &next.source
            && self.cache.complete(file_id).is_none()
            && let Some(job) = self.download_job(&next, file_id)
        {
            self.downloader.enqueue(job, Priority::Prefetch);
        }
    }

    /// Decide how the item reaches the player: cache, download (Wait) or stream.
    fn resolve(&self, item: &Item) -> Option<Resolved> {
        let (base, token) = (self.http_base.as_ref()?, self.session_token.as_ref()?);
        // The player shows the first subtitle: put the preferred language first.
        let ordered = Item {
            subtitles: crate::media::order_subtitles(
                &item.subtitles,
                &self.config.subtitle_languages,
            ),
            ..item.clone()
        };
        let item = &ordered;
        if let Some(path) = self.local_copy(item) {
            return Some(Resolved::Local {
                path,
                subtitles: self.subtitle_sources(item, base, token),
            });
        }
        if let Source::File { file_id } = &item.source {
            // Spec §7.1: local copy, then a completed cache entry, then the delivery mode.
            if let Some(path) = self.cache.complete(file_id) {
                self.cache.touch(file_id);
                let (_, subs) = self.cache_paths(file_id, item);
                let subtitles =
                    subs.into_iter().filter(|p| p.is_file()).map(MediaSource::Path).collect();
                return Some(Resolved::Local { path, subtitles });
            }
            if self.config.mode == DeliveryMode::Preload
                && !self.stream_fallback.contains(&item.item_id)
            {
                self.downloader.enqueue(self.download_job(item, file_id)?, Priority::Current);
                return Some(Resolved::Wait);
            }
        }
        Some(match stream_source(base, item, token) {
            Resolved::Stream { media, .. } if matches!(item.source, Source::File { .. }) => {
                Resolved::Stream { media, subtitles: self.subtitle_sources(item, base, token) }
            }
            other => other,
        })
    }

    /// Subtitles of a library item for the player: URLs, except VobSub pairs, which must be
    /// local files (see [`Self::missing_vobsubs`]); a VobSub not fetched yet is left out.
    fn subtitle_sources(&self, item: &Item, base: &str, token: &str) -> Vec<MediaSource> {
        let Source::File { file_id } = &item.source else { return Vec::new() };
        let (_, local) = self.cache_paths(file_id, item);
        item.subtitles
            .iter()
            .zip(local)
            .filter_map(|(s, path)| {
                if is_vobsub(s) {
                    (path.is_file() && path.with_extension("sub").is_file())
                        .then_some(MediaSource::Path(path))
                } else {
                    Some(MediaSource::Url(subtitle_url(base, &file_id.0, s.index, token)))
                }
            })
            .collect()
    }

    /// VobSub `.idx` + `.sub` pairs of a library item still missing from the cache, as
    /// `(url, path)` downloads that put both files side by side.
    fn missing_vobsubs(&self, item: &Item) -> Vec<(String, std::path::PathBuf)> {
        let (Source::File { file_id }, Some(base), Some(token)) =
            (&item.source, self.http_base.as_ref(), self.session_token.as_ref())
        else {
            return Vec::new();
        };
        let (_, local) = self.cache_paths(file_id, item);
        let mut out = Vec::new();
        for (s, idx_path) in item.subtitles.iter().zip(local).filter(|(s, _)| is_vobsub(s)) {
            let sub_path = idx_path.with_extension("sub");
            if !(idx_path.is_file() && sub_path.is_file()) {
                out.push((subtitle_url(base, &file_id.0, s.index, token), idx_path));
                out.push((companion_url(base, &file_id.0, s.index, token), sub_path));
            }
        }
        out
    }

    async fn start_item(&mut self, id: ItemId, resolved: Resolved) {
        let (media, subs, delivery) = match resolved {
            Resolved::Stream { media, subtitles } => (media, subtitles, Delivery::Stream),
            Resolved::Local { path, subtitles } => {
                (MediaSource::Path(path), subtitles, Delivery::Local)
            }
            Resolved::Rejected { reason } => {
                self.media = MediaState { item: Some(id), ..MediaState::default() };
                self.emit(UiEvent::Error(reason.clone()));
                self.set_status_forced(MediaStatus::Failed { message: reason });
                return;
            }
            Resolved::Wait => {
                let total = self.item(id).and_then(|i| i.fingerprint).map_or(0, |f| f.size);
                self.media =
                    MediaState { item: Some(id), downloading: true, ..MediaState::default() };
                self.send_status();
                self.send(ClientMsg::Progress {
                    item_id: id,
                    delivery: Delivery::Download { bytes: 0, total },
                    drift_ms: None,
                });
                self.osd("Downloading…").await;
                return;
            }
        };
        let streamed_from_server =
            matches!(&media, MediaSource::Url(u) if u.contains("/media/") && u.contains("?t="));
        self.media = MediaState {
            item: Some(id),
            local: matches!(media, MediaSource::Path(_)),
            token: streamed_from_server.then(|| self.session_token.clone()).flatten(),
            ..MediaState::default()
        };
        self.echo.issued(self.local_ms(), PendingOp::Load);
        if let Some(p) = &mut self.player
            && let Err(e) = p.load(media, subs).await
        {
            tracing::warn!("load failed: {e}");
        }
        self.send_status();
        let local = delivery == Delivery::Local;
        self.media.delivery = Some(delivery.clone());
        self.send(ClientMsg::Progress { item_id: id, delivery, drift_ms: None });
        if local {
            self.maybe_prefetch();
        }
    }

    /// The current item if it is the library file `file_id` and we are waiting for its download.
    fn downloading_item(&self, file_id: &FileId) -> Option<Item> {
        let item = self.item(self.media.item?)?;
        let matches = matches!(&item.source, Source::File { file_id: f } if f == file_id);
        (matches && self.media.downloading).then_some(item)
    }

    async fn on_download_event(&mut self, ev: DownloadEvent) {
        match ev {
            DownloadEvent::Progress { file_id, bytes, total } => {
                if let Some(item) = self.downloading_item(&file_id) {
                    self.send(ClientMsg::Progress {
                        item_id: item.item_id,
                        delivery: Delivery::Download { bytes, total },
                        drift_ms: None,
                    });
                    let _ = (bytes, total);
                }
            }
            DownloadEvent::Done { file_id } => {
                if self.downloading_item(&file_id).is_some() {
                    self.osd("Download complete").await;
                }
                if let Err(e) = self.cache.evict(&self.protected_files()) {
                    tracing::warn!("cache eviction failed: {e}");
                }
                self.snap.local.cache_used_bytes = self.cache.used_bytes();
                if let Some(item) = self.downloading_item(&file_id)
                    && let Some(resolved) = self.resolve(&item)
                {
                    self.start_item(item.item_id, resolved).await;
                }
            }
            DownloadEvent::Failed { file_id, attempts } => {
                if let Some(item) = self.downloading_item(&file_id) {
                    tracing::warn!("download failed after {attempts} attempts; streaming instead");
                    self.stream_fallback.insert(item.item_id);
                    if let Some(resolved) = self.resolve(&item) {
                        self.start_item(item.item_id, resolved).await;
                    }
                }
            }
        }
    }

    /// The client is the source of truth for its own status. If the server shows something
    /// else for the item we have loaded (e.g. it restarted the same item and reset everyone to
    /// Loading), say it again; otherwise the room would wait for us forever.
    fn reassert_status(&self) {
        let (Some(me), Some(room)) = (self.snap.me, &self.snap.room) else { return };
        if self.media.item.is_none() || self.media.item != self.current_item_id() {
            return;
        }
        let server_view = room.participants.iter().find(|p| p.id == me).map(|p| &p.status);
        if server_view.is_some_and(|s| *s != self.media.status) {
            self.send_status();
        }
    }

    fn send_status(&self) {
        if let Some(item_id) = self.media.item {
            self.send(ClientMsg::SetStatus { item_id, status: self.media.status.clone() });
        }
    }

    fn set_status_forced(&mut self, status: MediaStatus) {
        self.media.status = status;
        self.send_status();
    }

    fn set_status(&mut self, status: MediaStatus) {
        if self.media.status != status {
            self.media.status = status;
            self.send_status();
        }
    }

    /// Loading/Stalled → Ready once there is a reserve (spec §6.4).
    fn update_status(&mut self) {
        if !self.media.file_loaded || self.media.buffering {
            return;
        }
        if !matches!(self.media.status, MediaStatus::Loading | MediaStatus::Stalled) {
            return;
        }
        let reserve_ms = self
            .snap
            .room
            .as_ref()
            .map(|r| u64::from(r.settings.resume_buffer_secs) * 1000)
            .unwrap_or(10_000);
        if self.media.local || self.media.cache_eof || self.media.cache_ms >= reserve_ms {
            let status = MediaStatus::Ready {
                duration_ms: self.media.duration_ms,
                title: self.media.title.clone(),
            };
            self.set_status(status);
        }
    }

    // ---- player ----------------------------------------------------------------------------

    async fn ensure_player(&mut self) {
        if self.player.is_some() {
            return;
        }
        match self.launcher.launch().await {
            Ok(p) => {
                self.player = Some(p);
                self.snap.player_open = true;
                self.last_exit_code = None;
                self.media = MediaState::default();
                self.echo = EchoFilter::new(ECHO_TIMEOUT_MS);
                self.controller = DriftController::new(self.config.advanced.clone());
                self.apply_playback().await;
            }
            Err(PlayerError::NotInstalled(path)) => {
                self.snap.player_open = false;
                self.emit(UiEvent::PlayerUnavailable(format!(
                    "mpv not found ({path}): install mpv or set its path in Settings"
                )));
            }
            Err(e) => {
                self.snap.player_open = false;
                self.emit(UiEvent::PlayerUnavailable(format!("Cannot start the player: {e}")));
            }
        }
    }

    fn player_closed(&mut self) {
        self.player_closed_by_user = true;
        self.snap.player_open = false;
        if let Some(item_id) = self.media.item {
            self.send(ClientMsg::SetStatus { item_id, status: MediaStatus::PlayerClosed });
        }
        self.media = MediaState::default();
    }

    async fn on_player_event(&mut self, ev: Option<PlayerEvent>) {
        let Some(ev) = ev else {
            self.player = None;
            let code = self.last_exit_code.take().flatten();
            if code == Some(0) {
                self.player_closed();
            } else if self.last_crash.is_some_and(|t| t.elapsed() < CRASH_WINDOW) {
                tracing::warn!("player crashed again (exit code {code:?})");
                self.player_closed();
            } else {
                tracing::warn!("player crashed (exit code {code:?}), restarting");
                self.last_crash = Some(Instant::now());
                self.ensure_player().await;
            }
            return;
        };
        let now = self.local_ms();
        match ev {
            PlayerEvent::TimePos(ms) => {
                let jump = ms.abs_diff(self.observed_position(now));
                if self.media.file_loaded
                    && jump > self.config.advanced.seek_threshold_ms
                    && !self.echo.seek_pending(now)
                {
                    self.media.foreign_jump_until = now + FOREIGN_JUMP_GRACE_MS;
                }
                self.media.pos_ms = ms;
                self.media.pos_at_ms = now;
            }
            PlayerEvent::Pause(paused) => {
                self.media.paused = paused;
                if self.media.file_loaded {
                    self.classify(RawEvent::Pause(paused)).await;
                }
            }
            PlayerEvent::Seeked { position_ms } => {
                self.media.foreign_jump_until = 0;
                self.media.pos_ms = position_ms;
                self.media.pos_at_ms = now;
                if self.media.file_loaded {
                    self.classify(RawEvent::Seeked { position_ms }).await;
                }
            }
            PlayerEvent::Speed(speed) => {
                self.media.speed = speed;
                if self.media.file_loaded {
                    self.classify(RawEvent::Speed(speed)).await;
                }
            }
            PlayerEvent::FileLoaded { duration_ms, title } => {
                self.media.file_loaded = true;
                self.media.duration_ms = duration_ms;
                self.media.title = title;
                self.media.pos_ms = 0;
                self.media.pos_at_ms = now;
                self.echo.load_finished();
                self.controller.reset_settle(now);
                self.update_status();
            }
            PlayerEvent::Buffering(buffering) => {
                self.media.buffering = buffering;
                if buffering {
                    if matches!(self.media.status, MediaStatus::Ready { .. }) {
                        self.set_status(MediaStatus::Stalled);
                    }
                } else {
                    self.update_status();
                }
            }
            PlayerEvent::CacheAhead { ms, eof } => {
                self.media.cache_ms = ms;
                self.media.cache_eof = eof;
                self.update_status();
            }
            PlayerEvent::Eof => {
                // A late EOF of the previous file must not end the item we are loading now.
                if !self.media.file_loaded || self.echo.is_loading() {
                    return;
                }
                if let Some(item_id) = self.media.item {
                    self.send(ClientMsg::ItemEnded { item_id });
                }
            }
            PlayerEvent::LoadFailed { message } => {
                self.emit(UiEvent::Error(format!(
                    "The player could not open the media: {message}"
                )));
                self.set_status(MediaStatus::Failed { message });
            }
            PlayerEvent::Exited { code } => self.last_exit_code = Some(code),
        }
    }

    fn observed_position(&self, now: u64) -> u64 {
        let m = &self.media;
        if m.paused || m.buffering {
            m.pos_ms
        } else {
            m.pos_ms + (now.saturating_sub(m.pos_at_ms) as f64 * m.speed).round() as u64
        }
    }

    async fn classify(&mut self, ev: RawEvent) {
        let now = self.local_ms();
        let position_ms = self.observed_position(now);
        match self.echo.observe(now, ev, position_ms) {
            Classified::Ours | Classified::Ignored => {}
            Classified::User(action) => {
                let req = match action {
                    UserAction::Pause { position_ms } => self.request(|req_id| ClientMsg::Pause {
                        req_id,
                        position_ms: Some(position_ms),
                    }),
                    UserAction::Play { position_ms } => {
                        // The room may be waiting for someone: playing locally would just be
                        // undone, so say why.
                        if let Some(text) = self.waiting_text() {
                            self.osd(&text).await;
                        }
                        self.request(|req_id| ClientMsg::Play {
                            req_id,
                            position_ms: Some(position_ms),
                        })
                    }
                    UserAction::Seek { position_ms } => {
                        self.request(|req_id| ClientMsg::Seek { req_id, position_ms })
                    }
                };
                self.awaiting = Some((req, now + AWAIT_REPLY_MS));
            }
            Classified::RevertSpeed => {
                let room_speed = self.snap.playback.as_ref().map_or(1.0, |p| p.speed);
                let speed = self.controller.desired_speed(room_speed);
                self.echo.issued(now, PendingOp::Speed(speed));
                if let Some(p) = &mut self.player {
                    let _ = p.set_speed(speed).await;
                }
            }
        }
    }

    // ---- sync tick ---------------------------------------------------------------------------

    async fn on_tick(&mut self) {
        let now = self.local_ms();
        if self.awaiting.is_some_and(|(_, deadline)| now > deadline) {
            self.awaiting = None;
        }
        if self.awaiting.is_some()
            || !self.media.file_loaded
            || self.player.is_none()
            || now < self.media.foreign_jump_until
            // The player cannot move while buffering; long stalls are the server's business.
            || self.media.buffering
        {
            return;
        }
        let Some(pb) = self.snap.playback.clone() else { return };
        if pb.item_id != self.media.item {
            return;
        }
        let Some(now_server) =
            self.conn.as_ref().and_then(|c| c.clock.lock().expect("clock").to_server_ms(now))
        else {
            return;
        };
        let target = Target {
            position_ms: pb.expected_position_ms(now_server),
            playing: pb.is_playing(),
            room_speed: pb.speed,
        };
        let observed = Observed {
            position_ms: self.observed_position(now),
            paused: self.media.paused,
            speed: self.media.speed,
        };
        if target.playing
            && now.saturating_sub(self.media.last_drift_report_ms) >= DRIFT_REPORT_MS
            && let (Some(item_id), Some(delivery)) = (self.media.item, self.media.delivery.clone())
        {
            self.media.last_drift_report_ms = now;
            let drift_ms = Some(observed.position_ms as i64 - target.position_ms as i64);
            self.send(ClientMsg::Progress { item_id, delivery, drift_ms });
        }
        for c in self.controller.update(now, &target, &observed) {
            self.apply_correction(now, c, &pb).await;
        }
    }

    async fn apply_correction(&mut self, now: u64, c: Correction, _pb: &PlaybackState) {
        let Some(p) = &mut self.player else { return };
        let result = match c {
            Correction::SetSpeed(speed) => {
                self.echo.issued(now, PendingOp::Speed(speed));
                p.set_speed(speed).await
            }
            Correction::Seek { position_ms } => {
                self.echo.issued(now, PendingOp::Seek { target_ms: position_ms });
                p.seek(position_ms).await
            }
            Correction::Pause => {
                self.echo.issued(now, PendingOp::Pause(true));
                p.pause().await
            }
            Correction::Play => {
                self.echo.issued(now, PendingOp::Pause(false));
                p.play().await
            }
        };
        if let Err(e) = result {
            tracing::debug!("player command failed: {e}");
        }
    }
}

/// Build the local index in the background, then refresh it every 10 minutes.
fn spawn_local_index(
    index: std::sync::Arc<std::sync::Mutex<LocalIndex>>,
    roots: Vec<std::path::PathBuf>,
    done: mpsc::UnboundedSender<()>,
) {
    if roots.is_empty() {
        return;
    }
    tokio::spawn(async move {
        loop {
            let (idx, r) = (index.clone(), roots.clone());
            let scanned = tokio::task::spawn_blocking(move || {
                let mut fresh = LocalIndex::default();
                std::mem::swap(&mut fresh, &mut *idx.lock().expect("local index"));
                fresh.rescan(&r);
                *idx.lock().expect("local index") = fresh;
            })
            .await;
            if scanned.is_err() || done.send(()).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
}

async fn fetch_to_file(
    client: &reqwest::Client,
    url: &str,
    path: &std::path::Path,
) -> Result<(), String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    }
    tokio::fs::write(path, &body).await.map_err(|e| e.to_string())
}
